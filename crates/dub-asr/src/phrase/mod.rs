//! Подсистема формирования естественных фраз (Natural Phrases) для нейродубляжа.
//!
//! Модуль обеспечивает объединение ASR-токенов/слов и интервалов диаризации
//! в естественные реплики на основе динамического программирования (DP)
//! с оптимизацией пауз, пунктуации и синтаксических связей.

pub mod aligner;
pub mod builder;
pub mod config;
pub mod dp;
pub mod profile;
pub mod time;
pub mod turns;
pub mod types;

#[cfg(test)]
pub mod test_utils;

pub use aligner::{align_and_smooth, align_units, smooth_aligned_units};
pub use builder::{
    build_utterance, clamp_adjacent_utterances, finalize_utterances, link_backchannels,
    sanitize_unit_timestamps,
};
pub use config::{ConfigError, SegmentationConfig};
pub use dp::{
    compute_boundary_score, compute_effective_gap, compute_length_penalty, compute_segment_cost,
    segment_turn_dp, DpResult, RawSegment,
};
pub use profile::{
    normalize_language_tag, resolve_source_language, ChineseProfile, DefaultProfile,
    EnglishProfile, JapaneseProfile, KoreanProfile, LanguageProfile, ProfileRegistry,
    RussianProfile,
};
pub use time::{Millis, Seconds};
pub use turns::{build_speaker_turns, detect_overlaps};
pub use types::*;

/// Сквозная чистая функция сегментации ASR-потока в естественные фразы дубляжа.
///
/// Конвейер:
/// 1. Валидация конфигурации (`config.validate()`);
/// 2. Определение языка источника и выбор `LanguageProfile`;
/// 3. Санитизация временных меток ASR-юнитов (clamp и защита от NaN/Inf/отрицательных);
/// 4. Привязка к интервалам диаризации и сглаживание (`align_and_smooth`);
/// 5. Группировка в ходы дикторов с детекцией наложений (`build_speaker_turns`);
/// 6. Независимая DP-сегментация каждого хода (`segment_turn_dp`);
/// 7. Финализация реплик в `builder`: хронологическая сортировка, сквозные ID,
///    clamp смежных границ одного диктора и связывание прерванных реплик (`finalize_utterances`).
pub fn segment(
    asr: &AsrResult,
    diar: &DiarizationResult,
    config: &SegmentationConfig,
    speech_regions: Option<&[SpeechSpan]>,
) -> Result<Vec<Utterance>, SegmentationError> {
    config.validate()?;

    let resolved_lang =
        resolve_source_language(config.language.as_deref(), asr.language.as_deref());
    let profile = ProfileRegistry::get(Some(&resolved_lang));

    let sanitized_units = builder::sanitize_unit_timestamps(&asr.units)?;
    if sanitized_units.is_empty() {
        return Ok(Vec::new());
    }

    let aligned_units = aligner::align_and_smooth(&sanitized_units, diar, config, profile.as_ref());
    let turns = turns::build_speaker_turns(&aligned_units, Some(diar));

    let mut raw_utterances = Vec::new();
    let turns_count = turns.len();
    for (turn_idx, turn) in turns.iter().enumerate() {
        let is_last_turn = turn_idx + 1 == turns_count;
        let dp_res =
            dp::segment_turn_dp(turn, profile.as_ref(), config, speech_regions, is_last_turn);
        for seg in &dp_res.segments {
            raw_utterances.push(builder::build_utterance(
                turn,
                seg,
                &resolved_lang,
                profile.as_ref(),
                config,
                &dp_res.boundary_candidates,
            ));
        }
    }

    let final_utterances = builder::finalize_utterances(raw_utterances, profile.as_ref(), config);

    Ok(final_utterances)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_approx_eq {
        ($a:expr, $b:expr) => {
            let diff = ($a - $b).abs();
            assert!(
                diff < 1e-4,
                "assertion failed: `(left ~= right)`\n  left: `{}`,\n right: `{}`, diff: `{}`",
                $a,
                $b,
                diff
            );
        };
    }

    #[test]
    fn test_segment_empty_input() {
        let asr = AsrResult::default();
        let diar = DiarizationResult::default();
        let cfg = SegmentationConfig::default();

        let res = segment(&asr, &diar, &cfg, None).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn test_segment_invalid_timestamps_returns_error() {
        let asr = AsrResult {
            units: vec![AsrUnit {
                text: "bad".to_string(),
                start: Seconds(f64::NAN),
                end: Seconds(1.0),
                confidence: None,
                is_whisper_boundary: false,
                can_split_after: true,
            }],
            raw_segments: Vec::new(),
            language: Some("ru".to_string()),
        };
        let diar = DiarizationResult::default();
        let cfg = SegmentationConfig::default();

        let res = segment(&asr, &diar, &cfg, None);
        assert!(matches!(res, Err(SegmentationError::InvalidTimestamps(_))));
    }

    #[test]
    fn test_api_has_zero_target_language_awareness() {
        let cfg = SegmentationConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.to_lowercase().contains("target"));
        assert!(!json.to_lowercase().contains("tgt"));
    }

    #[test]
    fn test_spec_section_14_pipeline_example() {
        // Эталонный тест Раздела 14 ТЗ:
        // Входные данные: 3 хода дикторов (S0, S1, S0):
        // 1. S0 [10.10–13.00] (2.90с): «Я хотел тебе сказать, что завтра мы уезжаем.»
        // 2. S1 [14.00–14.40] (0.40с): «Ага.»
        // 3. S0 [15.20–16.90] (1.70с): «И вернёмся через неделю.»
        let units = vec![
            AsrUnit {
                text: "Я".to_string(),
                start: Seconds(10.10),
                end: Seconds(10.25),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "хотел".to_string(),
                start: Seconds(10.27),
                end: Seconds(10.60),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "тебе".to_string(),
                start: Seconds(10.62),
                end: Seconds(10.85),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "сказать,".to_string(),
                start: Seconds(10.88),
                end: Seconds(11.40),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "что".to_string(),
                start: Seconds(11.45),
                end: Seconds(11.65),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "завтра".to_string(),
                start: Seconds(11.68),
                end: Seconds(12.10),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "мы".to_string(),
                start: Seconds(12.12),
                end: Seconds(12.30),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "уезжаем.".to_string(),
                start: Seconds(12.33),
                end: Seconds(13.00),
                confidence: Some(0.99),
                is_whisper_boundary: true,
                can_split_after: true,
            },
            AsrUnit {
                text: "Ага.".to_string(),
                start: Seconds(14.00),
                end: Seconds(14.40),
                confidence: Some(0.99),
                is_whisper_boundary: true,
                can_split_after: true,
            },
            AsrUnit {
                text: "И".to_string(),
                start: Seconds(15.20),
                end: Seconds(15.35),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "вернёмся".to_string(),
                start: Seconds(15.38),
                end: Seconds(16.00),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "через".to_string(),
                start: Seconds(16.03),
                end: Seconds(16.35),
                confidence: Some(0.99),
                is_whisper_boundary: false,
                can_split_after: true,
            },
            AsrUnit {
                text: "неделю.".to_string(),
                start: Seconds(16.38),
                end: Seconds(16.90),
                confidence: Some(0.99),
                is_whisper_boundary: true,
                can_split_after: true,
            },
        ];

        let diarization = DiarizationResult {
            intervals: vec![
                DiarInterval {
                    speaker: "S0".to_string(),
                    start: Seconds(10.00),
                    end: Seconds(13.20),
                    confidence: Some(0.95),
                },
                DiarInterval {
                    speaker: "S1".to_string(),
                    start: Seconds(13.90),
                    end: Seconds(14.60),
                    confidence: Some(0.95),
                },
                DiarInterval {
                    speaker: "S0".to_string(),
                    start: Seconds(15.10),
                    end: Seconds(17.00),
                    confidence: Some(0.95),
                },
            ],
        };

        let asr = AsrResult {
            units,
            raw_segments: Vec::new(),
            language: Some("ru".to_string()),
        };

        let cfg = SegmentationConfig::default();
        let utterances = segment(&asr, &diarization, &cfg, None).unwrap();

        assert_eq!(utterances.len(), 3, "Ожидается ровно 3 реплики");

        // Реплика 1
        assert_eq!(utterances[0].id, "utt_001");
        assert_eq!(utterances[0].speaker, "S0");
        assert_eq!(utterances[0].start, Seconds(10.10));
        assert_eq!(utterances[0].end, Seconds(13.00));
        assert_eq!(
            utterances[0].text,
            "Я хотел тебе сказать, что завтра мы уезжаем."
        );
        assert_eq!(
            utterances[0].segmentation.reason,
            SegmentationReason::SpeakerChange
        );
        assert!(!utterances[0].segmentation.is_oversize);
        assert!(!utterances[0].segmentation.is_soft_max_exceeded);
        // Ручной расчёт DP: D=2.90, penalty=0.5*|2.9-4.0|=0.55 => Cost = 2.5 + 0.55 = 3.05
        assert_approx_eq!(
            utterances[0].dp_cost_breakdown.as_ref().unwrap().total_cost,
            3.05
        );

        // Реплика 2
        assert_eq!(utterances[1].id, "utt_002");
        assert_eq!(utterances[1].speaker, "S1");
        assert_eq!(utterances[1].start, Seconds(14.00));
        assert_eq!(utterances[1].end, Seconds(14.40));
        assert_eq!(utterances[1].text, "Ага.");
        assert_eq!(
            utterances[1].segmentation.reason,
            SegmentationReason::SpeakerChange
        );
        assert!(!utterances[1].segmentation.is_oversize);
        assert!(!utterances[1].segmentation.is_soft_max_exceeded);
        // Ручной расчёт DP: D=0.40, penalty=0.5*|0.4-4.0|=1.80 => Cost = 2.5 + 1.80 = 4.30
        assert_approx_eq!(
            utterances[1].dp_cost_breakdown.as_ref().unwrap().total_cost,
            4.30
        );

        // Реплика 3
        assert_eq!(utterances[2].id, "utt_003");
        assert_eq!(utterances[2].speaker, "S0");
        assert_eq!(utterances[2].start, Seconds(15.20));
        assert_eq!(utterances[2].end, Seconds(16.90));
        assert_eq!(utterances[2].text, "И вернёмся через неделю.");
        assert_eq!(
            utterances[2].segmentation.reason,
            SegmentationReason::TurnEnd
        );
        assert!(!utterances[2].segmentation.is_oversize);
        assert!(!utterances[2].segmentation.is_soft_max_exceeded);
        // Ручной расчёт DP: D=1.70, penalty=0.5*|1.7-4.0|=1.15 => Cost = 2.5 + 1.15 = 3.65
        assert_approx_eq!(
            utterances[2].dp_cost_breakdown.as_ref().unwrap().total_cost,
            3.65
        );

        // Проверка инварианта связывания бэкчанела:
        // Первая реплика завершается точкой («мы уезжаем.»),
        // поэтому связывание НЕ активируется: context_group_id == None, continues_from == None
        assert_eq!(
            utterances[0].context_group_id, None,
            "context_group_id первой реплики должен быть None"
        );
        assert_eq!(
            utterances[0].continues_from, None,
            "continues_from первой реплики должен быть None"
        );
        assert_eq!(
            utterances[1].context_group_id, None,
            "context_group_id бэкчанела должен быть None"
        );
        assert_eq!(
            utterances[2].context_group_id, None,
            "context_group_id третьей реплики должен быть None"
        );
        assert_eq!(
            utterances[2].continues_from, None,
            "continues_from третьей реплики должен быть None"
        );

        // Проверка сериализации в Segment.extra:
        let extra = utterances[0].to_extra_value();
        assert_eq!(extra["source_language"], "ru");
        assert_eq!(extra["segmentation"]["reason"], "SpeakerChange");
        assert_eq!(extra["context_group_id"], serde_json::Value::Null);
    }
}
