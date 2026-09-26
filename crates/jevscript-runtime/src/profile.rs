//! Model profiles (spec section 10.6).
//!
//! Everything the runtime needs to know about a model is a profile, keyed by
//! model id: token limits, request caps, the tokenizer and prices. Profiles are
//! the only place those numbers live. Programs never state them and this crate
//! never hard-codes them — the bundle below is data with a date on it, not a
//! constant in the middle of the size check.
//!
//! A host overrides or adds profiles with the SDK option `profiles: <path>` or
//! the environment variable [`PROFILES_ENV`], pointing at a JSON file holding a
//! list of profile records. A run whose model id has no profile fails to start
//! with [`RuntimeErrorCode::ProfileMissing`].

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{RuntimeError, RuntimeErrorCode};

/// The environment variable that points at a profiles file.
pub const PROFILES_ENV: &str = "JEVSCRIPT_PROFILES";

/// The model id a run uses when it names none.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The date the bundled profiles were taken from the TypeSafe documentation.
pub const BUNDLE_DATE: &str = "2026-09-21";

/// The profiles this build ships with.
///
/// Their values come from the TypeSafe documentation for that version on
/// [`BUNDLE_DATE`]. `jev-latest` is an alias the bundle maps to a concrete
/// version, so a program that names nothing still gets a pinned set of limits.
const BUNDLED: &str = include_str!("../profiles/bundled.json");

/// How `tokens()` estimates a value's size from its profile (spec section 10.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tokenizer {
    /// Characters divided by four, rounded up.
    Chars4,
    /// One of the offline tiktoken encodings supported by the runtime.
    Tiktoken(TiktokenEncoding),
}

impl Tokenizer {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Chars4 => "chars4",
            Self::Tiktoken(encoding) => encoding.wire_name(),
        }
    }
}

impl Serialize for Tokenizer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.wire_name())
    }
}

impl<'de> Deserialize<'de> for Tokenizer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value == "chars4" {
            return Ok(Self::Chars4);
        }
        TiktokenEncoding::from_wire_name(&value)
            .map(Self::Tiktoken)
            .ok_or_else(|| {
                de::Error::custom(format!(
                    "unsupported tokenizer `{value}`; expected `chars4` or a supported `tiktoken:<encoding>`"
                ))
            })
    }
}

/// An offline tiktoken encoding supported by model profiles (spec section 10.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiktokenEncoding {
    /// The `o200k_harmony` encoding.
    O200kHarmony,
    /// The `o200k_base` encoding.
    O200kBase,
    /// The `cl100k_base` encoding.
    Cl100kBase,
    /// The `p50k_base` encoding.
    P50kBase,
    /// The `p50k_edit` encoding.
    P50kEdit,
    /// The `r50k_base` encoding.
    R50kBase,
    /// The `gpt2` encoding, which uses the `r50k_base` vocabulary.
    Gpt2,
}

impl TiktokenEncoding {
    fn from_wire_name(value: &str) -> Option<Self> {
        match value {
            "tiktoken:o200k_harmony" => Some(Self::O200kHarmony),
            "tiktoken:o200k_base" => Some(Self::O200kBase),
            "tiktoken:cl100k_base" => Some(Self::Cl100kBase),
            "tiktoken:p50k_base" => Some(Self::P50kBase),
            "tiktoken:p50k_edit" => Some(Self::P50kEdit),
            "tiktoken:r50k_base" => Some(Self::R50kBase),
            "tiktoken:gpt2" => Some(Self::Gpt2),
            _ => None,
        }
    }

    fn wire_name(self) -> &'static str {
        match self {
            Self::O200kHarmony => "tiktoken:o200k_harmony",
            Self::O200kBase => "tiktoken:o200k_base",
            Self::Cl100kBase => "tiktoken:cl100k_base",
            Self::P50kBase => "tiktoken:p50k_base",
            Self::P50kEdit => "tiktoken:p50k_edit",
            Self::R50kBase => "tiktoken:r50k_base",
            Self::Gpt2 => "tiktoken:gpt2",
        }
    }
}

/// One model's profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The model id this profile is keyed by.
    pub model: String,
    /// Where requests for this model go.
    pub endpoint: String,
    /// The limit on the state plus every question in one request (spec 6.10).
    pub total_tokens: u64,
    /// The limit on the state plus the largest single question (spec 6.10).
    pub state_plus_question_tokens: u64,
    /// How many questions may share one request, which is what splits an `each`
    /// over a long list across requests (spec section 6.5).
    pub max_questions_per_request: u32,
    /// The cap on `pick` labels and on a `pick among` list (spec section 6.4a).
    pub max_criteria_per_question: u32,
    /// The estimator `tokens()` uses.
    pub tokenizer: Tokenizer,
    /// Input price, for the `usd` budget and `usage` reporting.
    pub price_per_million_input_usd: f64,
    /// Output price. Jev does not generate text, so this is normally zero.
    pub price_per_million_output_usd: f64,
    /// For an alias such as `jev-latest`, the concrete model it resolves to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<String>,
}

impl Profile {
    /// The estimated spend for `tokens` of input at this profile's price, for
    /// the `usd` budget and `usage` reporting (spec section 10.6). Jev does not
    /// generate text, so output tokens are not priced here.
    pub fn usd_for(&self, tokens: u64) -> f64 {
        tokens as f64 / 1_000_000.0 * self.price_per_million_input_usd
    }
}

/// Every profile a run can select from.
#[derive(Debug, Clone, PartialEq)]
pub struct Profiles {
    by_model: BTreeMap<String, Profile>,
}

impl Profiles {
    /// The profiles this build bundles.
    ///
    /// # Panics
    ///
    /// Only if the bundled JSON is malformed, which is a build-time mistake.
    pub fn bundled() -> Self {
        let profiles: Vec<Profile> =
            serde_json::from_str(BUNDLED).expect("the bundled profiles are valid JSON");
        Self {
            by_model: profiles.into_iter().map(|p| (p.model.clone(), p)).collect(),
        }
    }

    /// The bundled profiles, with any from [`PROFILES_ENV`] layered over them.
    ///
    /// # Errors
    ///
    /// If the file named by the environment variable cannot be read or parsed.
    pub fn from_env() -> Result<Self, RuntimeError> {
        let mut profiles = Self::bundled();
        if let Ok(path) = std::env::var(PROFILES_ENV) {
            profiles.overlay_file(Path::new(&path))?;
        }
        Ok(profiles)
    }

    /// Layer the profiles in `path` over these, replacing any with the same id.
    ///
    /// # Errors
    ///
    /// If the file cannot be read or does not hold a list of profile records.
    pub fn overlay_file(&mut self, path: &Path) -> Result<(), RuntimeError> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::ProfileMissing,
                format!(
                    "could not read the profiles file {}: {error}",
                    path.display()
                ),
            )
        })?;
        let profiles: Vec<Profile> = serde_json::from_str(&text).map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::ProfileMissing,
                format!(
                    "could not parse the profiles file {}: {error}",
                    path.display()
                ),
            )
        })?;
        for profile in profiles {
            self.by_model.insert(profile.model.clone(), profile);
        }
        Ok(())
    }

    /// The profile for `model`, following one level of alias.
    ///
    /// # Errors
    ///
    /// [`RuntimeErrorCode::ProfileMissing`] if no profile has that id.
    pub fn resolve(&self, model: &str) -> Result<&Profile, RuntimeError> {
        let profile = self.by_model.get(model).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::ProfileMissing,
                format!("no profile for model `{model}`; set {PROFILES_ENV} or pass `profiles`"),
            )
        })?;
        match &profile.aliases {
            Some(target) => self.by_model.get(target).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::ProfileMissing,
                    format!("`{model}` points at `{target}`, which has no profile"),
                )
            }),
            None => Ok(profile),
        }
    }

    /// Every model id, including aliases.
    pub fn model_ids(&self) -> impl Iterator<Item = &str> {
        self.by_model.keys().map(String::as_str)
    }
}

impl Default for Profiles {
    fn default() -> Self {
        Self::bundled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_json(tokenizer: &str) -> String {
        format!(
            r#"[{{
                "model": "test-model",
                "endpoint": "https://example.invalid",
                "total_tokens": 100,
                "state_plus_question_tokens": 50,
                "max_questions_per_request": 4,
                "max_criteria_per_question": 8,
                "tokenizer": "{tokenizer}",
                "price_per_million_input_usd": 0.0,
                "price_per_million_output_usd": 0.0
            }}]"#
        )
    }

    #[test]
    fn the_bundle_parses_and_has_the_default_model() {
        let profiles = Profiles::bundled();
        let latest = profiles
            .resolve(DEFAULT_MODEL)
            .expect("jev-latest resolves");
        // The alias must land on a concrete version, not on itself.
        assert_ne!(latest.model, DEFAULT_MODEL);
        assert!(latest.total_tokens > 0);
    }

    #[test]
    fn prices_come_from_the_profile() {
        // Spec 10.6: `price_per_million_*` drive the `usd` budget.
        let profile = Profile {
            price_per_million_input_usd: 0.5,
            ..Profiles::bundled()
                .resolve(DEFAULT_MODEL)
                .expect("resolves")
                .clone()
        };
        assert!((profile.usd_for(2_000_000) - 1.0).abs() < 1e-12);
        assert_eq!(profile.usd_for(0), 0.0);
    }

    #[test]
    fn an_unknown_model_is_profile_missing() {
        let error = Profiles::bundled()
            .resolve("jev-imaginary")
            .expect_err("no such model");
        assert_eq!(error.code, RuntimeErrorCode::ProfileMissing);
    }

    #[test]
    fn every_bundled_profile_caps_questions_and_criteria() {
        let profiles = Profiles::bundled();
        for id in profiles.model_ids().map(str::to_string).collect::<Vec<_>>() {
            let profile = profiles.resolve(&id).expect("resolves");
            assert!(profile.max_questions_per_request > 0, "{id}");
            assert!(profile.max_criteria_per_question > 0, "{id}");
            assert!(
                profile.state_plus_question_tokens <= profile.total_tokens,
                "{id}"
            );
        }
    }

    #[test]
    fn tokenizer_wire_names_round_trip() {
        // Spec 10.6 fixes the profile spelling independently of Rust names.
        let cases = [
            ("chars4", Tokenizer::Chars4),
            (
                "tiktoken:o200k_harmony",
                Tokenizer::Tiktoken(TiktokenEncoding::O200kHarmony),
            ),
            (
                "tiktoken:o200k_base",
                Tokenizer::Tiktoken(TiktokenEncoding::O200kBase),
            ),
            (
                "tiktoken:cl100k_base",
                Tokenizer::Tiktoken(TiktokenEncoding::Cl100kBase),
            ),
            (
                "tiktoken:p50k_base",
                Tokenizer::Tiktoken(TiktokenEncoding::P50kBase),
            ),
            (
                "tiktoken:p50k_edit",
                Tokenizer::Tiktoken(TiktokenEncoding::P50kEdit),
            ),
            (
                "tiktoken:r50k_base",
                Tokenizer::Tiktoken(TiktokenEncoding::R50kBase),
            ),
            ("tiktoken:gpt2", Tokenizer::Tiktoken(TiktokenEncoding::Gpt2)),
        ];
        for (wire, tokenizer) in cases {
            let json = format!("\"{wire}\"");
            assert_eq!(serde_json::from_str::<Tokenizer>(&json).unwrap(), tokenizer);
            assert_eq!(serde_json::to_string(&tokenizer).unwrap(), json);
        }
    }

    #[test]
    fn unsupported_tokenizer_makes_an_overlay_profile_missing() {
        // Spec 10.6 and 12: invalid profiles never fall back to `chars4`.
        let path = std::env::temp_dir().join(format!(
            "jevscript-unsupported-tokenizer-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, profile_json("tiktoken:not_real")).unwrap();

        let error = Profiles::bundled()
            .overlay_file(&path)
            .expect_err("unsupported tokenizer must reject the profile");
        std::fs::remove_file(path).unwrap();

        assert_eq!(error.code, RuntimeErrorCode::ProfileMissing);
        assert!(error.message.contains("tiktoken:not_real"));
    }

    #[test]
    fn unknown_tokenizer_kind_is_rejected_by_profile_deserialization() {
        let error = serde_json::from_str::<Vec<Profile>>(&profile_json("sentencepiece:model"))
            .expect_err("unknown tokenizer kind must reject the profile");
        assert!(error.to_string().contains("sentencepiece:model"));
    }
}
