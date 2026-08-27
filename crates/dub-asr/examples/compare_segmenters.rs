//! Бенчмарк и сравнительный анализ: OLD pipeline (segment_words + merge_short_turns)
//! против NEW (DubbingSegmenter со строгими границами спикеров).
//!
//! Запуск: cargo run --example compare_segmenters -p dub-asr

use dub_asr::{
    segment_words, word_ends_sentence, DubbingSegment, DubbingSegmenter, Segment, Word,
    WordWithTimestamp,
};

/// Эмуляция старого пайплайна DubbingStudio (segment_words 0.6s/8.0s + merge_short_turns).
fn run_old_pipeline(words: &[WordWithTimestamp]) -> Vec<Segment> {
    let raw_words: Vec<Word> = words.iter().cloned().map(Into::into).collect();
    let mut segs = segment_words(&raw_words, 0.6, 8.0);

    if segs.len() < 2 {
        return segs;
    }
    const GAP: f64 = 0.35;
    const OVERLAP: f64 = 0.2;
    const SHORT: f64 = 1.6;
    const MAX_DUR: f64 = 12.0;
    const MAX_CH: usize = 200;

    let src = std::mem::take(&mut segs);
    let mut out: Vec<Segment> = Vec::with_capacity(src.len());

    for s in src {
        if let Some(last) = out.last_mut() {
            let gap = s.start - last.end;
            let short = (last.end - last.start) < SHORT || (s.end - s.start) < SHORT;
            let dur_ok = (s.end - last.start) <= MAX_DUR;
            let ch_ok = last.text.chars().count() + s.text.chars().count() < MAX_CH;

            if gap > -OVERLAP && gap < GAP && short && dur_ok && ch_ok {
                let lt = last.text.trim_end();
                let rt = s.text.trim_start();
                let sep = if lt.is_empty() || rt.is_empty() { "" } else { " " };
                last.text = format!("{lt}{sep}{rt}");
                last.end = last.end.max(s.end);
                last.words.extend(s.words);
                continue;
            }
        }
        out.push(s);
    }
    out
}

/// Статистический отчет по набору сегментов.
#[derive(Debug, Default)]
pub struct PipelineMetrics {
    pub total_segments: usize,
    pub broken_phrases: usize,
    pub multi_speaker_segments: usize,
    pub speaker_boundary_splits: usize,
    pub avg_duration: f64,
    pub max_duration: f64,
    pub min_duration: f64,
    pub in_sweet_spot_count: usize, // 3.0s .. 12.5s
    pub over_hard_limit_count: usize, // > 15.0s
    pub micro_segments_count: usize, // < 1.5s
    pub emergency_fallback_count: usize,
}

pub fn evaluate_dubbing_segments(
    segs: &[DubbingSegment],
    fallback_count: usize,
    spk_splits: usize,
) -> PipelineMetrics {
    if segs.is_empty() {
        return PipelineMetrics::default();
    }

    let mut broken = 0usize;
    let mut multi_spk = 0usize;
    let mut dur_sum = 0.0f64;
    let mut max_dur = 0.0f64;
    let mut min_dur = f64::MAX;
    let mut sweet_spot = 0usize;
    let mut over_hard = 0usize;
    let mut micro = 0usize;

    for (i, s) in segs.iter().enumerate() {
        let dur = (s.end - s.start).max(0.0);
        dur_sum += dur;
        if dur > max_dur {
            max_dur = dur;
        }
        if dur < min_dur {
            min_dur = dur;
        }

        if (3.0..=12.5).contains(&dur) {
            sweet_spot += 1;
        }
        if dur > 15.0 {
            over_hard += 1;
        }
        if dur < 1.5 {
            micro += 1;
        }

        // Проверка: есть ли слова с другим спикером
        let has_alien = s.words.iter().any(|w| w.speaker != s.speaker);
        if has_alien {
            multi_spk += 1;
        }

        let is_last = i + 1 == segs.len();
        let ends_sent = word_ends_sentence(&s.text);
        if !ends_sent && !is_last {
            broken += 1;
        }
    }

    PipelineMetrics {
        total_segments: segs.len(),
        broken_phrases: broken,
        multi_speaker_segments: multi_spk,
        speaker_boundary_splits: spk_splits,
        avg_duration: dur_sum / segs.len() as f64,
        max_duration: max_dur,
        min_duration: min_dur,
        in_sweet_spot_count: sweet_spot,
        over_hard_limit_count: over_hard,
        micro_segments_count: micro,
        emergency_fallback_count: fallback_count,
    }
}

fn main() {
    println!("===============================================================================");
    println!("     DUBBING STUDIO SEGMENTER BENCHMARK: SPEAKER BOUNDARIES & TTS TUNING       ");
    println!("===============================================================================");

    let segmenter = DubbingSegmenter::new();

    // ── ТЕСТОВЫЙ КОРПУС 1: Диалог нескольких спикеров (A -> B -> A) ──
    let dialogue_words = vec![
        WordWithTimestamp::new("Hello,", 0.0, 0.4).with_speaker("A"),
        WordWithTimestamp::new("I", 0.5, 0.7).with_speaker("A"),
        WordWithTimestamp::new("wanted", 0.8, 1.2).with_speaker("A"),
        WordWithTimestamp::new("to", 1.3, 1.4).with_speaker("A"),
        WordWithTimestamp::new("tell", 1.5, 1.8).with_speaker("A"),
        WordWithTimestamp::new("you", 1.9, 2.1).with_speaker("A"),
        WordWithTimestamp::new("something.", 2.2, 3.0).with_speaker("A"),
        WordWithTimestamp::new("What?", 3.2, 3.8).with_speaker("B"),
        WordWithTimestamp::new("It's", 4.0, 4.3).with_speaker("A"),
        WordWithTimestamp::new("about", 4.4, 4.8).with_speaker("A"),
        WordWithTimestamp::new("yesterday.", 4.9, 5.8).with_speaker("A"),
    ];

    let old_segs = run_old_pipeline(&dialogue_words);
    let (new_segs, debug) = segmenter.segment_with_debug(&dialogue_words);
    let fallbacks = debug.iter().filter(|d| d.is_fallback).count();
    let spk_splits = debug.iter().filter(|d| d.is_speaker_boundary).count();

    let new_m = evaluate_dubbing_segments(&new_segs, fallbacks, spk_splits);

    println!("\n[ТЕСТ 1: Диалог A -> B -> A]");
    println!("  OLD pipeline -> Сегментов: {} (старый pipeline не учитывал границы диаризации на уровне слов)", old_segs.len());
    println!("  NEW Segmenter -> Сегментов: {}, Сегментов с >1 спикером: {} (0 гарантировано!)", new_m.total_segments, new_m.multi_speaker_segments);
    println!("  Границ спикеров зафиксировано: {}, Fallbacks: {}", new_m.speaker_boundary_splits, new_m.emergency_fallback_count);

    println!("\n  Сегменты NEW:");
    for (i, s) in new_segs.iter().enumerate() {
        println!("    [{}] [Spk: {:?}] [{:.1}s - {:.1}s] ({:.2}s) \"{}\"", i, s.speaker, s.start, s.end, s.end - s.start, s.text);
    }
}
