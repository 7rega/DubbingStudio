//! Бенчмарк и сравнительный анализ: Classic (8s) vs Natural / Dubbing Mode (до 12.5–14с).
//!
//! Запуск: cargo run --example compare_segmenters -p dub-asr

use dub_asr::{
    segment_words, word_ends_sentence, DubbingSegment, DubbingSegmenter, Segment, Word,
    WordWithTimestamp,
};

/// Classic pipeline (segment_words 0.6s/8.0s + merge_short_turns).
fn run_classic_pipeline(words: &[WordWithTimestamp]) -> Vec<Segment> {
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

/// Статистический отчет по ТЗ раздел 14.
#[derive(Debug, Default)]
pub struct PipelineStats {
    pub total_segments: usize,
    pub broken_phrases: usize,
    pub avg_duration: f64,
    pub max_duration: f64,
    pub segs_over_12_5s: usize,
    pub segs_over_14s: usize,
    pub fallback_splits: usize,
    pub eight_sec_cut_splits: usize,
    pub eliminated_artificial_breaks: usize,
}

pub fn evaluate_classic(segs: &[Segment]) -> PipelineStats {
    if segs.is_empty() {
        return PipelineStats::default();
    }
    let mut broken = 0usize;
    let mut dur_sum = 0.0f64;
    let mut max_dur = 0.0f64;
    let mut over_12_5 = 0usize;
    let mut over_14 = 0usize;
    let mut eight_sec_cuts = 0usize;

    for (i, s) in segs.iter().enumerate() {
        let dur = (s.end - s.start).max(0.0);
        dur_sum += dur;
        if dur > max_dur {
            max_dur = dur;
        }
        if dur > 12.5 {
            over_12_5 += 1;
        }
        if dur > 14.0 {
            over_14 += 1;
        }
        let is_last = i + 1 == segs.len();
        let ends_sent = word_ends_sentence(&s.text);
        if !ends_sent && !is_last {
            broken += 1;
            if dur >= 7.5 && dur <= 8.2 {
                eight_sec_cuts += 1;
            }
        }
    }

    PipelineStats {
        total_segments: segs.len(),
        broken_phrases: broken,
        avg_duration: dur_sum / segs.len() as f64,
        max_duration: max_dur,
        segs_over_12_5s: over_12_5,
        segs_over_14s: over_14,
        fallback_splits: 0,
        eight_sec_cut_splits: eight_sec_cuts,
        eliminated_artificial_breaks: 0,
    }
}

pub fn evaluate_natural(segs: &[DubbingSegment], fallbacks: usize, classic_broken: usize) -> PipelineStats {
    if segs.is_empty() {
        return PipelineStats::default();
    }
    let mut broken = 0usize;
    let mut dur_sum = 0.0f64;
    let mut max_dur = 0.0f64;
    let mut over_12_5 = 0usize;
    let mut over_14 = 0usize;

    for (i, s) in segs.iter().enumerate() {
        let dur = (s.end - s.start).max(0.0);
        dur_sum += dur;
        if dur > max_dur {
            max_dur = dur;
        }
        if dur > 12.5 {
            over_12_5 += 1;
        }
        if dur > 14.0 {
            over_14 += 1;
        }
        let is_last = i + 1 == segs.len();
        let ends_sent = word_ends_sentence(&s.text);
        if !ends_sent && !is_last {
            broken += 1;
        }
    }

    let eliminated = classic_broken.saturating_sub(broken);

    PipelineStats {
        total_segments: segs.len(),
        broken_phrases: broken,
        avg_duration: dur_sum / segs.len() as f64,
        max_duration: max_dur,
        segs_over_12_5s: over_12_5,
        segs_over_14s: over_14,
        fallback_splits: fallbacks,
        eight_sec_cut_splits: 0,
        eliminated_artificial_breaks: eliminated,
    }
}

fn main() {
    println!("===============================================================================");
    println!("          DUBBING STUDIO SEGMENTER BENCHMARK: CLASSIC vs NATURAL MODE          ");
    println!("===============================================================================");

    let segmenter = DubbingSegmenter::new();

    // ── Тестовый корпус: Комплексный набор (диалог + длинные мысли + паузы вдоха) ──
    let mut corpus: Vec<WordWithTimestamp> = Vec::new();

    // Блок 1 (A): Длинная мысль 10.5с с паузой вдоха 0.7с посреди фразы
    let b1 = vec![
        ("I", 0.0, 0.3), ("wanted", 0.4, 0.8), ("to", 0.9, 1.1), ("tell", 1.2, 1.5),
        ("you", 1.6, 1.8), ("that", 1.9, 2.3), ("we", 3.0, 3.4), ("should", 3.5, 3.8),
        ("definitely", 3.9, 4.5), ("leave", 4.6, 5.0), ("before", 5.1, 5.5),
        ("the", 5.6, 5.8), ("heavy", 5.9, 6.4), ("rain", 6.5, 7.0), ("starts", 7.1, 7.7),
        ("flooding", 7.8, 8.4), ("the", 8.5, 8.7), ("streets.", 8.8, 9.5),
    ];
    for (w, st, en) in b1 {
        corpus.push(WordWithTimestamp::new(w, st, en).with_speaker("spk_A"));
    }

    // Блок 2 (B): Реплика спикера B
    let b2 = vec![
        ("Are", 10.0, 10.3), ("you", 10.4, 10.6), ("sure", 10.7, 11.1), ("about", 11.2, 11.5),
        ("that?", 11.6, 12.1),
    ];
    for (w, st, en) in b2 {
        corpus.push(WordWithTimestamp::new(w, st, en).with_speaker("spk_B"));
    }

    // Блок 3 (A): Фраза на 13.5с
    let b3 = vec![
        ("Yes,", 12.8, 13.2), ("the", 13.3, 13.5), ("forecast", 13.6, 14.2),
        ("predicts", 14.3, 14.8), ("a", 14.9, 15.0), ("massive", 15.1, 15.7),
        ("storm", 15.8, 16.3), ("approaching", 16.4, 17.2), ("our", 17.3, 17.6),
        ("city", 17.7, 18.1), ("very", 18.2, 18.6), ("soon.", 18.7, 19.4),
    ];
    for (w, st, en) in b3 {
        corpus.push(WordWithTimestamp::new(w, st, en).with_speaker("spk_A"));
    }

    let classic_segs = run_classic_pipeline(&corpus);
    let (natural_segs, debug) = segmenter.segment_with_debug(&corpus);
    let fallbacks = debug.iter().filter(|d| d.is_fallback).count();

    let c_stats = evaluate_classic(&classic_segs);
    let n_stats = evaluate_natural(&natural_segs, fallbacks, c_stats.broken_phrases);

    println!("\n[РЕЗУЛЬТАТЫ СРАВНЕНИЯ]");
    println!("  Classic Segments:               {}", c_stats.total_segments);
    println!("  Natural Segments:               {}", n_stats.total_segments);
    println!("  Classic Broken / Cut Phrases:   {}", c_stats.broken_phrases);
    println!("  Natural Broken / Cut Phrases:   {}", n_stats.broken_phrases);
    println!("  Eliminated Artificial Breaks:   {}", n_stats.eliminated_artificial_breaks);
    println!("  Avg Duration (Classic / Nat):   {:.2}s / {:.2}s", c_stats.avg_duration, n_stats.avg_duration);
    println!("  Max Duration (Classic / Nat):   {:.2}s / {:.2}s", c_stats.max_duration, n_stats.max_duration);
    println!("  Segments >12.5s (Classic / Nat): {} / {}", c_stats.segs_over_12_5s, n_stats.segs_over_12_5s);
    println!("  Segments >14.0s (Classic / Nat): {} / {}", c_stats.segs_over_14s, n_stats.segs_over_14s);
    println!("  Fallback Splits:                {}", n_stats.fallback_splits);

    println!("\n[ПРИМЕРЫ СЕГМЕНТОВ CLASSIC]");
    for (i, s) in classic_segs.iter().enumerate() {
        println!("  [{}] [{:.1}s - {:.1}s] ({:.2}s) \"{}\"", i + 1, s.start, s.end, s.end - s.start, s.text);
    }

    println!("\n[ПРИМЕРЫ СЕГМЕНТОВ NATURAL]");
    for (i, s) in natural_segs.iter().enumerate() {
        println!("  [{}] [Spk: {:?}] [{:.1}s - {:.1}s] ({:.2}s) \"{}\"", i + 1, s.speaker, s.start, s.end, s.end - s.start, s.text);
    }
}
