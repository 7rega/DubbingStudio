//! Модуль группировки юнитов в речевые ходы дикторов и детекции наложений речи (turns.rs).
//!
//! Обеспечивает:
//! 1. Формирование раздельных ходов `SpeakerTurn` при любой смене спикера (S0 -> S1 -> S0).
//! 2. 100% изоляцию разных дикторов (слова разных дикторов никогда не попадают в один ход).
//! 3. Детекцию акустических наложений речи (`is_overlap: bool`) на уровне юнитов,
//!    интервалов диаризации и пересекающихся по времени ходов.

use crate::phrase::{
    time::Seconds,
    types::{AlignedUnit, AmbiguityStatus, DiarizationResult, SpeakerTurn},
};

/// Построение последовательности ходов диктора (`SpeakerTurn`) из выровненных юнитов.
///
/// Каждая непрерывная последовательность юнитов одного спикера формирует отдельный `SpeakerTurn`.
/// При смене диктора ВСЕГДА создается новый ход
/// с детерминированным идентификатором (`turn_001`, ...).
pub fn build_speaker_turns(
    units: &[AlignedUnit],
    diarization: Option<&DiarizationResult>,
) -> Vec<SpeakerTurn> {
    if units.is_empty() {
        return Vec::new();
    }

    let mut turns: Vec<SpeakerTurn> = Vec::new();
    let mut current_units: Vec<AlignedUnit> = Vec::new();
    let mut current_speaker = units[0].speaker.clone();
    let mut turn_counter = 1usize;

    for u in units {
        if u.speaker != current_speaker {
            // Завершаем текущий ход
            if !current_units.is_empty() {
                turns.push(create_turn(
                    turn_counter,
                    current_speaker.clone(),
                    std::mem::take(&mut current_units),
                ));
                turn_counter += 1;
            }
            current_speaker = u.speaker.clone();
        }
        current_units.push(u.clone());
    }

    // Сохраняем последний ход
    if !current_units.is_empty() {
        turns.push(create_turn(turn_counter, current_speaker, current_units));
    }

    // Детекция наложений речи
    detect_overlaps(&mut turns, diarization);

    turns
}

/// Вспомогательное создание объекта `SpeakerTurn` из накопленных юнитов.
fn create_turn(index: usize, speaker: String, units: Vec<AlignedUnit>) -> SpeakerTurn {
    let start = units.first().map(|u| u.start).unwrap_or(Seconds::ZERO);
    let end = units
        .iter()
        .map(|u| u.end)
        .fold(Seconds::ZERO, |acc, x| acc.max(x));

    SpeakerTurn {
        id: format!("turn_{:03}", index),
        speaker,
        start,
        end,
        units,
        is_overlap: false,
        context_group_id: None,
        continues_from_turn_id: None,
    }
}

/// Детекция наложений речи (пересечений интервалов разных дикторов).
///
/// Выставляет флаг `is_overlap = true` на ходах диктора, если:
/// 1. Хотя бы один юнит хода имеет статус `MultipleSpeakersOverlap` или `alternative_speaker`.
/// 2. Внешний интервал диаризации другого диктора перекрывает временной отрезок хода.
/// 3. Два хода разных дикторов акустически пересекаются во времени.
pub fn detect_overlaps(turns: &mut [SpeakerTurn], diarization: Option<&DiarizationResult>) {
    if turns.is_empty() {
        return;
    }

    // 1. Проверка на уровне юнитов
    for turn in turns.iter_mut() {
        let has_unit_overlap = turn.units.iter().any(|u| {
            u.ambiguity == AmbiguityStatus::MultipleSpeakersOverlap
                || u.alternative_speaker.is_some()
        });
        if has_unit_overlap {
            turn.is_overlap = true;
        }
    }

    // 2. Проверка перекрытия с внешними интервалами диаризации других дикторов
    if let Some(diar) = diarization {
        for turn in turns.iter_mut() {
            if turn.is_overlap {
                continue;
            }
            for interval in &diar.intervals {
                if interval.speaker != turn.speaker {
                    let o_start = turn.start.max(interval.start);
                    let o_end = turn.end.min(interval.end);
                    if o_start < o_end && (o_end - o_start).as_f64() > 0.001 {
                        turn.is_overlap = true;
                        break;
                    }
                }
            }
        }
    }

    // 3. Проверка межходовых акустических пересечений разных дикторов
    let n = turns.len();
    for i in 0..n {
        for j in (i + 1)..n {
            if turns[i].speaker != turns[j].speaker {
                let o_start = turns[i].start.max(turns[j].start);
                let o_end = turns[i].end.min(turns[j].end);
                if o_start < o_end && (o_end - o_start).as_f64() > 0.001 {
                    turns[i].is_overlap = true;
                    turns[j].is_overlap = true;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Модульные тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::types::DiarInterval;

    fn make_aligned_unit(text: &str, speaker: &str, start: f64, end: f64) -> AlignedUnit {
        AlignedUnit {
            text: text.to_string(),
            start: Seconds(start),
            end: Seconds(end),
            speaker: speaker.to_string(),
            word_confidence: Some(0.95),
            speaker_confidence: 0.90,
            overlap_ratio: 1.0,
            ambiguity: AmbiguityStatus::Clear,
            alternative_speaker: None,
            alternative_overlap_ratio: 0.0,
            is_whisper_boundary: false,
            can_split_after: true,
        }
    }

    #[test]
    fn test_empty_units() {
        let turns = build_speaker_turns(&[], None);
        assert!(turns.is_empty());
    }

    #[test]
    fn test_single_speaker_turn() {
        let units = vec![
            make_aligned_unit("Привет", "S0", 0.0, 1.0),
            make_aligned_unit("мир", "S0", 1.1, 2.0),
        ];
        let turns = build_speaker_turns(&units, None);

        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].id, "turn_001");
        assert_eq!(turns[0].speaker, "S0");
        assert_eq!(turns[0].start, Seconds(0.0));
        assert_eq!(turns[0].end, Seconds(2.0));
        assert_eq!(turns[0].units.len(), 2);
        assert!(!turns[0].is_overlap);
    }

    #[test]
    fn test_speaker_change_isolation() {
        // S0 -> S1 -> S0
        let units = vec![
            make_aligned_unit("Первый", "S0", 0.0, 1.0),
            make_aligned_unit("Второй", "S1", 1.2, 2.0),
            make_aligned_unit("Третий", "S0", 2.2, 3.0),
        ];
        let turns = build_speaker_turns(&units, None);

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].id, "turn_001");
        assert_eq!(turns[0].speaker, "S0");
        assert_eq!(turns[0].units.len(), 1);

        assert_eq!(turns[1].id, "turn_002");
        assert_eq!(turns[1].speaker, "S1");
        assert_eq!(turns[1].units.len(), 1);

        assert_eq!(turns[2].id, "turn_003");
        assert_eq!(turns[2].speaker, "S0");
        assert_eq!(turns[2].units.len(), 1);
    }

    #[test]
    fn test_single_short_unit_speaker_change() {
        // Тест 11 плана: одиночная короткая реплика диктора B «Да.» (0.3с)
        // сохраняется в отдельный ход, смена спикера — безусловная граница.
        let units = vec![
            make_aligned_unit("Я хотел сказать,", "S0", 0.0, 2.0),
            make_aligned_unit("Да.", "S1", 2.1, 2.4), // 0.3с
            make_aligned_unit("что мы уезжаем.", "S0", 2.5, 4.0),
        ];
        let turns = build_speaker_turns(&units, None);

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[1].speaker, "S1");
        assert_eq!(turns[1].units.len(), 1);
        assert_eq!(turns[1].units[0].text, "Да.");
        assert_eq!(turns[1].start, Seconds(2.1));
        assert_eq!(turns[1].end, Seconds(2.4));
        assert!((turns[1].end - turns[1].start).as_f64() - 0.3 < 1e-4);
    }

    #[test]
    fn test_overlap_detection_via_unit_ambiguity() {
        let mut u = make_aligned_unit("Слово", "S0", 0.0, 1.0);
        u.ambiguity = AmbiguityStatus::MultipleSpeakersOverlap;
        u.alternative_speaker = Some("S1".to_string());
        u.alternative_overlap_ratio = 0.4;

        let turns = build_speaker_turns(&[u], None);
        assert_eq!(turns.len(), 1);
        assert!(turns[0].is_overlap);
    }

    #[test]
    fn test_overlap_detection_via_diarization() {
        let units = vec![make_aligned_unit("Речь", "S0", 1.0, 3.0)];
        let diarization = DiarizationResult {
            intervals: vec![DiarInterval {
                speaker: "S1".to_string(),
                start: Seconds(2.0),
                end: Seconds(2.5),
                confidence: None,
            }],
        };
        let turns = build_speaker_turns(&units, Some(&diarization));

        assert_eq!(turns.len(), 1);
        assert!(turns[0].is_overlap);
    }

    #[test]
    fn test_overlap_detection_via_inter_turn() {
        // Два хода разных спикеров пересекаются по времени
        // S0: [0.0, 2.5]
        // S1: [2.0, 3.5]
        let mut turn0 = create_turn(
            1,
            "S0".to_string(),
            vec![make_aligned_unit("А", "S0", 0.0, 2.5)],
        );
        let mut turn1 = create_turn(
            2,
            "S1".to_string(),
            vec![make_aligned_unit("Б", "S1", 2.0, 3.5)],
        );
        let mut turns = vec![turn0, turn1];

        detect_overlaps(&mut turns, None);
        assert!(turns[0].is_overlap);
        assert!(turns[1].is_overlap);
    }

    #[test]
    fn test_no_overlap_clean() {
        let units = vec![
            make_aligned_unit("Первый", "S0", 0.0, 1.0),
            make_aligned_unit("Второй", "S1", 1.5, 2.5),
        ];
        let diarization = DiarizationResult {
            intervals: vec![
                DiarInterval {
                    speaker: "S0".to_string(),
                    start: Seconds(0.0),
                    end: Seconds(1.0),
                    confidence: None,
                },
                DiarInterval {
                    speaker: "S1".to_string(),
                    start: Seconds(1.5),
                    end: Seconds(2.5),
                    confidence: None,
                },
            ],
        };
        let turns = build_speaker_turns(&units, Some(&diarization));

        assert_eq!(turns.len(), 2);
        assert!(!turns[0].is_overlap);
        assert!(!turns[1].is_overlap);
    }
}
