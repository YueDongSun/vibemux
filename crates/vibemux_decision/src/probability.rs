//! Fixed-point probabilities and the confidence threshold.
//!
//! Records and events keep probabilities as integer millionths, so stored
//! evidence is exact, comparable, and replayable without float drift.

use serde::{Deserialize, Serialize};

pub const MILLIONTHS_PER_UNIT: u32 = 1_000_000;
/// Lowest allowed threshold. Below it, an answer would be accepted at the
/// confidence level independent evaluations observed when no option was
/// correct (ADR 034 §8).
pub const MIN_THRESHOLD_MILLIONTHS: u32 = 500_000;

/// A value in [0, 1] stored as integer millionths.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Millionths(u32);

impl Millionths {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(MILLIONTHS_PER_UNIT);

    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if value <= MILLIONTHS_PER_UNIT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Rounds a finite value in [0, 1] to the nearest millionth.
    #[must_use]
    pub fn from_unit(value: f64) -> Option<Self> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return None;
        }
        let scaled = (value * f64::from(MILLIONTHS_PER_UNIT)).round();
        // In [0, 1_000_000] by the check above, so the cast cannot truncate.
        Self::new(scaled as u32)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn as_unit(self) -> f64 {
        f64::from(self.0) / f64::from(MILLIONTHS_PER_UNIT)
    }
}

impl TryFrom<u32> for Millionths {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("millionths out of range")
    }
}

impl From<Millionths> for u32 {
    fn from(value: Millionths) -> Self {
        value.0
    }
}

/// The confidence an answer needs before VibeMux applies it. Policy, not
/// calibration: the provider's confidence is uncalibrated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Threshold(Millionths);

impl Threshold {
    #[must_use]
    pub const fn new(value: Millionths) -> Option<Self> {
        if value.get() >= MIN_THRESHOLD_MILLIONTHS {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub fn from_unit(value: f64) -> Option<Self> {
        Millionths::from_unit(value).and_then(Self::new)
    }

    #[must_use]
    pub const fn millionths(self) -> Millionths {
        self.0
    }

    #[must_use]
    pub const fn is_met_by(self, confidence: Millionths) -> bool {
        confidence.get() >= self.0.get()
    }
}

impl TryFrom<u32> for Threshold {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Millionths::new(value)
            .and_then(Self::new)
            .ok_or("threshold out of range")
    }
}

impl From<Threshold> for u32 {
    fn from(value: Threshold) -> Self {
        value.0.get()
    }
}

/// `(p_max − 1/n) / (1 − 1/n)`, clamped to [0, 1]: 0 for a flat
/// distribution, 1 when one option holds all probability. `option_count`
/// must be at least 2.
#[must_use]
pub fn normalized_peak(p_max: f64, option_count: usize) -> f64 {
    if option_count < 2 || !p_max.is_finite() {
        return 0.0;
    }
    // Option counts are at most 255, so the conversion is exact.
    let uniform = 1.0 / option_count as f64;
    ((p_max - uniform) / (1.0 - uniform)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millionths_round_and_bound() {
        assert_eq!(Millionths::from_unit(0.0), Some(Millionths::ZERO));
        assert_eq!(Millionths::from_unit(1.0), Some(Millionths::ONE));
        assert_eq!(Millionths::from_unit(0.123_456_7).unwrap().get(), 123_457);
        for bad in [-0.000_001, 1.000_001, f64::NAN, f64::INFINITY] {
            assert_eq!(Millionths::from_unit(bad), None);
        }
        assert!(Millionths::new(MILLIONTHS_PER_UNIT + 1).is_none());
        assert!(serde_json::from_str::<Millionths>("1000001").is_err());
    }

    #[test]
    fn threshold_floor_is_one_half() {
        assert!(Threshold::from_unit(0.499_999).is_none());
        assert!(Threshold::from_unit(0.5).is_some());
        assert!(Threshold::from_unit(1.0).is_some());
        let threshold = Threshold::from_unit(0.9).unwrap();
        assert!(threshold.is_met_by(Millionths::new(900_000).unwrap()));
        assert!(!threshold.is_met_by(Millionths::new(899_999).unwrap()));
        assert!(serde_json::from_str::<Threshold>("400000").is_err());
    }

    #[test]
    fn normalized_peak_matches_the_formula() {
        assert!((normalized_peak(0.5, 2) - 0.0).abs() < 1e-12);
        assert!((normalized_peak(1.0, 4) - 1.0).abs() < 1e-12);
        let expected = (0.88 - 1.0 / 3.0) / (1.0 - 1.0 / 3.0);
        assert!((normalized_peak(0.88, 3) - expected).abs() < 1e-12);
        assert!(
            normalized_peak(0.1, 3).abs() < 1e-12,
            "below uniform clamps to 0"
        );
        assert!(normalized_peak(0.9, 1).abs() < 1e-12);
    }
}
