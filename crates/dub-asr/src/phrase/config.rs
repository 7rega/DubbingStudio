use crate::phrase::time::{Millis, Seconds};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ConfigError {
    #[error("Пороги длительности должны быть упорядочены: 0 <= min ({min:?}) < ideal ({ideal:?}) <= soft_max ({soft:?}) <= hard_max ({hard:?})")]
    InvalidDurationOrder {
        min: Seconds,
        ideal: Seconds,
        soft: Seconds,
        hard: Seconds,
    },
    #[error("Паузные пороги должны быть упорядочены: 0 <= min ({min:?}) <= soft ({soft:?}) <= strong ({strong:?})")]
    InvalidPauseOrder {
        min: Millis,
        soft: Millis,
        strong: Millis,
    },
    #[error("Значение {field} должно быть в диапазоне [0.0, 1.0], получено: {val}")]
    InvalidRatio { field: &'static str, val: f64 },
    #[error("Значение {field} не может быть отрицательным, NaN или бесконечным: {val}")]
    InvalidNumericValue { field: &'static str, val: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SegmentationConfig {
    /// Постоянный штраф за создание нового сегмента (cut tax) \lambda.
    /// Предварительный ориентир: 2.5
    pub dp_segment_penalty: f64,

    pub pause_min_ms: Millis,
    pub pause_soft_ms: Millis,
    pub pause_strong_ms: Millis,

    pub min_utterance_sec: Seconds,
    pub ideal_utterance_sec: Seconds,
    pub soft_max_utterance_sec: Seconds,
    pub hard_max_utterance_sec: Seconds,

    pub min_internal_pause_sec: Seconds,

    pub dp_weight_ideal_dev: f64,
    pub dp_weight_short_penalty: f64,
    pub dp_weight_soft_max_penalty: f64,
    pub dp_weight_hard_max_slope: f64,
    pub dp_weight_boundary_reward: f64,

    pub min_overlap_ratio: f64,
    pub unassigned_max_distance_ms: Millis,
    pub smoothing_max_unit_dur_ms: Millis,
    pub smoothing_max_gap_ms: Millis,
    pub smoothing_confidence_threshold: f64,

    /// Запас окна назад для DP (по умолчанию 5.0с)
    pub dp_window_buffer_sec: Seconds,

    pub language: Option<String>,
    pub debug_mode: bool,
}

impl Default for SegmentationConfig {
    fn default() -> Self {
        Self {
            dp_segment_penalty: 2.5,
            pause_min_ms: Millis(250.0),
            pause_soft_ms: Millis(500.0),
            pause_strong_ms: Millis(800.0),
            min_utterance_sec: Seconds(0.4),
            ideal_utterance_sec: Seconds(4.0),
            soft_max_utterance_sec: Seconds(8.0),
            hard_max_utterance_sec: Seconds(15.0),
            min_internal_pause_sec: Seconds(0.1),
            dp_weight_ideal_dev: 0.5,
            dp_weight_short_penalty: 50.0,
            dp_weight_soft_max_penalty: 2.0,
            dp_weight_hard_max_slope: 50.0,
            dp_weight_boundary_reward: 1.5,
            min_overlap_ratio: 0.5,
            unassigned_max_distance_ms: Millis(500.0),
            smoothing_max_unit_dur_ms: Millis(150.0),
            smoothing_max_gap_ms: Millis(150.0),
            smoothing_confidence_threshold: 0.65,
            dp_window_buffer_sec: Seconds(5.0),
            language: None,
            debug_mode: false,
        }
    }
}

impl SegmentationConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let check_finite_non_neg = |val: f64, field: &'static str| {
            if val.is_nan() || val.is_infinite() || val < 0.0 {
                Err(ConfigError::InvalidNumericValue { field, val })
            } else {
                Ok(())
            }
        };

        let check_ratio = |val: f64, field: &'static str| {
            if val.is_nan() || !(0.0..=1.0).contains(&val) {
                Err(ConfigError::InvalidRatio { field, val })
            } else {
                Ok(())
            }
        };

        check_finite_non_neg(self.dp_segment_penalty, "dp_segment_penalty")?;
        check_finite_non_neg(self.dp_weight_ideal_dev, "dp_weight_ideal_dev")?;
        check_finite_non_neg(self.dp_weight_short_penalty, "dp_weight_short_penalty")?;
        check_finite_non_neg(self.dp_weight_soft_max_penalty, "dp_weight_soft_max_penalty")?;
        check_finite_non_neg(self.dp_weight_hard_max_slope, "dp_weight_hard_max_slope")?;
        check_finite_non_neg(self.dp_weight_boundary_reward, "dp_weight_boundary_reward")?;

        // Проверка конечности и неотрицательности для ВСЕХ полей длительностей и пауз
        check_finite_non_neg(self.min_utterance_sec.0, "min_utterance_sec")?;
        check_finite_non_neg(self.ideal_utterance_sec.0, "ideal_utterance_sec")?;
        check_finite_non_neg(self.soft_max_utterance_sec.0, "soft_max_utterance_sec")?;
        check_finite_non_neg(self.hard_max_utterance_sec.0, "hard_max_utterance_sec")?;
        check_finite_non_neg(self.min_internal_pause_sec.0, "min_internal_pause_sec")?;
        check_finite_non_neg(self.dp_window_buffer_sec.0, "dp_window_buffer_sec")?;

        check_finite_non_neg(self.pause_min_ms.0, "pause_min_ms")?;
        check_finite_non_neg(self.pause_soft_ms.0, "pause_soft_ms")?;
        check_finite_non_neg(self.pause_strong_ms.0, "pause_strong_ms")?;
        check_finite_non_neg(
            self.unassigned_max_distance_ms.0,
            "unassigned_max_distance_ms",
        )?;
        check_finite_non_neg(
            self.smoothing_max_unit_dur_ms.0,
            "smoothing_max_unit_dur_ms",
        )?;
        check_finite_non_neg(self.smoothing_max_gap_ms.0, "smoothing_max_gap_ms")?;

        check_ratio(self.min_overlap_ratio, "min_overlap_ratio")?;
        check_ratio(
            self.smoothing_confidence_threshold,
            "smoothing_confidence_threshold",
        )?;

        if !(self.min_utterance_sec.0 >= 0.0
            && self.min_utterance_sec < self.ideal_utterance_sec
            && self.ideal_utterance_sec <= self.soft_max_utterance_sec
            && self.soft_max_utterance_sec <= self.hard_max_utterance_sec)
        {
            return Err(ConfigError::InvalidDurationOrder {
                min: self.min_utterance_sec,
                ideal: self.ideal_utterance_sec,
                soft: self.soft_max_utterance_sec,
                hard: self.hard_max_utterance_sec,
            });
        }

        if !(self.pause_min_ms.0 >= 0.0
            && self.pause_min_ms <= self.pause_soft_ms
            && self.pause_soft_ms <= self.pause_strong_ms)
        {
            return Err(ConfigError::InvalidPauseOrder {
                min: self.pause_min_ms,
                soft: self.pause_soft_ms,
                strong: self.pause_strong_ms,
            });
        }

        Ok(())
    }

    /// Эффективный порог boundary score для сплита без выигрыша по длине:
    /// При нейтральной длине (длительности частей близки к ideal_utterance_sec)
    /// сплит выгоден только если reward перекрывает налог за сегмент:
    /// Score >= \lambda / w_boundary = 2.5 / 1.5 \approx 1.67.
    /// Все дефолтные веса являются предварительными ориентирами и подлежат
    /// тонкой калибровке на реальных речевых клипах студии.
    #[inline]
    pub fn effective_split_threshold(&self) -> f64 {
        if self.dp_weight_boundary_reward > 0.0 {
            self.dp_segment_penalty / self.dp_weight_boundary_reward
        } else {
            f64::INFINITY
        }
    }

    #[inline]
    pub fn pause_min_sec(&self) -> Seconds {
        self.pause_min_ms.to_seconds()
    }

    #[inline]
    pub fn pause_soft_sec(&self) -> Seconds {
        self.pause_soft_ms.to_seconds()
    }

    #[inline]
    pub fn pause_strong_sec(&self) -> Seconds {
        self.pause_strong_ms.to_seconds()
    }

    #[inline]
    pub fn unassigned_max_distance_sec(&self) -> Seconds {
        self.unassigned_max_distance_ms.to_seconds()
    }

    #[inline]
    pub fn smoothing_max_unit_dur_sec(&self) -> Seconds {
        self.smoothing_max_unit_dur_ms.to_seconds()
    }

    #[inline]
    pub fn smoothing_max_gap_sec(&self) -> Seconds {
        self.smoothing_max_gap_ms.to_seconds()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_valid() {
        let cfg = SegmentationConfig::default();
        assert!(cfg.validate().is_ok());
        let threshold = cfg.effective_split_threshold();
        assert!((threshold - (2.5 / 1.5)).abs() < 1e-6);

        assert_eq!(cfg.pause_min_sec(), Seconds(0.25));
        assert_eq!(cfg.pause_soft_sec(), Seconds(0.50));
        assert_eq!(cfg.pause_strong_sec(), Seconds(0.80));
        assert_eq!(cfg.unassigned_max_distance_sec(), Seconds(0.50));
        assert_eq!(cfg.smoothing_max_unit_dur_sec(), Seconds(0.15));
        assert_eq!(cfg.smoothing_max_gap_sec(), Seconds(0.15));
    }

    #[test]
    fn test_validate_all_error_branches() {
        // InvalidDurationOrder: min >= ideal
        let mut cfg = SegmentationConfig::default();
        cfg.min_utterance_sec = Seconds(5.0);
        cfg.ideal_utterance_sec = Seconds(4.0);
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidDurationOrder {
                min: Seconds(5.0),
                ideal: Seconds(4.0),
                soft: Seconds(8.0),
                hard: Seconds(15.0),
            })
        );

        // InvalidDurationOrder: ideal > soft
        let mut cfg = SegmentationConfig::default();
        cfg.ideal_utterance_sec = Seconds(9.0);
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidDurationOrder {
                min: Seconds(0.4),
                ideal: Seconds(9.0),
                soft: Seconds(8.0),
                hard: Seconds(15.0),
            })
        );

        // InvalidDurationOrder: soft > hard
        let mut cfg = SegmentationConfig::default();
        cfg.soft_max_utterance_sec = Seconds(16.0);
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidDurationOrder {
                min: Seconds(0.4),
                ideal: Seconds(4.0),
                soft: Seconds(16.0),
                hard: Seconds(15.0),
            })
        );

        // InvalidPauseOrder: min > soft
        let mut cfg = SegmentationConfig::default();
        cfg.pause_min_ms = Millis(600.0);
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidPauseOrder {
                min: Millis(600.0),
                soft: Millis(500.0),
                strong: Millis(800.0),
            })
        );

        // InvalidPauseOrder: soft > strong
        let mut cfg = SegmentationConfig::default();
        cfg.pause_soft_ms = Millis(900.0);
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidPauseOrder {
                min: Millis(250.0),
                soft: Millis(900.0),
                strong: Millis(800.0),
            })
        );

        // InvalidRatio: min_overlap_ratio
        let mut cfg = SegmentationConfig::default();
        cfg.min_overlap_ratio = 1.2;
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidRatio {
                field: "min_overlap_ratio",
                val: 1.2,
            })
        );

        let mut cfg = SegmentationConfig::default();
        cfg.min_overlap_ratio = -0.1;
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidRatio {
                field: "min_overlap_ratio",
                val: -0.1,
            })
        );

        let mut cfg = SegmentationConfig::default();
        cfg.smoothing_confidence_threshold = 2.0;
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidRatio {
                field: "smoothing_confidence_threshold",
                val: 2.0,
            })
        );

        // InvalidNumericValue: negative penalty
        let mut cfg = SegmentationConfig::default();
        cfg.dp_segment_penalty = -1.0;
        assert_eq!(
            cfg.validate(),
            Err(ConfigError::InvalidNumericValue {
                field: "dp_segment_penalty",
                val: -1.0,
            })
        );

        // InvalidNumericValue: NaN weight
        let mut cfg = SegmentationConfig::default();
        cfg.dp_weight_boundary_reward = f64::NAN;
        match cfg.validate() {
            Err(ConfigError::InvalidNumericValue { field, .. }) => {
                assert_eq!(field, "dp_weight_boundary_reward");
            }
            other => panic!("expected InvalidNumericValue for NaN, got {other:?}"),
        }

        // InvalidNumericValue: infinite buffer
        let mut cfg = SegmentationConfig::default();
        cfg.dp_window_buffer_sec = Seconds(f64::INFINITY);
        match cfg.validate() {
            Err(ConfigError::InvalidNumericValue { field, .. }) => {
                assert_eq!(field, "dp_window_buffer_sec");
            }
            other => panic!("expected InvalidNumericValue for INFINITY, got {other:?}"),
        }
    }

    #[test]
    fn test_api_has_zero_target_language_awareness() {
        let cfg = SegmentationConfig::default();
        let json_value: serde_json::Value = serde_json::to_value(&cfg).unwrap();
        if let serde_json::Value::Object(map) = json_value {
            for key in map.keys() {
                assert!(
                    !key.starts_with("target"),
                    "SegmentationConfig API must NOT have any target language awareness: found {key}"
                );
                assert!(
                    !key.contains("target_language"),
                    "SegmentationConfig API must NOT have target_language: found {key}"
                );
            }
        }
    }
}
