//! Модуль конструирования реплик и связывания бэкчанелов (builder.rs).
//!
//! Обеспечивает:
//! 1. Сборку объектов `Utterance` из сырых результатов DP-сегментации (`RawSegment`).
//! 2. Расчёт внутренних пауз (`internal_pauses`) и проекцию `boundary_candidates`.
//! 3. Санитизацию временных меток ASR (`sanitize_unit_timestamps`).
//! 4. Финализацию жизненного цикла реплик (`finalize_utterances`):
//!    - Сортировка по `(start, end)`;
//!    - Детерминированное присвоение ID (`utt_001`, `utt_002`, ...);
//!    - Поджатие смежных границ реплик одного диктора (`clamp_adjacent_utterances`);
//!    - Связывание прерванных реплик при бэкчанелах
//!      (`link_backchannels` $\to$ `cg_N`, `continues_from`).

use crate::phrase::{
    config::SegmentationConfig,
    dp::RawSegment,
    profile::LanguageProfile,
    time::Seconds,
    types::{
        AsrUnit, BoundaryCandidate, InternalPause, SegmentationError, SegmentationMeta,
        SpeakerTurn, Utterance,
    },
};

/// Санитизация временных меток ASR-юнитов.
///
/// 1. Проверяет валидность таймкодов (отсутствие NaN, Inf и отрицательных значений).
/// 2. Подтягивает start к предыдущему start для обеспечения строгой монотонности начал.
/// 3. Гарантирует end >= start для устранения инвертированных меток Whisper.
pub fn sanitize_unit_timestamps(units: &[AsrUnit]) -> Result<Vec<AsrUnit>, SegmentationError> {
    for u in units {
        let s = u.start.0;
        let e = u.end.0;
        if s.is_nan() || s.is_infinite() || s < 0.0 || e.is_nan() || e.is_infinite() || e < 0.0 {
            return Err(SegmentationError::InvalidTimestamps(format!(
                "Невалидные временные метки юнита '{}': start={}, end={}",
                u.text, s, e
            )));
        }
    }

    let mut out = units.to_vec();
    for i in 0..out.len() {
        if i > 0 && out[i].start < out[i - 1].start {
            out[i].start = out[i - 1].start;
        }
        if out[i].end < out[i].start {
            out[i].end = out[i].start;
        }
    }

    Ok(out)
}

/// Построение отдельной реплики `Utterance` из сырого сегмента DP.
pub fn build_utterance(
    turn: &SpeakerTurn,
    raw_seg: &RawSegment,
    source_language: &str,
    profile: &dyn LanguageProfile,
    cfg: &SegmentationConfig,
    all_candidates: &[BoundaryCandidate],
) -> Utterance {
    let seg_units = &turn.units[raw_seg.start_idx..raw_seg.end_idx];

    let (start, end) = if let (Some(first), Some(last)) = (seg_units.first(), seg_units.last()) {
        (first.start, last.end)
    } else {
        (Seconds::ZERO, Seconds::ZERO)
    };

    let unit_texts: Vec<&str> = seg_units.iter().map(|u| u.text.as_str()).collect();
    let text = profile.join_units(&unit_texts);

    // Вычисление внутренних пауз внутри реплики
    let mut internal_pauses = Vec::new();
    if seg_units.len() >= 2 {
        for i in 0..seg_units.len() - 1 {
            let p_start = seg_units[i].end;
            let p_end = seg_units[i + 1].start;
            if p_end > p_start {
                let dur = p_end - p_start;
                if dur >= cfg.min_internal_pause_sec {
                    internal_pauses.push(InternalPause {
                        after_unit_index: i,
                        start: p_start,
                        end: p_end,
                        duration: dur,
                    });
                }
            }
        }
    }

    // Проекция boundary_candidates, попавших внутрь реплики
    let mut boundary_candidates = Vec::new();
    for cand in all_candidates {
        if cand.after_unit_index >= raw_seg.start_idx
            && cand.after_unit_index + 1 < raw_seg.end_idx
        {
            boundary_candidates.push(BoundaryCandidate {
                after_unit_index: cand.after_unit_index - raw_seg.start_idx,
                time_offset: cand.time_offset,
                boundary_score: cand.boundary_score,
                can_split_after: cand.can_split_after,
            });
        }
    }

    let segmentation = SegmentationMeta {
        reason: raw_seg.reason,
        confidence: raw_seg.boundary_score.unwrap_or(1.0),
        is_oversize: raw_seg.is_oversize,
        is_soft_max_exceeded: raw_seg.is_soft_max_exceeded,
    };

    Utterance {
        id: String::new(), // будет присвоен детерминированно в finalize_utterances
        speaker: turn.speaker.clone(),
        source_language: source_language.to_string(),
        speaker_turn_id: Some(turn.id.clone()),
        context_group_id: None,
        continues_from: None,
        start,
        end,
        text,
        units: seg_units.to_vec(),
        internal_pauses,
        segmentation,
        overlap: turn.is_overlap,
        boundary_candidates,
        dp_cost_breakdown: Some(raw_seg.cost),
    }
}

/// Поджатие смежных границ реплик одного диктора для исключения нахлёстов.
///
/// Если из-за акустического перекрытия слов Whisper конец первой реплики превышает начало второй
/// И обе реплики принадлежат одному спикеру (`prev.speaker == next.speaker`),
/// правая граница первой реплики поджимается (`prev.end = next.start`).
///
/// При разных дикторах нахлёст физически сохраняется как естественное наложение речи.
#[inline]
pub fn clamp_adjacent_utterances(prev: &mut Utterance, next: &Utterance) {
    if prev.speaker == next.speaker && prev.end > next.start {
        prev.end = next.start;
    }
}

/// Связывание прерванных реплик при бэкчанелах (отдельный пост-DP проход).
///
/// Анализирует тройки (U_A, U_B, U_C) в хронологически отсортированном списке реплик.
pub fn link_backchannels(utterances: &mut [Utterance], profile: &dyn LanguageProfile) {
    let n = utterances.len();
    if n < 3 {
        return;
    }

    let mut next_cg_idx = 1usize;

    for i in 0..n - 2 {
        let (is_match, existing_cg) = {
            let u_a = &utterances[i];
            let u_b = &utterances[i + 1];
            let u_c = &utterances[i + 2];

            let same_speaker = u_a.speaker == u_c.speaker;
            let diff_interrupter = u_a.speaker != u_b.speaker;

            // Критерий вклинившегося диктора U_B (короткий бэкчанел)
            let u_b_dur = (u_b.end - u_b.start).as_f64();
            let u_b_dur_ok = u_b_dur <= 1.5;
            let u_b_len_ok = if profile.uses_whitespace_join() {
                u_b.text.split_whitespace().count() <= 3
            } else {
                u_b.text.chars().count() <= 6
            };

            // Критерий временного зазора между репликами диктора A
            let gap = (u_c.start - u_a.end).as_f64().max(0.0);
            let gap_ok = gap <= 2.5;

            // Лингвистический критерий незавершённости первого осколка (БЕЗ терминальной точки)
            let u_a_not_finished = !profile.ends_sentence(&u_a.text);

            // Лингвистический критерий продолжения второго осколка
            let u_c_continues = profile.starts_with_continuation(&u_c.text);

            if same_speaker
                && diff_interrupter
                && u_b_dur_ok
                && u_b_len_ok
                && gap_ok
                && u_a_not_finished
                && u_c_continues
            {
                (true, u_a.context_group_id.clone())
            } else {
                (false, None)
            }
        };

        if is_match {
            let cg = existing_cg.unwrap_or_else(|| {
                let name = format!("cg_{}", next_cg_idx);
                next_cg_idx += 1;
                name
            });

            utterances[i].context_group_id = Some(cg.clone());
            let prev_id = utterances[i].id.clone();
            utterances[i + 2].context_group_id = Some(cg);
            utterances[i + 2].continues_from = Some(prev_id);
        }
    }
}

/// Финализация жизненного цикла реплик.
///
/// 1. Сортировка по (start, end) для обеспечения строгой временной монотонности;
/// 2. Детерминированное присвоение сквозных ID (`utt_001`, `utt_002`, ...);
/// 3. Поджатие смежных границ ОДНОГО диктора (`clamp_adjacent_utterances`);
/// 4. Связывание прерванных реплик при бэкчанелах (`link_backchannels`).
pub fn finalize_utterances(
    mut utterances: Vec<Utterance>,
    profile: &dyn LanguageProfile,
    _cfg: &SegmentationConfig,
) -> Vec<Utterance> {
    if utterances.is_empty() {
        return utterances;
    }

    // 1. Каноническая сортировка по таймкодам
    utterances.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then_with(|| a.end.total_cmp(&b.end))
    });

    // 2. Детерминированное присвоение сквозных ID
    for (idx, utt) in utterances.iter_mut().enumerate() {
        utt.id = format!("utt_{:03}", idx + 1);
    }

    // 3. Поджатие смежных границ одного диктора (clamp)
    for i in 0..utterances.len().saturating_sub(1) {
        if utterances[i].speaker == utterances[i + 1].speaker
            && utterances[i].end > utterances[i + 1].start
        {
            utterances[i].end = utterances[i + 1].start;
        }
    }

    // 4. Связывание прерванных реплик (бэкчанелов)
    link_backchannels(&mut utterances, profile);

    utterances
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::{
        profile::{DefaultProfile, JapaneseProfile, RussianProfile},
        test_utils::make_aligned_unit,
        types::{AmbiguityStatus, DpCostBreakdown, SegmentationReason},
    };

    fn make_test_utterance(
        id: &str,
        speaker: &str,
        start: f64,
        end: f64,
        text: &str,
    ) -> Utterance {
        Utterance {
            id: id.to_string(),
            speaker: speaker.to_string(),
            source_language: "ru".to_string(),
            speaker_turn_id: Some("turn_001".to_string()),
            context_group_id: None,
            continues_from: None,
            start: Seconds(start),
            end: Seconds(end),
            text: text.to_string(),
            units: vec![make_aligned_unit(text, start, end, speaker)],
            internal_pauses: Vec::new(),
            segmentation: SegmentationMeta {
                reason: SegmentationReason::PunctuationPause,
                confidence: 1.0,
                is_oversize: false,
                is_soft_max_exceeded: false,
            },
            overlap: false,
            boundary_candidates: Vec::new(),
            dp_cost_breakdown: Some(DpCostBreakdown {
                segment_penalty: 2.5,
                length_penalty: 0.0,
                boundary_reward: 0.0,
                total_cost: 2.5,
            }),
        }
    }

    #[test]
    fn test_clamp_adjacent_utterances_same_speaker() {
        let mut u1 = make_test_utterance("utt_001", "S0", 1.0, 3.2, "Первая фраза");
        let u2 = make_test_utterance("utt_002", "S0", 3.0, 5.0, "Вторая фраза");

        clamp_adjacent_utterances(&mut u1, &u2);
        assert_eq!(u1.end, Seconds(3.0));
    }

    #[test]
    fn test_clamp_adjacent_utterances_different_speaker_no_clamp() {
        let mut u1 = make_test_utterance("utt_001", "S0", 1.0, 3.2, "Первая фраза");
        let u2 = make_test_utterance("utt_002", "S1", 3.0, 5.0, "Вторая фраза другого");

        clamp_adjacent_utterances(&mut u1, &u2);
        // Не поджимается, сохраняя физический overlap
        assert_eq!(u1.end, Seconds(3.2));
    }

    #[test]
    fn test_backchannel_context_grouping_success() {
        let profile = RussianProfile;
        let cfg = SegmentationConfig::default();

        let u1 = make_test_utterance("utt_001", "S0", 0.0, 3.0, "Я хотел сказать,");
        let u2 = make_test_utterance("utt_002", "S1", 3.2, 3.6, "Ага.");
        let u3 = make_test_utterance("utt_003", "S0", 4.0, 6.0, "что мы уезжаем.");

        let res = finalize_utterances(vec![u1, u2, u3], &profile, &cfg);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].context_group_id, Some("cg_1".to_string()));
        assert_eq!(res[0].continues_from, None);
        assert_eq!(res[1].context_group_id, None);
        assert_eq!(res[2].context_group_id, Some("cg_1".to_string()));
        assert_eq!(res[2].continues_from, Some("utt_001".to_string()));
    }

    #[test]
    fn test_backchannel_no_linking_when_first_has_terminal_punct() {
        let profile = RussianProfile;
        let cfg = SegmentationConfig::default();

        // U1 завершается точкой -> связывание не должно активироваться
        let u1 = make_test_utterance(
            "utt_001",
            "S0",
            10.10,
            13.00,
            "Я хотел тебе сказать, что завтра мы уезжаем.",
        );
        let u2 = make_test_utterance("utt_002", "S1", 14.00, 14.40, "Ага.");
        let u3 = make_test_utterance("utt_003", "S0", 15.20, 16.90, "И вернёмся через неделю.");

        let res = finalize_utterances(vec![u1, u2, u3], &profile, &cfg);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].context_group_id, None);
        assert_eq!(res[0].continues_from, None);
        assert_eq!(res[1].context_group_id, None);
        assert_eq!(res[2].context_group_id, None);
        assert_eq!(res[2].continues_from, None);
    }

    #[test]
    fn test_backchannel_with_japanese_continuation() {
        let profile = JapaneseProfile;
        let cfg = SegmentationConfig::default();

        let u1 = make_test_utterance("utt_001", "S0", 0.0, 2.0, "東京に行きたい");
        let u2 = make_test_utterance("utt_002", "S1", 2.2, 2.6, "うん");
        let u3 = make_test_utterance("utt_003", "S0", 3.0, 5.0, "そして買い物をする");

        let res = finalize_utterances(vec![u1, u2, u3], &profile, &cfg);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].context_group_id, Some("cg_1".to_string()));
        assert_eq!(res[2].context_group_id, Some("cg_1".to_string()));
        assert_eq!(res[2].continues_from, Some("utt_001".to_string()));
    }

    #[test]
    fn test_deterministic_id_assignment() {
        let profile = DefaultProfile;
        let cfg = SegmentationConfig::default();

        let u1 = make_test_utterance("tmp_b", "S0", 5.0, 7.0, "Second");
        let u2 = make_test_utterance("tmp_a", "S0", 1.0, 3.0, "First");

        let res = finalize_utterances(vec![u1, u2], &profile, &cfg);
        assert_eq!(res[0].id, "utt_001");
        assert_eq!(res[0].text, "First");
        assert_eq!(res[1].id, "utt_002");
        assert_eq!(res[1].text, "Second");
    }

    #[test]
    fn test_sanitize_unit_timestamps_valid() {
        let units = vec![
            AsrUnit {
                text: "a".to_string(),
                start: Seconds(1.5),
                end: Seconds(1.2), // Инвертирован
                confidence: None,
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "b".to_string(),
                start: Seconds(1.0), // Откат старта назад
                end: Seconds(2.0),
                confidence: None,
                is_whisper_boundary: false,
                can_split_after: true,
            },
        ];

        let sanitized = sanitize_unit_timestamps(&units).unwrap();
        // u0.end подтянут к u0.start (1.5)
        assert_eq!(sanitized[0].start, Seconds(1.5));
        assert_eq!(sanitized[0].end, Seconds(1.5));
        // u1.start подтянут к u0.start (1.5)
        assert_eq!(sanitized[1].start, Seconds(1.5));
        assert_eq!(sanitized[1].end, Seconds(2.0));
    }

    #[test]
    fn test_sanitize_unit_timestamps_invalid_nan_or_negative() {
        let units_nan = vec![AsrUnit {
            text: "bad".to_string(),
            start: Seconds(f64::NAN),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }];
        assert!(sanitize_unit_timestamps(&units_nan).is_err());

        let units_neg = vec![AsrUnit {
            text: "bad".to_string(),
            start: Seconds(-0.5),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }];
        assert!(sanitize_unit_timestamps(&units_neg).is_err());
    }

    #[test]
    fn test_internal_pauses_and_boundary_candidates_in_build_utterance() {
        let profile = RussianProfile;
        let cfg = SegmentationConfig::default();

        let u0 = make_aligned_unit("Слово", 1.0, 1.5, "S0");
        // пауза 0.3с >= min_internal_pause (0.1с)
        let u1 = make_aligned_unit("пауза", 1.8, 2.3, "S0");

        let turn = SpeakerTurn {
            id: "turn_001".to_string(),
            speaker: "S0".to_string(),
            start: Seconds(1.0),
            end: Seconds(2.3),
            units: vec![u0, u1],
            is_overlap: false,
            context_group_id: None,
            continues_from_turn_id: None,
        };

        let raw_seg = RawSegment {
            start_idx: 0,
            end_idx: 2,
            reason: SegmentationReason::OptimalDpSplit,
            boundary_score: Some(1.2),
            is_oversize: false,
            is_soft_max_exceeded: false,
            cost: DpCostBreakdown {
                segment_penalty: 2.5,
                length_penalty: 0.1,
                boundary_reward: 1.8,
                total_cost: 0.8,
            },
        };

        let candidates = vec![BoundaryCandidate {
            after_unit_index: 0,
            time_offset: Seconds(1.5),
            boundary_score: 1.2,
            can_split_after: true,
        }];

        let utt = build_utterance(&turn, &raw_seg, "ru", &profile, &cfg, &candidates);
        assert_eq!(utt.start, Seconds(1.0));
        assert_eq!(utt.end, Seconds(2.3));
        assert_eq!(utt.internal_pauses.len(), 1);
        assert_eq!(utt.internal_pauses[0].after_unit_index, 0);
        assert_eq!(utt.internal_pauses[0].duration, Seconds(0.3));
        assert_eq!(utt.boundary_candidates.len(), 1);
        assert_eq!(utt.boundary_candidates[0].after_unit_index, 0);
    }
}
