//! Модуль привязки спикеров, обработки дыр диаризации и сглаживания артефактов (aligner.rs).
//!
//! Обеспечивает:
//! 1. Перекрытие ASR-юнитов с интервалами диаризации и расчёт `speaker_confidence`.
//! 2. Вычисление статусов амбивалентности (`AmbiguityStatus`) с приоритетом
//!    `MultipleSpeakersOverlap` над `LowOverlapRatio`.
//! 3. Восстановление пропусков (дыр) диаризации через поиск ближайшего интервала
//!    (в окне 500мс) либо наследование от левого уверенного соседа.
//! 4. Консервативное сглаживание сомнительных коротких шумовых выбросов (<= 150мс)
//!    с защитой от zero-duration и безусловным сохранением чистой уверенной речи.

use crate::phrase::{
    config::SegmentationConfig,
    profile::LanguageProfile,
    time::Seconds,
    types::{AlignedUnit, AmbiguityStatus, AsrUnit, DiarInterval, DiarizationResult},
};
use std::collections::HashMap;

/// Вычисление кратчайшего расстояния между временным интервалом юнита и интервалом диаризации.
#[inline]
fn distance_to_interval(u_start: Seconds, u_end: Seconds, interval: &DiarInterval) -> f64 {
    if interval.end < u_start {
        (u_start - interval.end).as_f64()
    } else if interval.start > u_end {
        (interval.start - u_end).as_f64()
    } else {
        0.0f64
    }
}

/// Привязка ASR-юнитов к интервалам диаризации.
///
/// Обрабатывает перекрытия, дыры диаризации и проставляет статусы `AmbiguityStatus`.
pub fn align_units(
    units: &[AsrUnit],
    diarization: &DiarizationResult,
    cfg: &SegmentationConfig,
) -> Vec<AlignedUnit> {
    let mut aligned = Vec::with_capacity(units.len());
    let max_distance = cfg.unassigned_max_distance_sec().as_f64();

    for u in units {
        let dur = (u.end - u.start).as_f64();

        // 1. Поиск перекрытий с интервалами диаризации (только при положительной длительности)
        let mut speaker_overlaps: HashMap<String, f64> = HashMap::new();
        if dur > 0.001 {
            for interval in &diarization.intervals {
                let o_start = u.start.max(interval.start);
                let o_end = u.end.min(interval.end);
                if o_start < o_end {
                    let o_sec = (o_end - o_start).as_f64();
                    *speaker_overlaps
                        .entry(interval.speaker.clone())
                        .or_insert(0.0) += o_sec;
                }
            }
        }

        let mut sorted_speakers: Vec<(String, f64)> = speaker_overlaps.into_iter().collect();
        // Стабильная сортировка по убыванию перекрытия
        sorted_speakers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Случай A: Есть перекрытие с диаризацией
        if !sorted_speakers.is_empty() && sorted_speakers[0].1 > 0.0 {
            let primary_speaker = sorted_speakers[0].0.clone();
            let primary_overlap = sorted_speakers[0].1.min(dur);
            let overlap_ratio = (primary_overlap / dur).clamp(0.0, 1.0);

            let (alt_speaker, secondary_overlap, secondary_ratio) =
                if sorted_speakers.len() > 1 && sorted_speakers[1].1 > 0.0 {
                    let sec_spk = sorted_speakers[1].0.clone();
                    let sec_overlap = sorted_speakers[1].1.min(dur);
                    let sec_ratio = (sec_overlap / dur).clamp(0.0, 1.0);
                    (Some(sec_spk), sec_overlap, sec_ratio)
                } else {
                    (None, 0.0f64, 0.0f64)
                };

            let margin = (overlap_ratio - secondary_ratio).max(0.0);
            let base_confidence = (overlap_ratio * (0.5 + 0.5 * margin)).clamp(0.0, 1.0);

            let (ambiguity, speaker_confidence) =
                if secondary_ratio >= 0.20 && sorted_speakers.len() >= 2 {
                    // Приоритет MultipleSpeakersOverlap над LowOverlapRatio
                    (AmbiguityStatus::MultipleSpeakersOverlap, base_confidence)
                } else if overlap_ratio < cfg.min_overlap_ratio {
                    (AmbiguityStatus::LowOverlapRatio, base_confidence * 0.6)
                } else {
                    (AmbiguityStatus::Clear, base_confidence)
                };

            aligned.push(AlignedUnit {
                text: u.text.clone(),
                start: u.start,
                end: u.end,
                speaker: primary_speaker,
                word_confidence: u.confidence,
                speaker_confidence,
                overlap_ratio,
                ambiguity,
                alternative_speaker: if secondary_overlap > 0.0 {
                    alt_speaker
                } else {
                    None
                },
                alternative_overlap_ratio: secondary_ratio,
                is_whisper_boundary: u.is_whisper_boundary,
                can_split_after: u.can_split_after,
            });
            continue;
        }

        // Случай B: Дыра диаризации (overlap == 0 либо dur <= 0.001)
        // 1. Ищем ближайший интервал диаризации в окне unassigned_max_distance_sec
        let mut nearest: Option<(&DiarInterval, f64)> = None;
        for interval in &diarization.intervals {
            let dist = distance_to_interval(u.start, u.end, interval);
            match nearest {
                None => nearest = Some((interval, dist)),
                Some((_, best_d)) if dist < best_d => nearest = Some((interval, dist)),
                _ => {}
            }
        }

        if let Some((best_interval, dist)) = nearest {
            if dist <= max_distance {
                let conf = 0.5 * (-dist / 0.25).exp();
                aligned.push(AlignedUnit {
                    text: u.text.clone(),
                    start: u.start,
                    end: u.end,
                    speaker: best_interval.speaker.clone(),
                    word_confidence: u.confidence,
                    speaker_confidence: conf.clamp(0.0, 1.0),
                    overlap_ratio: 0.0,
                    ambiguity: AmbiguityStatus::GapNearestAssigned,
                    alternative_speaker: None,
                    alternative_overlap_ratio: 0.0,
                    is_whisper_boundary: u.is_whisper_boundary,
                    can_split_after: u.can_split_after,
                });
                continue;
            }
        }

        // 2. Наследование от левого уверенного соседа (confidence >= 0.70)
        let left_neighbor = aligned.last();
        if let Some(prev) = left_neighbor {
            if prev.speaker_confidence >= 0.70 && prev.speaker != "SPEAKER_UNKNOWN" {
                aligned.push(AlignedUnit {
                    text: u.text.clone(),
                    start: u.start,
                    end: u.end,
                    speaker: prev.speaker.clone(),
                    word_confidence: u.confidence,
                    speaker_confidence: 0.4,
                    overlap_ratio: 0.0,
                    ambiguity: AmbiguityStatus::GapInherited,
                    alternative_speaker: None,
                    alternative_overlap_ratio: 0.0,
                    is_whisper_boundary: u.is_whisper_boundary,
                    can_split_after: u.can_split_after,
                });
                continue;
            }
        }

        // 3. Fallback: неизвестный спикер
        aligned.push(AlignedUnit {
            text: u.text.clone(),
            start: u.start,
            end: u.end,
            speaker: "SPEAKER_UNKNOWN".to_string(),
            word_confidence: u.confidence,
            speaker_confidence: 0.0,
            overlap_ratio: 0.0,
            ambiguity: AmbiguityStatus::UnknownSpeaker,
            alternative_speaker: None,
            alternative_overlap_ratio: 0.0,
            is_whisper_boundary: u.is_whisper_boundary,
            can_split_after: u.can_split_after,
        });
    }

    aligned
}

/// Консервативное сглаживание артефактов привязки спикеров.
///
/// Устраняет короткие сомнительные выбросы между репликами одного и того же спикера.
/// Защищено от искажения чистой уверенной речи (Clear >= 0.65) и смены спикеров.
pub fn smooth_aligned_units(
    units: &mut [AlignedUnit],
    cfg: &SegmentationConfig,
    profile: &dyn LanguageProfile,
) {
    if units.len() < 3 {
        return;
    }

    let max_dur = cfg.smoothing_max_unit_dur_sec().as_f64();
    let max_gap = cfg.smoothing_max_gap_sec().as_f64();

    for i in 1..units.len() - 1 {
        let (left_speaker, left_end, left_ends_sentence) = {
            let u_a1 = &units[i - 1];
            (
                u_a1.speaker.clone(),
                u_a1.end,
                profile.ends_sentence(&u_a1.text),
            )
        };
        let (right_speaker, right_start) = {
            let u_a2 = &units[i + 1];
            (u_a2.speaker.clone(), u_a2.start)
        };

        let u_b = &mut units[i];
        let dur = (u_b.end - u_b.start).as_f64();
        let gap_prev = (u_b.start - left_end).as_f64().max(0.0);
        let gap_next = (right_start - u_b.end).as_f64().max(0.0);

        // Защита от мусорных/нулевых таймкодов:
        // Применяется ТОЛЬКО если левый и правый соседи принадлежат одному диктору.
        // При смене дикторов (left_speaker != right_speaker) нулевой юнит не ломает границу.
        if dur <= 0.001 {
            if left_speaker == right_speaker {
                u_b.speaker = left_speaker;
            }
            continue;
        }

        let is_ambiguous_type = matches!(
            u_b.ambiguity,
            AmbiguityStatus::LowOverlapRatio
                | AmbiguityStatus::MultipleSpeakersOverlap
                | AmbiguityStatus::GapNearestAssigned
                | AmbiguityStatus::GapInherited
                | AmbiguityStatus::UnknownSpeaker
        );

        let can_smooth = left_speaker == right_speaker
            && u_b.speaker != left_speaker
            && dur <= max_dur
            && gap_prev <= max_gap
            && gap_next <= max_gap
            && u_b.speaker_confidence < cfg.smoothing_confidence_threshold
            && !profile.ends_sentence(&u_b.text)
            && !left_ends_sentence
            && is_ambiguous_type;

        if can_smooth {
            u_b.speaker = left_speaker;
        }
    }
}

/// Полный пайплайн привязки спикеров и сглаживания.
pub fn align_and_smooth(
    units: &[AsrUnit],
    diarization: &DiarizationResult,
    cfg: &SegmentationConfig,
    profile: &dyn LanguageProfile,
) -> Vec<AlignedUnit> {
    let mut aligned = align_units(units, diarization, cfg);
    smooth_aligned_units(&mut aligned, cfg, profile);
    aligned
}

// ---------------------------------------------------------------------------
// Модульные тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::profile::RussianProfile;

    fn make_unit(text: &str, start: f64, end: f64) -> AsrUnit {
        AsrUnit {
            text: text.to_string(),
            start: Seconds(start),
            end: Seconds(end),
            confidence: Some(0.95),
            is_whisper_boundary: false,
            can_split_after: true,
        }
    }

    #[test]
    fn test_align_clear_speaker() {
        let units = vec![make_unit("Привет", 0.0, 1.0)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S0".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.5),
                confidence: Some(1.0),
            }],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(aligned[0].ambiguity, AmbiguityStatus::Clear);
        assert!((aligned[0].overlap_ratio - 1.0).abs() < 1e-4);
        assert!((aligned[0].speaker_confidence - 1.0).abs() < 1e-4);
        assert_eq!(aligned[0].alternative_speaker, None);
    }

    #[test]
    fn test_align_multiple_speakers_overlap() {
        // Юнит 1.0с: S0 перекрывает 0.6с, S1 перекрывает 0.4с (>= 0.20)
        let units = vec![make_unit("Пересечение", 0.0, 1.0)];
        let diarization = DiarizationResult {
            intervals: vec![
                DiarInterval {
                    speaker: "S0".to_string(),
                    start: Seconds(0.0),
                    end: Seconds(0.6),
                    confidence: None,
                },
                DiarInterval {
                    speaker: "S1".to_string(),
                    start: Seconds(0.6),
                    end: Seconds(1.0),
                    confidence: None,
                },
            ],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(
            aligned[0].ambiguity,
            AmbiguityStatus::MultipleSpeakersOverlap
        );
        assert_eq!(aligned[0].alternative_speaker, Some("S1".to_string()));
        assert!((aligned[0].alternative_overlap_ratio - 0.4).abs() < 1e-4);
        // margin = 0.6 - 0.4 = 0.2
        // base_conf = 0.6 * (0.5 + 0.5 * 0.2) = 0.6 * 0.6 = 0.36
        assert!((aligned[0].speaker_confidence - 0.36).abs() < 1e-4);
    }

    #[test]
    fn test_multiple_speakers_priority_over_low_overlap() {
        // primary 0.40 (< 0.50), secondary 0.25 (>= 0.20)
        // MultipleSpeakersOverlap должен иметь приоритет над LowOverlapRatio
        let units = vec![make_unit("Тест", 0.0, 1.0)];
        let diarization = DiarizationResult {
            intervals: vec![
                DiarInterval {
                    speaker: "S0".to_string(),
                    start: Seconds(0.0),
                    end: Seconds(0.4),
                    confidence: None,
                },
                DiarInterval {
                    speaker: "S1".to_string(),
                    start: Seconds(0.4),
                    end: Seconds(0.65),
                    confidence: None,
                },
            ],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(
            aligned[0].ambiguity,
            AmbiguityStatus::MultipleSpeakersOverlap
        );
        assert_eq!(aligned[0].alternative_speaker, Some("S1".to_string()));
        assert!((aligned[0].alternative_overlap_ratio - 0.25).abs() < 1e-4);
    }

    #[test]
    fn test_align_low_overlap_ratio() {
        // primary 0.40 (< 0.50), secondary 0.0
        let units = vec![make_unit("Низкое", 0.0, 1.0)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S0".to_string(),
                start: Seconds(0.0),
                end: Seconds(0.4),
                confidence: None,
            }],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(aligned[0].ambiguity, AmbiguityStatus::LowOverlapRatio);
        // margin = 0.4, base_conf = 0.4 * (0.5 + 0.5 * 0.4) = 0.28
        // speaker_confidence = 0.28 * 0.6 = 0.168
        assert!((aligned[0].speaker_confidence - 0.168).abs() < 1e-4);
    }

    #[test]
    fn test_align_gap_nearest_assigned() {
        // Юнит [1.0, 1.5], интервал [0.0, 0.9] -> расстояние d = 0.1с <= 0.5с
        let units = vec![make_unit("Дыра", 1.0, 1.5)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S0".to_string(),
                start: Seconds(0.0),
                end: Seconds(0.9),
                confidence: None,
            }],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(aligned[0].ambiguity, AmbiguityStatus::GapNearestAssigned);
        let expected_conf = 0.5 * (-0.1f64 / 0.25).exp();
        assert!((aligned[0].speaker_confidence - expected_conf).abs() < 1e-4);
    }

    #[test]
    fn test_align_gap_inherited() {
        // Юнит 0: Clear (1.0с)
        // Юнит 1: [2.0, 2.5], интервал на расстоянии > 0.5с -> наследует от левого
        let units = vec![make_unit("Первый", 0.0, 1.0), make_unit("Второй", 2.0, 2.5)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S0".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                confidence: None,
            }],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 2);
        assert_eq!(aligned[0].speaker, "S0");
        assert_eq!(aligned[0].ambiguity, AmbiguityStatus::Clear);

        assert_eq!(aligned[1].speaker, "S0");
        assert_eq!(aligned[1].ambiguity, AmbiguityStatus::GapInherited);
        assert!((aligned[1].speaker_confidence - 0.4).abs() < 1e-4);
    }

    #[test]
    fn test_align_unknown_speaker() {
        // Юнит без перекрытия и без интервалов поблизости
        let units = vec![make_unit("Одинокий", 10.0, 11.0)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S0".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                confidence: None,
            }],
        };
        let cfg = SegmentationConfig::default();
        let aligned = align_units(&units, &diarization, &cfg);

        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].speaker, "SPEAKER_UNKNOWN");
        assert_eq!(aligned[0].ambiguity, AmbiguityStatus::UnknownSpeaker);
        assert!((aligned[0].speaker_confidence - 0.0).abs() < 1e-4);
    }

    #[test]
    fn test_smoothing_ambiguous_short_noise_vs_confident_word() {
        let profile = RussianProfile;
        let cfg = SegmentationConfig::default();

        // 1. Шум 80мс со статусом LowOverlapRatio и confidence 0.40 -> сглаживается в S0
        let mut units_noise = vec![
            AlignedUnit {
                text: "Привет".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "кхм".to_string(),
                start: Seconds(1.05),
                end: Seconds(1.13), // 80 мс
                speaker: "S1".to_string(),
                word_confidence: Some(0.5),
                speaker_confidence: 0.40,
                overlap_ratio: 0.35,
                ambiguity: AmbiguityStatus::LowOverlapRatio,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "продолжаем".to_string(),
                start: Seconds(1.20),
                end: Seconds(2.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
        ];

        smooth_aligned_units(&mut units_noise, &cfg, &profile);
        assert_eq!(
            units_noise[1].speaker, "S0",
            "Шумовой юнит должен быть сглажен в S0"
        );

        // 2. Короткое слово «Да.» 250мс с уверенностью 0.90 и статусом Clear -> НЕ сглаживается!
        let mut units_confident = vec![
            AlignedUnit {
                text: "Привет".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "Да.".to_string(),
                start: Seconds(1.05),
                end: Seconds(1.30), // 250 мс
                speaker: "S1".to_string(),
                word_confidence: Some(0.95),
                speaker_confidence: 0.90,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "продолжаем".to_string(),
                start: Seconds(1.35),
                end: Seconds(2.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
        ];

        smooth_aligned_units(&mut units_confident, &cfg, &profile);
        assert_eq!(
            units_confident[1].speaker, "S1",
            "Уверенное слово 'Да.' со статусом Clear строго НЕ должно сглаживаться!"
        );
    }

    #[test]
    fn test_aligner_zero_duration_guard() {
        let profile = RussianProfile;
        let cfg = SegmentationConfig::default();

        // Сценарий 1: u_a1.speaker == u_a2.speaker (S0 == S0)
        // Нулевой юнит должен сгладиться в S0
        let mut same_speaker_units = vec![
            AlignedUnit {
                text: "Слово1".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "ноль".to_string(),
                start: Seconds(1.0),
                end: Seconds(1.0), // 0 мс
                speaker: "S1".to_string(),
                word_confidence: Some(0.5),
                speaker_confidence: 0.5,
                overlap_ratio: 0.0,
                ambiguity: AmbiguityStatus::GapNearestAssigned,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "Слово2".to_string(),
                start: Seconds(1.0),
                end: Seconds(2.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
        ];

        smooth_aligned_units(&mut same_speaker_units, &cfg, &profile);
        assert_eq!(
            same_speaker_units[1].speaker, "S0",
            "Нулевой юнит между одинаковыми спикерами должен сгладиться в S0"
        );

        // Сценарий 2: u_a1.speaker != u_a2.speaker (S0 != S1)
        // Нулевой юнит НЕ должен сглаживаться и не должен ломать границу смены спикера
        let mut diff_speaker_units = vec![
            AlignedUnit {
                text: "Слово1".to_string(),
                start: Seconds(0.0),
                end: Seconds(1.0),
                speaker: "S0".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "ноль".to_string(),
                start: Seconds(1.0),
                end: Seconds(1.0), // 0 мс
                speaker: "S1".to_string(),
                word_confidence: Some(0.5),
                speaker_confidence: 0.5,
                overlap_ratio: 0.0,
                ambiguity: AmbiguityStatus::GapNearestAssigned,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AlignedUnit {
                text: "Слово2".to_string(),
                start: Seconds(1.0),
                end: Seconds(2.0),
                speaker: "S1".to_string(),
                word_confidence: Some(0.9),
                speaker_confidence: 1.0,
                overlap_ratio: 1.0,
                ambiguity: AmbiguityStatus::Clear,
                alternative_speaker: None,
                alternative_overlap_ratio: 0.0,
                is_whisper_boundary: false,
                can_split_after: true,
            },
        ];

        smooth_aligned_units(&mut diff_speaker_units, &cfg, &profile);
        assert_eq!(
            diff_speaker_units[1].speaker, "S1",
            "Нулевой юнит при смене спикеров не должен ломать физическую границу"
        );
    }
}
