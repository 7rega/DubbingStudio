use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Seconds(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Millis(pub f64);

impl Seconds {
    pub const ZERO: Seconds = Seconds(0.0);

    #[inline]
    pub fn to_millis(self) -> Millis {
        Millis(self.0 * 1000.0)
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        self.0
    }

    #[inline]
    pub fn max(self, other: Seconds) -> Seconds {
        Seconds(self.0.max(other.0))
    }

    #[inline]
    pub fn min(self, other: Seconds) -> Seconds {
        Seconds(self.0.min(other.0))
    }

    #[inline]
    pub fn total_cmp(self, other: Seconds) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Default for Seconds {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.0)
    }
}

impl Millis {
    pub const ZERO: Millis = Millis(0.0);

    #[inline]
    pub fn to_seconds(self) -> Seconds {
        Seconds(self.0 / 1000.0)
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        self.0
    }

    #[inline]
    pub fn max(self, other: Millis) -> Millis {
        Millis(self.0.max(other.0))
    }

    #[inline]
    pub fn min(self, other: Millis) -> Millis {
        Millis(self.0.min(other.0))
    }

    #[inline]
    pub fn total_cmp(self, other: Millis) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Default for Millis {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Millis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}ms", self.0)
    }
}

impl Add for Seconds {
    type Output = Seconds;

    #[inline]
    fn add(self, rhs: Seconds) -> Seconds {
        Seconds(self.0 + rhs.0)
    }
}

impl Sub for Seconds {
    type Output = Seconds;

    #[inline]
    fn sub(self, rhs: Seconds) -> Seconds {
        Seconds(self.0 - rhs.0)
    }
}

impl Add for Millis {
    type Output = Millis;

    #[inline]
    fn add(self, rhs: Millis) -> Millis {
        Millis(self.0 + rhs.0)
    }
}

impl Sub for Millis {
    type Output = Millis;

    #[inline]
    fn sub(self, rhs: Millis) -> Millis {
        Millis(self.0 - rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_millis_to_seconds_conversion() {
        let m = Millis(250.0);
        assert_eq!(m.to_seconds(), Seconds(0.250));

        let s = Seconds(1.5);
        assert_eq!(s.to_millis(), Millis(1500.0));

        assert_eq!(Seconds(1.0) + Seconds(2.5), Seconds(3.5));
        assert_eq!(Seconds(5.0) - Seconds(1.5), Seconds(3.5));

        assert_eq!(Millis(100.0) + Millis(250.0), Millis(350.0));
        assert_eq!(Millis(500.0) - Millis(150.0), Millis(350.0));

        assert!(Seconds(0.4) < Seconds(4.0));
        assert!(Millis(250.0) < Millis(500.0));

        assert_eq!(Seconds(1.0).max(Seconds(2.0)), Seconds(2.0));
        assert_eq!(Seconds(1.0).min(Seconds(2.0)), Seconds(1.0));
        assert_eq!(Millis(100.0).max(Millis(200.0)), Millis(200.0));
        assert_eq!(Millis(100.0).min(Millis(200.0)), Millis(100.0));

        // Serde roundtrip
        let json_s = serde_json::to_string(&Seconds(4.25)).unwrap();
        let de_s: Seconds = serde_json::from_str(&json_s).unwrap();
        assert_eq!(de_s, Seconds(4.25));

        let json_m = serde_json::to_string(&Millis(350.0)).unwrap();
        let de_m: Millis = serde_json::from_str(&json_m).unwrap();
        assert_eq!(de_m, Millis(350.0));
    }
}
