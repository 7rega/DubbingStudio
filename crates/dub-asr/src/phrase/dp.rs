use crate::phrase::{
    config::SegmentationConfig,
    profile::LanguageProfile,
    time::Seconds,
    types::{
        AlignedUnit, BoundaryCandidate, DpCostBreakdown, SegmentationReason, SpeakerTurn,
        SpeechSpan,
    },
};

/// Сырой результат DP-сегментации одного сегмента хода.
///
/// Не содержит сквозных идентификаторов `Utterance.id` и полей связывания бэкчанелов,
/// которые присваиваются на этапе post-DP обработки в `builder.rs` (PR 6).
#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    /// 0-based индекс начального юнита (включительно) в рамках хода `SpeakerTurn`.
    pub start_idx: usize,
    /// 0-based индекс конечного юнита (исключительно) в рамках хода `SpeakerTurn`.
    pub end_idx: usize,
    /// Причина границы, завершающей данный сегмент.
    pub reason: SegmentationReason,
    /// Значение BoundaryScore на границе сегмента (None для последнего сегмента хода).
    pub boundary_score: Option<f64>,
    /// Флаг превышения жесткого лимита длительности (`hard_max_utterance_sec`).
    pub is_oversize: bool,
    /// Флаг превышения мягкого порога длительности (`soft_max_utterance_sec`).
    pub is_soft_max_exceeded: bool,
    /// Детальная разбивка компонентов стоимости DP.
    pub cost: DpCostBreakdown,
}

/// Результат выполнения алгоритма динамического программирования для хода диктора.
#[derive(Debug, Clone, PartialEq)]
pub struct DpResult {
    pub segments: Vec<RawSegment>,
    pub boundary_candidates: Vec<BoundaryCandidate>,
}

/// Вычисление эффективного зазора между юнитами с учётом VAD и перекрытий.
///
/// Возвращает кортеж `(effective_gap_sec, is_negative)`.
///
/// Физические правила:
/// 1. Отрицательный зазор (`next_start < u_end`): метки Whisper нахлестнулись
///    или инвертированы $\to$ эффективный зазор равен 0.0, флаг `is_negative = true`.
/// 2. При наличии `speech_regions`: перекрывающиеся интервалы речи в пределах зазора
///    `[u_end, next_start]` сливаются воедино, их суммарная длительность вычитается из зазора.
/// 3. Эффективная тишина не может быть меньше 0.0.
pub fn compute_effective_gap(
    u_end: Seconds,
    next_start: Seconds,
    speech_regions: Option<&[SpeechSpan]>,
) -> (f64, bool) {
    if next_start.0 < u_end.0 {
        return (0.0, true);
    }
    let raw_gap = next_start.0 - u_end.0;
    if let Some(regions) = speech_regions {
        if !regions.is_empty() {
            let mut clamped_spans: Vec<(f64, f64)> = Vec::new();
            for r in regions {
                let s = r.start.0.max(u_end.0);
                let e = r.end.0.min(next_start.0);
                if e > s {
                    clamped_spans.push((s, e));
                }
            }
            if !clamped_spans.is_empty() {
                // Сортировка и слияние перекрывающихся участков речи
                clamped_spans.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut merged: Vec<(f64, f64)> = Vec::new();
                for span in clamped_spans {
                    if let Some(last) = merged.last_mut() {
                        if span.0 <= last.1 {
                            last.1 = last.1.max(span.1);
                            continue;
                        }
                    }
                    merged.push(span);
                }
                let speech_dur: f64 = merged.iter().map(|(s, e)| e - s).sum();
                let effective_gap = (raw_gap - speech_dur).max(0.0);
                return (effective_gap, false);
            }
        }
    }
    (raw_gap, false)
}

/// Расчет скора границы `BoundaryScore` на стыке двух юнитов.
///
/// Формула:
/// `BoundaryScore = S_punct + S_pause + S_syntax + S_whisper`
///
/// Полосы пауз (`S_pause`):
/// - `< pause_min_sec` (250мс) или отрицательный зазор: `-0.6`
/// - `[pause_min_sec, pause_soft_sec)` (250..500мс): `+0.3`
/// - `[pause_soft_sec, pause_strong_sec)` (500..800мс): `+0.7`
/// - `>= pause_strong_sec` (>= 800мс): `+1.1`
pub fn compute_boundary_score(
    current: &AlignedUnit,
    next: &AlignedUnit,
    profile: &dyn LanguageProfile,
    cfg: &SegmentationConfig,
    speech_regions: Option<&[SpeechSpan]>,
) -> f64 {
    let (eff_gap, is_negative) = compute_effective_gap(current.end, next.start, speech_regions);

    // 1. Пунктуационный скор S_punct
    let s_punct = if profile.ends_sentence(&current.text) {
        1.2
    } else if profile.ends_clause(&current.text) {
        0.35
    } else {
        0.0
    };

    // 2. Паузный скор S_pause по 4 полосам
    // Защита от машинной погрешности f64 (например, 3.3 - 2.5 = 0.7999999999999998)
    const EPSILON: f64 = 1e-6;
    let s_pause = if is_negative || eff_gap < cfg.pause_min_sec().as_f64() - EPSILON {
        -0.6
    } else if eff_gap < cfg.pause_soft_sec().as_f64() - EPSILON {
        0.3
    } else if eff_gap < cfg.pause_strong_sec().as_f64() - EPSILON {
        0.7
    } else {
        1.1
    };

    // 3. Синтаксический модификатор S_syntax
    let s_syntax = profile.syntax_boundary_modifier(current, next, Seconds(eff_gap));

    // 4. Бонус границы по сегменту Whisper S_whisper (+0.25)
    let s_whisper = if current.is_whisper_boundary {
        0.25
    } else {
        0.0
    };

    s_punct + s_pause + s_syntax + s_whisper
}

/// Вычисление штрафа длины сегмента `LengthPenalty(D)`.
pub fn compute_length_penalty(dur: f64, cfg: &SegmentationConfig) -> f64 {
    let min_sec = cfg.min_utterance_sec.as_f64();
    let ideal_sec = cfg.ideal_utterance_sec.as_f64();
    let soft_max_sec = cfg.soft_max_utterance_sec.as_f64();
    let hard_max_sec = cfg.hard_max_utterance_sec.as_f64();

    if dur < min_sec {
        cfg.dp_weight_short_penalty * (min_sec - dur).powi(2)
    } else if dur <= soft_max_sec {
        cfg.dp_weight_ideal_dev * (dur - ideal_sec).abs()
    } else if dur <= hard_max_sec {
        let soft_penalty = cfg.dp_weight_ideal_dev * (soft_max_sec - ideal_sec);
        soft_penalty + cfg.dp_weight_soft_max_penalty * (dur - soft_max_sec).powi(2)
    } else {
        let soft_penalty = cfg.dp_weight_ideal_dev * (soft_max_sec - ideal_sec);
        let hard_base =
            soft_penalty + cfg.dp_weight_soft_max_penalty * (hard_max_sec - soft_max_sec).powi(2);
        hard_base + cfg.dp_weight_hard_max_slope * (dur - hard_max_sec)
    }
}

/// Вычисление полной стоимости сегмента `[p, k)` в DP.
///
/// `Cost(p, k) = segment_penalty + LengthPenalty(D) - BoundaryReward(k-1)`
/// (награда начисляется только для внутренних разрезов `k < n`).
pub fn compute_segment_cost(
    p: usize,
    k: usize,
    units: &[AlignedUnit],
    boundary_scores: &[f64],
    cfg: &SegmentationConfig,
) -> DpCostBreakdown {
    let n = units.len();
    let seg_dur = (units[k - 1].end - units[p].start).as_f64();
    let length_penalty = compute_length_penalty(seg_dur, cfg);
    let segment_penalty = cfg.dp_segment_penalty;
    let boundary_reward = if k < n {
        cfg.dp_weight_boundary_reward * boundary_scores[k - 1]
    } else {
        0.0
    };
    let total_cost = segment_penalty + length_penalty - boundary_reward;
    DpCostBreakdown {
        segment_penalty,
        length_penalty,
        boundary_reward,
        total_cost,
    }
}

/// Сегментация хода диктора через глобальную оптимизацию Dynamic Programming (DP).
///
/// # Алгоритм:
/// 1. **Предрасчёт `boundary_scores`:** за $O(N)$ для всех внутренних стыков $i \in 0..N-1$.
/// 2. **Предрасчёт `valid_starts`:** список допустимых начал сегментов
///    по правилам профиля и `can_split_after`.
/// 3. **1-based цикл DP:**
///    Для каждого $k \in 1..=N$ отбираются кандидаты $p < k$
///    через `valid_starts.partition_point(|&p| p < k)`.
///    Порядок обхода кандидатов — обратный: от ближайших к $k$ к более ранним.
///    При совпадении стоимости строгое условие `<` (через `total_cmp`) сохраняет более позднего
///    кандидата (ближе к $k$, то есть более короткий сегмент).
///    Отсечение по `window_limit = hard_max + window_buffer` срабатывает только если уже найден
///    хотя бы один кандидат с конечной стоимостью.
/// 4. **Бэктрекинг:** восстановление последовательности точек разреза от $N$ до $0$.
/// 5. **Классификация `SegmentationReason`:**
///    - Последний сегмент хода ($k == n$): `TurnEnd` (если `is_last_turn`) иначе `SpeakerChange`.
///    - Внутренний сплит при $D > 15.0$с: `OversizeExceeded`.
///    - Терминальная пунктуация: `PunctuationPause` независимо от длины зазора.
///    - Длинная пауза $\ge 800$мс: `LongPause`.
///    - Клауза/запятая с паузой $\ge 250$мс: `SyntaxPause`.
///    - Иначе: `OptimalDpSplit`.
pub fn segment_turn_dp(
    turn: &SpeakerTurn,
    profile: &dyn LanguageProfile,
    cfg: &SegmentationConfig,
    speech_regions: Option<&[SpeechSpan]>,
    is_last_turn: bool,
) -> DpResult {
    let units = &turn.units;
    let n = units.len();
    if n == 0 {
        return DpResult {
            segments: Vec::new(),
            boundary_candidates: Vec::new(),
        };
    }

    // 1. Предрасчёт Boundary Scores для внутренних стыков
    let mut boundary_scores: Vec<f64> = Vec::with_capacity(n.saturating_sub(1));
    let mut boundary_candidates: Vec<BoundaryCandidate> = Vec::with_capacity(n.saturating_sub(1));
    for i in 0..n.saturating_sub(1) {
        let score = compute_boundary_score(&units[i], &units[i + 1], profile, cfg, speech_regions);
        boundary_scores.push(score);
        boundary_candidates.push(BoundaryCandidate {
            after_unit_index: i,
            time_offset: units[i].end,
            boundary_score: score,
            can_split_after: units[i].can_split_after,
        });
    }

    // 2. Предрасчёт допустимых начал сегментов (valid_starts)
    let mut valid_starts: Vec<usize> = Vec::with_capacity(n);
    valid_starts.push(0);
    for i in 1..n {
        let gap = units[i].start - units[i - 1].end;
        if profile.is_valid_boundary(&units[i - 1], &units[i], gap, cfg) {
            valid_starts.push(i);
        }
    }

    let window_limit = cfg.hard_max_utterance_sec.as_f64() + cfg.dp_window_buffer_sec.as_f64();

    // 3. DP цикл 1-based: dp[k] — минимальная стоимость префикса из k юнитов (0..k)
    let mut dp = vec![f64::INFINITY; n + 1];
    let mut parent = vec![0usize; n + 1];
    dp[0] = 0.0;

    for k in 1..=n {
        let mut best_cost = f64::INFINITY;
        let mut best_p = 0;

        // Быстрый срез кандидатов p < k через partition_point
        let idx = valid_starts.partition_point(|&p| p < k);

        // Обход кандидатов назад: от ближайших к k к более ранним.
        // Строгое сравнение Less сохраняет более позднего кандидата при совпадении стоимости.
        for &p in valid_starts[..idx].iter().rev() {
            let seg_dur = (units[k - 1].end - units[p].start).as_f64();

            // Прерываем поиск назад только если превышен лимит окна И найден конечный кандидат
            if seg_dur > window_limit && best_cost.is_finite() {
                break;
            }

            let cost = compute_segment_cost(p, k, units, &boundary_scores, cfg);
            let total = dp[p] + cost.total_cost;
            if total.total_cmp(&best_cost) == std::cmp::Ordering::Less {
                best_cost = total;
                best_p = p;
            }
        }

        dp[k] = best_cost;
        parent[k] = best_p;
    }

    // 4. Бэктрекинг оптимального пути
    let mut path_cuts = Vec::new();
    let mut curr = n;
    while curr > 0 {
        let p = parent[curr];
        path_cuts.push((p, curr));
        curr = p;
    }
    path_cuts.reverse();

    // 5. Конструирование RawSegment с классификацией Reason
    let mut segments = Vec::with_capacity(path_cuts.len());
    for (p, k) in path_cuts {
        let seg_dur = (units[k - 1].end - units[p].start).as_f64();
        let is_oversize = seg_dur > cfg.hard_max_utterance_sec.as_f64();
        let is_soft_max_exceeded = seg_dur > cfg.soft_max_utterance_sec.as_f64();
        let cost = compute_segment_cost(p, k, units, &boundary_scores, cfg);
        let boundary_score = if k < n {
            Some(boundary_scores[k - 1])
        } else {
            None
        };

        let reason = if k == n {
            if is_last_turn {
                SegmentationReason::TurnEnd
            } else {
                SegmentationReason::SpeakerChange
            }
        } else if is_oversize {
            SegmentationReason::OversizeExceeded
        } else if profile.ends_sentence(&units[k - 1].text) {
            SegmentationReason::PunctuationPause
        } else {
            let (eff_gap, _) =
                compute_effective_gap(units[k - 1].end, units[k].start, speech_regions);
            const EPSILON: f64 = 1e-6;
            if eff_gap >= cfg.pause_strong_sec().as_f64() - EPSILON {
                SegmentationReason::LongPause
            } else if profile.ends_clause(&units[k - 1].text)
                && eff_gap >= cfg.pause_min_sec().as_f64() - EPSILON
            {
                SegmentationReason::SyntaxPause
            } else {
                SegmentationReason::OptimalDpSplit
            }
        };

        segments.push(RawSegment {
            start_idx: p,
            end_idx: k,
            reason,
            boundary_score,
            is_oversize,
            is_soft_max_exceeded,
            cost,
        });
    }

    DpResult {
        segments,
        boundary_candidates,
    }
}

// ===========================================================================
// Модульные тесты DP (PR 5)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::{
        profile::{JapaneseProfile, RussianProfile},
        test_utils::{make_aligned_unit, make_turn, NoSyntaxProfile},
    };

    macro_rules! assert_approx_eq {
        ($a:expr, $b:expr) => {
            let diff = ($a - $b).abs();
            assert!(diff < 1e-4, "Expected {} ≈ {}, diff = {}", $a, $b, diff);
        };
        ($a:expr, $b:expr, $tol:expr) => {
            let diff = ($a - $b).abs();
            assert!(diff < $tol, "Expected {} ≈ {}, diff = {}", $a, $b, diff);
        };
    }

    /// Сценарий 1: «Да.» + «конечно.»
    /// U0 (0.3с), пауза 0.1с, U1 (0.5с).
    /// Стык: S_punct=1.2, S_pause=-0.6 => Score = 0.6.
    /// Сплит: D1=0.3c (штраф 0.5 => cost 3.0), D2=0.5c (штраф 1.75 => cost 4.25).
    /// Общая стоимость сплита: 3.0 + 4.25 - 1.5 * 0.6 = 6.35.
    /// Объединение: D=0.9c (штраф 1.55 => cost 4.05).
    /// Ожидание: Единая реплика (4.05 < 6.35).
    #[test]
    fn test_scenario_1_short_fragment_merging() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        let u0 = make_aligned_unit("Да.", 0.0, 0.3, true);
        let u1 = make_aligned_unit("конечно.", 0.4, 0.9, true);
        let turn = make_turn(vec![u0, u1]);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        // Проверка скора границы
        assert_eq!(res.boundary_candidates.len(), 1);
        assert_approx_eq!(res.boundary_candidates[0].boundary_score, 0.6);

        // Решение DP: объединение в одну реплику
        assert_eq!(res.segments.len(), 1);
        assert_eq!(res.segments[0].start_idx, 0);
        assert_eq!(res.segments[0].end_idx, 2);
        assert_approx_eq!(res.segments[0].cost.total_cost, 4.05);

        // Проверка теоретической стоимости альтернативы сплита
        let split_cost = compute_segment_cost(0, 1, &turn.units, &[0.6], &cfg).total_cost
            + compute_segment_cost(1, 2, &turn.units, &[0.6], &cfg).total_cost;
        assert_approx_eq!(split_cost, 6.35);
        assert!(res.segments[0].cost.total_cost < split_cost);
    }

    /// Сценарий 2: Длинная фраза 8с (4с + 4с)
    /// U0..Uk (4.0с), пауза 0.6с, Uk+1..UN (4.0с).
    /// Стык: S_punct=1.2, S_pause=0.7 => Score = 1.9.
    /// Сплит: D1=4.0с (cost 2.5), D2=4.0с (cost 2.5). Сплит = 5.0 - 1.5 * 1.9 = 2.15.
    /// Объединение: D=8.6с (penalty 2.72 => cost 5.22).
    /// Ожидание: Сплит на 2 реплики (2.15 < 5.22).
    #[test]
    fn test_scenario_2_long_phrase_8s_split() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        let u0 = make_aligned_unit("Первая фраза.", 0.0, 4.0, true);
        let u1 = make_aligned_unit("Вторая фраза.", 4.6, 8.6, true);
        let turn = make_turn(vec![u0, u1]);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        assert_eq!(res.boundary_candidates.len(), 1);
        assert_approx_eq!(res.boundary_candidates[0].boundary_score, 1.9);

        // Решение DP: сплит на 2 реплики
        assert_eq!(res.segments.len(), 2);
        assert_eq!(res.segments[0].start_idx, 0);
        assert_eq!(res.segments[0].end_idx, 1);
        assert_eq!(res.segments[1].start_idx, 1);
        assert_eq!(res.segments[1].end_idx, 2);

        let total_dp_cost: f64 = res.segments.iter().map(|s| s.cost.total_cost).sum();
        assert_approx_eq!(total_dp_cost, 2.15);

        // Теоретическая стоимость объединения
        let merge_cost = compute_segment_cost(0, 2, &turn.units, &[1.9], &cfg).total_cost;
        assert_approx_eq!(merge_cost, 5.22);
        assert!(total_dp_cost < merge_cost);
    }

    /// Сценарий 3: Дискриминирующий тест синтаксиса
    /// U0 «думаю,» (3.9с), пауза 0.6с, U1 «что...» (3.9с). Всего 8.4с.
    /// - Со штрафом синтаксиса (-1.5) RussianProfile: Score = -0.45.
    ///   Сплит: 5.775 > Объединение: 4.82 => ОБЪЕДИНЕНИЕ.
    /// - Без штрафа синтаксиса NoSyntaxProfile: Score = 1.05.
    ///   Сплит: 3.525 < Объединение: 4.82 => СПЛИТ.
    #[test]
    fn test_scenario_3_syntax_penalty_discriminating() {
        let cfg = SegmentationConfig::default();

        let u0 = make_aligned_unit("думаю,", 0.0, 3.9, true);
        let u1 = make_aligned_unit("что мы успеем.", 4.5, 8.4, true);
        let turn = make_turn(vec![u0, u1]);

        // 1. Со штрафом синтаксиса (RussianProfile)
        let profile_ru = RussianProfile;
        let res_ru = segment_turn_dp(&turn, &profile_ru, &cfg, None, true);

        assert_approx_eq!(res_ru.boundary_candidates[0].boundary_score, -0.45);
        assert_eq!(
            res_ru.segments.len(),
            1,
            "Синтаксический штраф должен предотвратить разрыв перед 'что'"
        );
        assert_approx_eq!(res_ru.segments[0].cost.total_cost, 4.82);

        let split_cost_ru = compute_segment_cost(0, 1, &turn.units, &[-0.45], &cfg).total_cost
            + compute_segment_cost(1, 2, &turn.units, &[-0.45], &cfg).total_cost;
        assert_approx_eq!(split_cost_ru, 5.775);
        assert!(res_ru.segments[0].cost.total_cost < split_cost_ru);

        // 2. Без штрафа синтаксиса (NoSyntaxProfile)
        let profile_no_syn = NoSyntaxProfile;
        let res_no_syn = segment_turn_dp(&turn, &profile_no_syn, &cfg, None, true);

        assert_approx_eq!(res_no_syn.boundary_candidates[0].boundary_score, 1.05);
        assert_eq!(
            res_no_syn.segments.len(),
            2,
            "Без синтаксического штрафа фразы должны разделиться"
        );

        let total_split_no_syn: f64 = res_no_syn.segments.iter().map(|s| s.cost.total_cost).sum();
        assert_approx_eq!(total_split_no_syn, 3.525);

        let merge_cost_no_syn = compute_segment_cost(0, 2, &turn.units, &[1.05], &cfg).total_cost;
        assert_approx_eq!(merge_cost_no_syn, 4.82);
        assert!(total_split_no_syn < merge_cost_no_syn);
    }

    /// Сценарий 4: Две фразы по 3с с точкой
    /// U0 (3.0с), пауза 0.6с, U1 (3.0с). Всего 6.6с.
    /// Стык: S_punct=1.2, S_pause=0.7 => Score = 1.9.
    /// Сплит: 3.15 < Объединение: 3.80.
    /// Ожидание: Сплит на две реплики.
    #[test]
    fn test_scenario_4_two_phrases_3s_split() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        let u0 = make_aligned_unit("Привет.", 0.0, 3.0, true);
        let u1 = make_aligned_unit("Мир.", 3.6, 6.6, true);
        let turn = make_turn(vec![u0, u1]);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        assert_approx_eq!(res.boundary_candidates[0].boundary_score, 1.9);
        assert_eq!(res.segments.len(), 2);

        let total_cost: f64 = res.segments.iter().map(|s| s.cost.total_cost).sum();
        assert_approx_eq!(total_cost, 3.15);

        let merge_cost = compute_segment_cost(0, 2, &turn.units, &[1.9], &cfg).total_cost;
        assert_approx_eq!(merge_cost, 3.80);
        assert!(total_cost < merge_cost);
    }

    /// Сценарий 5: 2с + 2с без пунктуации
    /// U0 (2.0с), пауза 0.2с, U1 (2.0с). Всего 4.2с.
    /// Стык: S_punct=0.0, S_pause=-0.6 => Score = -0.6.
    /// Сплит: 7.90 > Объединение: 2.60.
    /// Ожидание: Единая реплика.
    #[test]
    fn test_scenario_5_two_phrases_2s_no_punct_merges() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        let u0 = make_aligned_unit("привет", 0.0, 2.0, true);
        let u1 = make_aligned_unit("мир", 2.2, 4.2, true);
        let turn = make_turn(vec![u0, u1]);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        assert_approx_eq!(res.boundary_candidates[0].boundary_score, -0.6);
        assert_eq!(res.segments.len(), 1);
        assert_approx_eq!(res.segments[0].cost.total_cost, 2.60);

        let split_cost = compute_segment_cost(0, 1, &turn.units, &[-0.6], &cfg).total_cost
            + compute_segment_cost(1, 2, &turn.units, &[-0.6], &cfg).total_cost;
        assert_approx_eq!(split_cost, 7.90);
        assert!(res.segments[0].cost.total_cost < split_cost);
    }

    /// Сценарий 6: CJK «これはテストです。明日は晴れでしょう。»
    /// Часть 1 (2.5с), Часть 2 (2.5с). Стык: S_punct=1.2.
    /// - При паузе 0.7с (Score = 1.9): Сплит 3.65 > Объединение 3.35 => ОБЪЕДИНЕНИЕ.
    /// - При паузе 0.8с (Score = 2.3): Сплит 3.05 < Объединение 3.40 => СПЛИТ.
    #[test]
    fn test_scenario_6_cjk_clause_splitting_threshold() {
        let cfg = SegmentationConfig::default();
        let profile = JapaneseProfile;

        // 1. Пауза 0.7с: склейка
        let u0_a = make_aligned_unit("これはテストです。", 0.0, 2.5, true);
        let u1_a = make_aligned_unit("明日は晴れでしょう。", 3.2, 5.7, true);
        let turn_a = make_turn(vec![u0_a, u1_a]);

        let res_a = segment_turn_dp(&turn_a, &profile, &cfg, None, true);
        assert_approx_eq!(res_a.boundary_candidates[0].boundary_score, 1.9);
        assert_eq!(
            res_a.segments.len(),
            1,
            "При паузе 0.7с реплики CJK должны объединиться"
        );
        assert_approx_eq!(res_a.segments[0].cost.total_cost, 3.35);

        let split_cost_a = compute_segment_cost(0, 1, &turn_a.units, &[1.9], &cfg).total_cost
            + compute_segment_cost(1, 2, &turn_a.units, &[1.9], &cfg).total_cost;
        assert_approx_eq!(split_cost_a, 3.65);

        // 2. Пауза 0.8с: сплит
        let u0_b = make_aligned_unit("これはテストです。", 0.0, 2.5, true);
        let u1_b = make_aligned_unit("明日は晴れでしょう。", 3.3, 5.8, true);
        let turn_b = make_turn(vec![u0_b, u1_b]);

        let res_b = segment_turn_dp(&turn_b, &profile, &cfg, None, true);
        assert_approx_eq!(res_b.boundary_candidates[0].boundary_score, 2.3);
        assert_eq!(
            res_b.segments.len(),
            2,
            "При паузе 0.8с реплики CJK должны разделиться"
        );

        let total_split_b: f64 = res_b.segments.iter().map(|s| s.cost.total_cost).sum();
        assert_approx_eq!(total_split_b, 3.05);

        let merge_cost_b = compute_segment_cost(0, 2, &turn_b.units, &[2.3], &cfg).total_cost;
        assert_approx_eq!(merge_cost_b, 3.40);
        assert!(total_split_b < merge_cost_b);
    }

    /// Проверка 4 полос пауз и бонуса whisper-флага (+0.25):
    /// Две фразы по 3с с точкой:
    /// - пауза 0.4с -> склейка (3.70 vs 3.75)
    /// - пауза 0.5с -> сплит (3.15 vs 3.75)
    /// - whisper-флаг даёт ровно +0.25 к Score
    #[test]
    fn test_pause_four_bands_and_whisper_flag() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        // 1. Пауза 0.4с (полоса 250..500мс => S_pause = +0.3, Score = 1.2 + 0.3 = 1.5)
        // Сплит: 6.0 - 1.5 * 1.5 = 3.75. Объединение: D = 6.4 => penalty = 0.5 * 2.4 = 1.2 => 3.70.
        // Ожидание: склейка (3.70 < 3.75)
        let u0_400 = make_aligned_unit("Привет.", 0.0, 3.0, true);
        let u1_400 = make_aligned_unit("Мир.", 3.4, 6.4, true);
        let turn_400 = make_turn(vec![u0_400, u1_400]);

        let res_400 = segment_turn_dp(&turn_400, &profile, &cfg, None, true);
        assert_approx_eq!(res_400.boundary_candidates[0].boundary_score, 1.5);
        assert_eq!(
            res_400.segments.len(),
            1,
            "При паузе 0.4с должно побеждать объединение"
        );
        assert_approx_eq!(res_400.segments[0].cost.total_cost, 3.70);

        let split_cost_400 = compute_segment_cost(0, 1, &turn_400.units, &[1.5], &cfg).total_cost
            + compute_segment_cost(1, 2, &turn_400.units, &[1.5], &cfg).total_cost;
        assert_approx_eq!(split_cost_400, 3.75);

        // 2. Пауза 0.5с (полоса 500..800мс => S_pause = +0.7, Score = 1.2 + 0.7 = 1.9)
        // Сплит: 6.0 - 1.5 * 1.9 = 3.15.
        // Объединение: D = 6.5 => penalty = 0.5 * 2.5 = 1.25 => 3.75.
        // Ожидание: сплит (3.15 < 3.75)
        let u0_500 = make_aligned_unit("Привет.", 0.0, 3.0, true);
        let u1_500 = make_aligned_unit("Мир.", 3.5, 6.5, true);
        let turn_500 = make_turn(vec![u0_500, u1_500]);

        let res_500 = segment_turn_dp(&turn_500, &profile, &cfg, None, true);
        assert_approx_eq!(res_500.boundary_candidates[0].boundary_score, 1.9);
        assert_eq!(
            res_500.segments.len(),
            2,
            "При паузе 0.5с должен побеждать сплит"
        );

        let total_split_500: f64 = res_500.segments.iter().map(|s| s.cost.total_cost).sum();
        assert_approx_eq!(total_split_500, 3.15);

        let merge_cost_500 = compute_segment_cost(0, 2, &turn_500.units, &[1.9], &cfg).total_cost;
        assert_approx_eq!(merge_cost_500, 3.75);

        // 3. Проверка флага whisper_boundary: добавляет ровно +0.25
        let mut u0_whisper = make_aligned_unit("Привет.", 0.0, 3.0, true);
        u0_whisper.is_whisper_boundary = true;
        let u1_whisper = make_aligned_unit("Мир.", 3.4, 6.4, true);
        let turn_whisper = make_turn(vec![u0_whisper, u1_whisper]);

        let res_whisper = segment_turn_dp(&turn_whisper, &profile, &cfg, None, true);
        // Без флага Score был 1.5, с флагом должен стать 1.75
        assert_approx_eq!(res_whisper.boundary_candidates[0].boundary_score, 1.75);
        // Сплит: 6.0 - 1.5 * 1.75 = 3.375 < 3.70 => теперь фразы делятся!
        assert_eq!(res_whisper.segments.len(), 2);
    }

    /// Проверка VAD: слияние перекрывающихся SpeechSpan, обрезка по зазору;
    /// отрицательный зазор -> 0 и S_pause = -0.6.
    #[test]
    fn test_vad_handling_and_negative_gap() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        // 1. Отрицательный зазор (нахлёст меток)
        let u0 = make_aligned_unit("Слово", 0.0, 1.5, true);
        let u1 = make_aligned_unit("следующее", 1.4, 2.0, true); // start < prev.end
        let (eff_gap, is_neg) = compute_effective_gap(u0.end, u1.start, None);
        assert_eq!(eff_gap, 0.0);
        assert!(is_neg);

        let score = compute_boundary_score(&u0, &u1, &profile, &cfg, None);
        // Без пунктуации (0.0), пауза отрицательная (-0.6) => -0.6
        assert_approx_eq!(score, -0.6);

        // 2. Слияние перекрывающихся интервалов VAD
        // Зазор [1.0 .. 2.0] = 1.0с.
        // VAD интервалы: [0.8 .. 1.3], [1.2 .. 1.6], [2.1 .. 2.5]
        // Пересечения с [1.0 .. 2.0]: [1.0 .. 1.3] и [1.2 .. 1.6]
        // Слитый интервал: [1.0 .. 1.6] длительностью 0.6с
        // Эффективный зазор тишины: 1.0 - 0.6 = 0.4с (полоса 250..500мс => S_pause = +0.3)
        let regions = vec![
            SpeechSpan {
                start: Seconds(0.8),
                end: Seconds(1.3),
            },
            SpeechSpan {
                start: Seconds(1.2),
                end: Seconds(1.6),
            },
            SpeechSpan {
                start: Seconds(2.1),
                end: Seconds(2.5),
            },
        ];
        let (eff_gap_vad, is_neg_vad) =
            compute_effective_gap(Seconds(1.0), Seconds(2.0), Some(&regions));
        assert_approx_eq!(eff_gap_vad, 0.4);
        assert!(!is_neg_vad);

        let u_a = make_aligned_unit("слово", 0.0, 1.0, true);
        let u_b = make_aligned_unit("слово", 2.0, 3.0, true);
        let score_vad = compute_boundary_score(&u_a, &u_b, &profile, &cfg, Some(&regions));
        assert_approx_eq!(score_vad, 0.3);
    }

    /// Тест 12: монолог 18.0с без допустимых границ
    /// Входные данные: 18с непрерывной речи с can_split_after = false.
    /// Ожидание: ровно 1 реплика, TurnEnd (или SpeakerChange),
    /// is_oversize = true, is_soft_max_exceeded = true, стоимость 252.5.
    #[test]
    fn test_12_single_monologue_18s_oversize() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        // Создаем монолог из 9 слов по 2.0с каждое, все can_split_after = false
        let mut units = Vec::new();
        for i in 0..9 {
            let start = i as f64 * 2.0;
            let end = start + 2.0;
            units.push(make_aligned_unit("монолог", start, end, false));
        }
        let turn = make_turn(units);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        assert_eq!(res.segments.len(), 1);
        let seg = &res.segments[0];
        assert_eq!(seg.start_idx, 0);
        assert_eq!(seg.end_idx, 9);
        assert!(seg.is_oversize);
        assert!(seg.is_soft_max_exceeded);
        assert_eq!(seg.reason, SegmentationReason::TurnEnd);
        assert_approx_eq!(seg.cost.total_cost, 252.5);

        // Проверка при is_last_turn = false -> SpeakerChange
        let res_mid = segment_turn_dp(&turn, &profile, &cfg, None, false);
        assert_eq!(
            res_mid.segments[0].reason,
            SegmentationReason::SpeakerChange
        );
    }

    /// Тест 13: неразрывный участок монолога внутри хода сохраняет внешние границы
    /// Входные данные: монолог 32.0с:
    /// - Секция 1 (0.0–5.0с): слова с can_split_after = true, на 5.0с точка и пауза 0.6с;
    /// - Секция 2 (5.6–27.6с, 22.0с): участок монолога, у которого can_split_after = false
    ///   у всех юнитов кроме последнего на 27.6с (у него true);
    /// - Секция 3 (27.6–32.0с, 4.4с): can_split_after = true, точка на 32.0с и конец хода.
    ///
    /// Ожидаемый результат: ровно 3 реплики:
    /// 1. Реплика 1 (0.0–5.0с): PunctuationPause, is_oversize=false,
    ///    is_soft_max_exceeded=false;
    /// 2. Реплика 2 (5.6–27.6с, 22.0с): OversizeExceeded, is_oversize=true,
    ///    is_soft_max_exceeded=true;
    /// 3. Реплика 3 (27.6–32.0с, 4.4с): TurnEnd, is_oversize=false,
    ///    is_soft_max_exceeded=false.
    #[test]
    fn test_13_dp_long_unsplittable_segment_preserves_surrounding_boundaries() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;

        let mut units = Vec::new();

        // Секция 1: 0.0 - 5.0с (например, 2 слова по 2.5с, второе завершается точкой)
        units.push(make_aligned_unit("Начало", 0.0, 2.5, true));
        units.push(make_aligned_unit("фразы.", 2.5, 5.0, true)); // точка, can_split_after = true

        // Пауза 0.6с перед Секцией 2
        // Секция 2: 5.6 - 27.6с (22.0с). Сделаем 10 юнитов по 2.2с.
        // У всех can_split_after = false, кроме последнего!
        for i in 0..10 {
            let start = 5.6 + i as f64 * 2.2;
            let end = start + 2.2;
            let is_last = i == 9;
            units.push(make_aligned_unit("длинный_монолог", start, end, is_last));
        }

        // Секция 3: 27.6 - 32.0с (4.4с)
        units.push(make_aligned_unit("Заключение", 27.6, 29.8, true));
        units.push(make_aligned_unit("мысли.", 29.8, 32.0, true));

        let turn = make_turn(units);
        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);

        assert_eq!(
            res.segments.len(),
            3,
            "Должно сформироваться ровно 3 реплики"
        );

        // Сегмент 1
        assert_eq!(res.segments[0].start_idx, 0);
        assert_eq!(res.segments[0].end_idx, 2);
        assert_eq!(res.segments[0].reason, SegmentationReason::PunctuationPause);
        assert!(!res.segments[0].is_oversize);
        assert!(!res.segments[0].is_soft_max_exceeded);

        // Сегмент 2 (вынужденный сплит сверхлимитного монолога внутри хода)
        assert_eq!(res.segments[1].start_idx, 2);
        assert_eq!(res.segments[1].end_idx, 12);
        assert_eq!(res.segments[1].reason, SegmentationReason::OversizeExceeded);
        assert!(res.segments[1].is_oversize);
        assert!(res.segments[1].is_soft_max_exceeded);

        // Сегмент 3 (финал хода)
        assert_eq!(res.segments[2].start_idx, 12);
        assert_eq!(res.segments[2].end_idx, 14);
        assert_eq!(res.segments[2].reason, SegmentationReason::TurnEnd);
        assert!(!res.segments[2].is_oversize);
        assert!(!res.segments[2].is_soft_max_exceeded);
    }

    /// Граничный случай: пустой ход (0 юнитов)
    #[test]
    fn test_empty_turn() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;
        let turn = make_turn(Vec::new());

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);
        assert!(res.segments.is_empty());
        assert!(res.boundary_candidates.is_empty());
    }

    /// Граничный случай: ход из 1 юнита
    #[test]
    fn test_single_unit_turn() {
        let cfg = SegmentationConfig::default();
        let profile = RussianProfile;
        let u0 = make_aligned_unit("Здравствуйте.", 0.0, 1.2, true);
        let turn = make_turn(vec![u0]);

        let res = segment_turn_dp(&turn, &profile, &cfg, None, true);
        assert_eq!(res.segments.len(), 1);
        assert_eq!(res.segments[0].start_idx, 0);
        assert_eq!(res.segments[0].end_idx, 1);
        assert_eq!(res.segments[0].reason, SegmentationReason::TurnEnd);
        assert_eq!(res.segments[0].boundary_score, None);
        assert!(!res.segments[0].is_oversize);
        assert!(!res.segments[0].is_soft_max_exceeded);
        assert!(res.boundary_candidates.is_empty());
    }
}
