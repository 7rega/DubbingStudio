//! Property-based and stress tests for the natural phrase segmentation subsystem.
//!
//! Validates the 7 core invariants defined in the specification (Section 5.3):
//! 1. Completeness: every input unit is contained in exactly one utterance.
//! 2. Ordering: unit ordering is strictly monotonically preserved within and across utterances.
//! 3. Speaker Purity: no utterance mixes units from different speakers.
//! 4. Determinism: identical inputs produce identical utterance sequences.
//! 5. Temporal Consistency & Clamping: adjacent utterances of the same speaker never overlap.
//! 6. Stress Test: 10,000-word monologue completes within a reasonable timeout without panic.
//! 7. Fault Tolerance: inverted timestamps are safely clamped; NaN/Inf/negative timestamps error cleanly.

use std::time::Instant;
use proptest::prelude::*;

use dub_asr::phrase::{
    self, AsrResult, AsrUnit, DiarizationSegment, Seconds, SegmentationConfig, SegmentationError,
    Utterance,
};

#[derive(Debug, Clone)]
struct RawUnitGen {
    text: String,
    dur: f64,
    gap: f64,
    can_split_after: bool,
    is_whisper: bool,
}

fn arb_unit_gen() -> impl Strategy<Value = RawUnitGen> {
    let words = prop_oneof![
        Just("Привет".to_string()),
        Just("мир,".to_string()),
        Just("как".to_string()),
        Just("дела?".to_string()),
        Just("хорошо.".to_string()),
        Just("да,".to_string()),
        Just("конечно!".to_string()),
        Just("что".to_string()),
        Just("потому".to_string()),
        Just("чтобы".to_string()),
        Just("слово".to_string()),
        Just("фраза.".to_string()),
        Just("сегмент".to_string()),
        Just("звук".to_string()),
    ];
    (
        words,
        0.05f64..0.8f64,
        0.0f64..0.6f64,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(text, dur, gap, can_split_after, is_whisper)| RawUnitGen {
            text,
            dur,
            gap,
            can_split_after,
            is_whisper,
        })
}

fn arb_valid_session() -> impl Strategy<Value = (AsrResult, Vec<DiarizationSegment>, SegmentationConfig)> {
    (
        prop::collection::vec(arb_unit_gen(), 1..40),
        prop_oneof![Just("ru"), Just("en"), Just("ja"), Just("und")],
        prop::collection::vec(
            (0.5f64..4.0f64, prop_oneof![Just("S0"), Just("S1"), Just("S2")]),
            1..4,
        ),
    )
        .prop_map(|(unit_gens, lang, diar_specs)| {
            let mut units = Vec::new();
            let mut cur_time = 0.1f64;
            for g in unit_gens {
                cur_time += g.gap;
                let start = Seconds(cur_time);
                let end = Seconds(cur_time + g.dur);
                cur_time += g.dur;
                units.push(AsrUnit {
                    text: g.text,
                    start,
                    end,
                    confidence: Some(0.95),
                    is_whisper_boundary: g.is_whisper,
                    can_split_after: g.can_split_after,
                });
            }

            let full_text = units
                .iter()
                .map(|u| u.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");

            let asr = AsrResult {
                text: full_text,
                language: Some(lang.to_string()),
                units,
            };

            let total_dur = cur_time + 1.0;
            let mut diar = Vec::new();
            let mut d_cur = 0.0f64;
            for (dur, spk) in diar_specs {
                if d_cur >= total_dur {
                    break;
                }
                let d_end = (d_cur + dur).min(total_dur);
                diar.push(DiarizationSegment {
                    speaker: spk.to_string(),
                    start: Seconds(d_cur),
                    end: Seconds(d_end),
                });
                d_cur = d_end;
            }
            if d_cur < total_dur {
                diar.push(DiarizationSegment {
                    speaker: "S0".to_string(),
                    start: Seconds(d_cur),
                    end: Seconds(total_dur),
                });
            }

            let config = SegmentationConfig::default();
            (asr, diar, config)
        })
}

fn arb_jittered_session() -> impl Strategy<Value = (AsrResult, Vec<DiarizationSegment>, SegmentationConfig)> {
    (
        prop::collection::vec(
            (
                prop_oneof![Just("слово".to_string()), Just("тест".to_string())],
                0.0f64..10.0f64,
                0.0f64..10.0f64,
            ),
            1..25,
        ),
        prop::collection::vec(
            (1.0f64..5.0f64, prop_oneof![Just("S0"), Just("S1")]),
            1..3,
        ),
    )
        .prop_map(|(raw_units, diar_specs)| {
            let units = raw_units
                .into_iter()
                .map(|(text, t1, t2)| AsrUnit {
                    text,
                    start: Seconds(t1),
                    end: Seconds(t2), // может быть t2 < t1 или немонотонно
                    confidence: Some(0.9),
                    is_whisper_boundary: false,
                    can_split_after: true,
                })
                .collect::<Vec<_>>();

            let asr = AsrResult {
                text: "jittered".to_string(),
                language: Some("ru".to_string()),
                units,
            };

            let mut diar = Vec::new();
            let mut d_cur = 0.0f64;
            for (dur, spk) in diar_specs {
                diar.push(DiarizationSegment {
                    speaker: spk.to_string(),
                    start: Seconds(d_cur),
                    end: Seconds(d_cur + dur),
                });
                d_cur += dur;
            }

            let config = SegmentationConfig::default();
            (asr, diar, config)
        })
}

fn check_completeness(asr: &AsrResult, utts: &[Utterance]) {
    let total_utt_units: usize = utts.iter().map(|u| u.units.len()).sum();
    assert_eq!(
        total_utt_units,
        asr.units.len(),
        "Total units in utterances ({total_utt_units}) must equal input units ({})",
        asr.units.len()
    );
}

fn check_ordering(asr: &AsrResult, utts: &[Utterance]) {
    for utt in utts {
        for w in utt.units.windows(2) {
            assert!(
                w[0].start <= w[1].start,
                "Units inside utterance must have non-decreasing start times: {:?} > {:?}",
                w[0].start,
                w[1].start
            );
            assert!(
                w[0].end <= w[1].end,
                "Units inside utterance must have non-decreasing end times: {:?} > {:?}",
                w[0].end,
                w[1].end
            );
        }
    }

    let orig_texts: Vec<&str> = asr.units.iter().map(|u| u.text.as_str()).collect();
    let flat_texts: Vec<&str> = utts
        .iter()
        .flat_map(|u| u.units.iter().map(|un| un.text.as_str()))
        .collect();
    assert_eq!(
        flat_texts, orig_texts,
        "Flattened units across utterances must match original text sequence"
    );
}

fn check_speaker_purity(utts: &[Utterance]) {
    for utt in utts {
        for unit in &utt.units {
            assert_eq!(
                unit.speaker, utt.speaker,
                "Unit speaker '{}' must match utterance speaker '{}'",
                unit.speaker, utt.speaker
            );
        }
    }
}

fn check_determinism(
    asr: &AsrResult,
    diar: &[DiarizationSegment],
    cfg: &SegmentationConfig,
    first_res: &[Utterance],
) {
    let second_res = phrase::segment(asr, diar, cfg, None).expect("Second run must succeed");
    assert_eq!(
        first_res,
        &second_res[..],
        "Segmentation must be 100% deterministic"
    );
}

fn check_temporal_consistency_and_clamping(utts: &[Utterance]) {
    for utt in utts {
        assert!(
            utt.start <= utt.end,
            "Utterance start {:?} must be <= end {:?}",
            utt.start,
            utt.end
        );
    }
    for w in utts.windows(2) {
        assert!(
            w[0].start <= w[1].start,
            "Utterances must be sorted by start time: {:?} > {:?}",
            w[0].start,
            w[1].start
        );
    }
    for w in utts.windows(2) {
        if w[0].speaker == w[1].speaker {
            assert!(
                w[0].end <= w[1].start,
                "Adjacent utterances of the same speaker must not overlap: {:?} > {:?}",
                w[0].end,
                w[1].start
            );
        }
    }
}

fn cases_count() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases_count()))]

    #[test]
    fn prop_core_invariants((asr, diar, cfg) in arb_valid_session()) {
        let utts = phrase::segment(&asr, &diar, &cfg, None)
            .expect("Segmentation of valid session must succeed");

        // Инвариант 1: Полнота
        check_completeness(&asr, &utts);

        // Инвариант 2: Порядок
        check_ordering(&asr, &utts);

        // Инвариант 3: Чистота спикера
        check_speaker_purity(&utts);

        // Инвариант 4: Детерминизм
        check_determinism(&asr, &diar, &cfg, &utts);

        // Инвариант 5: Временная непротиворечивость и clamp
        check_temporal_consistency_and_clamping(&utts);
    }

    #[test]
    fn prop_fault_tolerance_inverted((asr, diar, cfg) in arb_jittered_session()) {
        let res = phrase::segment(&asr, &diar, &cfg, None);
        prop_assert!(res.is_ok(), "Jittered/inverted timestamps must be sanitized safely");
        let utts = res.unwrap();
        for utt in &utts {
            prop_assert!(utt.start <= utt.end);
            for unit in &utt.units {
                prop_assert!(unit.start <= unit.end);
            }
        }
        for w in utts.windows(2) {
            if w[0].speaker == w[1].speaker {
                prop_assert!(w[0].end <= w[1].start);
            }
        }
    }
}

#[test]
fn test_stress_10000_words_monologue() {
    let word_pool = [
        "слово", "фраза,", "предложение.", "речь", "дубляж,", "звук.", "голос",
        "актер,", "студия.", "сегмент", "дорожка,", "таймкод.", "пауза", "дыхание,",
        "интонация.", "тембр", "скорость,", "громкость.", "монтаж", "результат.",
    ];

    let mut units = Vec::with_capacity(10_000);
    let mut cur_time = 0.0f64;
    for i in 0..10_000 {
        let text = word_pool[i % word_pool.len()];
        let dur = 0.25f64;
        let gap = if text.ends_with('.') {
            0.60f64
        } else if text.ends_with(',') {
            0.20f64
        } else {
            0.05f64
        };

        cur_time += gap;
        let start = Seconds(cur_time);
        let end = Seconds(cur_time + dur);
        cur_time += dur;

        units.push(AsrUnit {
            text: text.to_string(),
            start,
            end,
            confidence: Some(0.95),
            is_whisper_boundary: text.ends_with('.'),
            can_split_after: text.ends_with('.') || text.ends_with(','),
        });
    }

    let asr = AsrResult {
        text: "10000 words stress monologue".to_string(),
        language: Some("ru".to_string()),
        units,
    };

    let diar = vec![DiarizationSegment {
        speaker: "SPEAKER_00".to_string(),
        start: Seconds::ZERO,
        end: Seconds(cur_time + 10.0),
    }];

    let cfg = SegmentationConfig::default();

    let start_instant = Instant::now();
    let utts = phrase::segment(&asr, &diar, &cfg, None)
        .expect("10,000-word monologue segmentation must succeed without error");
    let elapsed = start_instant.elapsed();

    println!(
        "Stress test 10,000 words completed in {:.3}s, produced {} utterances",
        elapsed.as_secs_f64(),
        utts.len()
    );

    // Разумный таймаут в секундах для CI Windows debug runner
    assert!(
        elapsed.as_secs_f64() < 15.0,
        "10,000 words must segment within 15 seconds (took {:.3}s)",
        elapsed.as_secs_f64()
    );

    // Инвариант полноты на 10 000 слов
    let total_words: usize = utts.iter().map(|u| u.units.len()).sum();
    assert_eq!(total_words, 10_000);

    // Инварианты порядка, чистоты спикера и clamp
    check_ordering(&asr, &utts);
    check_speaker_purity(&utts);
    check_temporal_consistency_and_clamping(&utts);
}

#[test]
fn test_empty_asr_input() {
    let asr = AsrResult {
        text: String::new(),
        language: Some("ru".to_string()),
        units: Vec::new(),
    };
    let diar = Vec::new();
    let cfg = SegmentationConfig::default();

    let utts = phrase::segment(&asr, &diar, &cfg, None).expect("Empty input must succeed");
    assert!(utts.is_empty(), "Empty input must return empty utterances");
}

#[test]
fn test_single_unit_input() {
    let asr = AsrResult {
        text: "Привет.".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "Привет.".to_string(),
            start: Seconds(1.0),
            end: Seconds(1.5),
            confidence: Some(0.95),
            is_whisper_boundary: true,
            can_split_after: true,
        }],
    };
    let diar = vec![DiarizationSegment {
        speaker: "S0".to_string(),
        start: Seconds(0.0),
        end: Seconds(3.0),
    }];
    let cfg = SegmentationConfig::default();

    let utts = phrase::segment(&asr, &diar, &cfg, None).expect("Single unit input must succeed");
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].units.len(), 1);
    assert_eq!(utts[0].speaker, "S0");
    assert_eq!(utts[0].start, Seconds(1.0));
    assert_eq!(utts[0].end, Seconds(1.5));
    assert_eq!(utts[0].text, "Привет.");
}

#[test]
fn test_invalid_timestamps_strictly_error() {
    let cfg = SegmentationConfig::default();
    let diar = vec![DiarizationSegment {
        speaker: "S0".to_string(),
        start: Seconds::ZERO,
        end: Seconds(10.0),
    }];

    // 1. NaN в start
    let asr_nan_start = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(f64::NAN),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_nan_start, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));

    // 2. NaN в end
    let asr_nan_end = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(1.0),
            end: Seconds(f64::NAN),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_nan_end, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));

    // 3. +INFINITY
    let asr_inf = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(f64::INFINITY),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_inf, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));

    // 4. -INFINITY
    let asr_neg_inf = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(1.0),
            end: Seconds(f64::NEG_INFINITY),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_neg_inf, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));

    // 5. Отрицательный start
    let asr_neg_start = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(-0.5),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_neg_start, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));

    // 6. Отрицательный end
    let asr_neg_end = AsrResult {
        text: "тест".to_string(),
        language: Some("ru".to_string()),
        units: vec![AsrUnit {
            text: "тест".to_string(),
            start: Seconds(0.0),
            end: Seconds(-0.01),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        }],
    };
    assert!(matches!(
        phrase::segment(&asr_neg_end, &diar, &cfg, None),
        Err(SegmentationError::InvalidTimestamps)
    ));
}
