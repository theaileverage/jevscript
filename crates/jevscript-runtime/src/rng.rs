//! The run's random source (spec sections 5.7, 6.11 and 10.3).
//!
//! `random()` and every sampling draw come from one generator per run. It is
//! seeded from the SDK option `sample: { seed }` when the host gives one, and
//! from the clock otherwise. Either way each draw is written to the recording
//! as a `draw` event, and **replay never calls this generator**: the recorded
//! value is used instead, which is what makes a sampled run replay exactly
//! (spec section 10.4).
//!
//! The generator is xoshiro256** seeded through splitmix64, written here so
//! that the sequence a seed produces is fixed by this crate and not by a
//! dependency's version.

use crate::rpc::Sample;

/// A small deterministic generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    /// A generator that will produce the same sequence for the same seed.
    pub fn seeded(seed: u64) -> Self {
        let mut sm = seed;
        let mut next = || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    /// A generator seeded from the clock, for a run that asked to sample
    /// without naming a seed. The draws are still recorded, so the run still
    /// replays.
    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self::seeded(nanos ^ u64::from(std::process::id()).rotate_left(32))
    }

    /// The generator a run's `sample` option asks for (spec section 6.11).
    pub fn for_sample(sample: Option<&Sample>) -> Self {
        match sample {
            Some(Sample::Seeded { seed }) => Self::seeded(*seed),
            _ => Self::from_clock(),
        }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// A number in `[0, 1)`, which is what `random()` returns (spec 5.7).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        // Spec 6.11: `sample: { seed }` reproduces a sampled run without a
        // recording.
        let mut a = Rng::seeded(42);
        let mut b = Rng::seeded(42);
        let xs: Vec<f64> = (0..8).map(|_| a.next_f64()).collect();
        let ys: Vec<f64> = (0..8).map(|_| b.next_f64()).collect();
        assert_eq!(xs, ys);
        let mut c = Rng::seeded(43);
        assert_ne!(xs[0], c.next_f64());
    }

    #[test]
    fn draws_are_in_the_half_open_unit_interval() {
        // Spec 5.7: `random()` is in `[0, 1)`.
        let mut rng = Rng::seeded(7);
        for _ in 0..10_000 {
            let x = rng.next_f64();
            assert!((0.0..1.0).contains(&x), "{x}");
        }
    }

    #[test]
    fn a_seeded_sample_option_is_deterministic_and_an_unseeded_one_is_not_required_to_be() {
        let seeded = Sample::Seeded { seed: 9 };
        assert_eq!(
            Rng::for_sample(Some(&seeded)),
            Rng::for_sample(Some(&seeded))
        );
        // Just check it produces a value; the clock seed is not comparable.
        let _ = Rng::for_sample(Some(&Sample::On(true))).next_f64();
    }
}
