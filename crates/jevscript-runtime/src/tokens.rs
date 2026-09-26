//! Token estimation.
//!
//! The context window is the constraint (spec principle 2). `shape` caps fields
//! in tokens, `focus` recurses until a text fits, and the runtime estimates
//! tokens before sending a request so that an oversized state pauses with
//! `state_too_large` naming the largest subject rather than failing at the API
//! (spec section 6.10).
//!
//! The limits themselves come from a model profile keyed by model id, and the
//! profile also names which estimator to use. See [`crate::profile`].

use crate::profile::{TiktokenEncoding, Tokenizer};
use tiktoken_rs::{
    CoreBPE, cl100k_base_singleton, o200k_base_singleton, o200k_harmony_singleton,
    p50k_base_singleton, p50k_edit_singleton, r50k_base_singleton,
};

/// Estimates text size using the selected model profile (spec section 10.6).
pub trait TokenEstimator: Send + Sync {
    /// The estimated token count of `text`.
    fn estimate(&self, text: &str) -> u64;
}

/// The `chars4` profile estimator rounds characters divided by four up (spec 10.6).
#[derive(Debug, Clone, Copy, Default)]
pub struct CharsPerToken;

impl TokenEstimator for CharsPerToken {
    fn estimate(&self, text: &str) -> u64 {
        text.chars().count().div_ceil(4) as u64
    }
}

/// An offline tiktoken estimator selected by a model profile (spec section 10.6).
struct TiktokenEstimator {
    encoding: &'static CoreBPE,
}

impl TiktokenEstimator {
    fn new(encoding: TiktokenEncoding) -> Self {
        let encoding = match encoding {
            TiktokenEncoding::O200kHarmony => o200k_harmony_singleton(),
            TiktokenEncoding::O200kBase => o200k_base_singleton(),
            TiktokenEncoding::Cl100kBase => cl100k_base_singleton(),
            TiktokenEncoding::P50kBase => p50k_base_singleton(),
            TiktokenEncoding::P50kEdit => p50k_edit_singleton(),
            TiktokenEncoding::R50kBase | TiktokenEncoding::Gpt2 => r50k_base_singleton(),
        };
        Self { encoding }
    }
}

impl TokenEstimator for TiktokenEstimator {
    fn estimate(&self, text: &str) -> u64 {
        self.encoding.count_ordinary(text) as u64
    }
}

/// The estimator selected by a model profile (spec section 10.6).
pub fn estimator_for(tokenizer: &Tokenizer) -> Box<dyn TokenEstimator> {
    match tokenizer {
        Tokenizer::Chars4 => Box::new(CharsPerToken),
        Tokenizer::Tiktoken(encoding) => Box::new(TiktokenEstimator::new(*encoding)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heuristic_rounds_up() {
        assert_eq!(CharsPerToken.estimate(""), 0);
        assert_eq!(CharsPerToken.estimate("abc"), 1);
        assert_eq!(CharsPerToken.estimate("abcd"), 1);
        assert_eq!(CharsPerToken.estimate("abcde"), 2);
    }

    #[test]
    fn chars4_counts_unicode_characters_not_bytes() {
        // Spec 10.6 defines chars4 in characters, so multibyte UTF-8 is one char.
        assert_eq!(CharsPerToken.estimate("é🙂中abc"), 2);
    }

    #[test]
    fn a_profile_selects_a_real_tiktoken_encoding() {
        // `hello world` is two cl100k tokens, while chars4 estimates three.
        let estimator = estimator_for(&Tokenizer::Tiktoken(TiktokenEncoding::Cl100kBase));
        assert_eq!(estimator.estimate("hello world"), 2);
        assert_eq!(CharsPerToken.estimate("hello world"), 3);
    }

    #[test]
    fn tiktoken_counts_unicode_text() {
        // OpenAI's cl100k reference count for this Japanese phrase is nine.
        let estimator = estimator_for(&Tokenizer::Tiktoken(TiktokenEncoding::Cl100kBase));
        assert_eq!(estimator.estimate("お誕生日おめでとう"), 9);
    }

    #[test]
    fn tiktoken_treats_special_token_looking_text_as_ordinary() {
        // Spec 10.6 requires state text to remain ordinary input.
        let estimator = estimator_for(&Tokenizer::Tiktoken(TiktokenEncoding::Cl100kBase));
        assert!(estimator.estimate("<|endoftext|>") > 1);
    }

    #[test]
    fn every_supported_profile_encoding_can_estimate_ordinary_text() {
        // Spec 10.6 promises all seven encodings work offline, not only cl100k.
        for encoding in [
            TiktokenEncoding::O200kHarmony,
            TiktokenEncoding::O200kBase,
            TiktokenEncoding::Cl100kBase,
            TiktokenEncoding::P50kBase,
            TiktokenEncoding::P50kEdit,
            TiktokenEncoding::R50kBase,
            TiktokenEncoding::Gpt2,
        ] {
            let estimator = estimator_for(&Tokenizer::Tiktoken(encoding));
            assert_eq!(estimator.estimate(""), 0, "{encoding:?}");
            assert_eq!(estimator.estimate("hello world"), 2, "{encoding:?}");
            assert!(estimator.estimate("<|endoftext|>") > 1, "{encoding:?}");
        }
    }
}
