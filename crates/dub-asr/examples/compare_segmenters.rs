//! Бенчмарк и сравнительный анализ: OLD pipeline (segment_words + merge_short_turns)
//! против NEW (DubbingSegmenter).
//!
//! Запуск: cargo run --example compare_segmenters -p dub-asr

use dub_asr::{
    is_terminal_punct, segment_words, word_ends_sentence, DubbingSegmenter, Segment, Word,
    WordWithTimestamp,
};

/// Эмуляция старого пайплайна DubbingStudio (segment_words 0.6s/8.0s + merge_short_turns).
fn run_old_pipeline(words: &[WordWithTimestamp]) -> Vec<Segment> {
    let raw_words: Vec<Word> = words.iter().cloned().map(Into::into).collect();
    let mut segs = segment_words(&raw_words, 0.6, 8.0);

    // merge_short_turns из analyze.rs:
    // GAP=0.35, OVERLAP=0.2, SHORT=1.6, MAX_DUR=12.0, MAX_CH=200
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
    pub avg_duration: f64,
    pub max_duration: f64,
    pub min_duration: f64,
    pub in_sweet_spot_count: usize, // 3.0s .. 12.5s
    pub over_hard_limit_count: usize, // > 15.0s
    pub micro_segments_count: usize, // < 1.5s
    pub emergency_fallback_count: usize,
}

pub fn evaluate_segments(segs: &[Segment], fallback_count: usize) -> PipelineMetrics {
    if segs.is_empty() {
        return PipelineMetrics::default();
    }

    let mut broken = 0usize;
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

        // Проверка: завершено ли предложение или это оборванный кусок (кроме последнего)
        let is_last = i + 1 == segs.len();
        let ends_sent = word_ends_sentence(&s.text);
        if !ends_sent && !is_last {
            broken += 1;
        }
    }

    PipelineMetrics {
        total_segments: segs.len(),
        broken_phrases: broken,
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
    println!("         DUBBING STUDIO SEGMENTER BENCHMARK: OLD vs NEW (DubbingSegmenter)     ");
    println!("===============================================================================");

    let segmenter = DubbingSegmenter::new();

    // ── Тестовый корпус 1: Диалог с естественными паузами на вдох (0.65–0.8с) ──
    let mut dialogue_words = Vec::new();
    let mut t = 0.0f64;
    let sentences = vec![
        ("We went to the local grocery store and", 6, 0.45, 0.70), // вдох 0.70с -> старый ASR режет!
        ("bought some delicious fresh apples.", 5, 0.50, 1.20),
        ("It was really surprising how cheap everything was today,", 8, 0.45, 0.65), // пауза 0.65с -> старый ASR режет!
        ("especially considering the current market prices.", 6, 0.50, 1.50),
        ("Do you think we should go back tomorrow morning?", 9, 0.40, 1.80),
        ("Yes, absolutely, I would love to grab more fresh bread.", 10, 0.42, 2.00),
    ];

    for (text, word_count, word_dur, end_pause) in sentences {
        let parts: Vec<&str> = text.split_whitespace().collect();
        for (i, p) in parts.iter().enumerate() {
            let st = t;
            let en = st + word_dur;
            dialogue_words.push(WordWithTimestamp::new(*p, st, en));
            t = en;
            if i + 1 == parts.len() {
                t += end_pause;
            } else {
                t += 0.04;
            }
        }
    }

    let old_segs = run_old_pipeline(&dialogue_words);
    let (new_segs, debug) = segmenter.segment_with_debug(&dialogue_words);
    let fallbacks = debug.iter().filter(|d| d.is_fallback).count();

    let old_m = evaluate_segments(&old_segs, 0);
    let new_m = evaluate_segments(&new_segs, fallbacks);

    println!("\n[ТЕСТ 1: Естественный диалог с паузами на вдох]");
    println!("  OLD pipeline -> Сегментов: {}, Оборванных фраз: {}, Средняя длит: {:.2}с, Макс: {:.2}с",
        old_m.total_segments, old_m.broken_phrases, old_m.avg_duration, old_m.max_duration);
    println!("  NEW Segmenter -> Сегментов: {}, Оборванных фраз: {}, Средняя длит: {:.2}с, Макс: {:.2}с",
        new_m.total_segments, new_m.broken_phrases, new_m.avg_duration, new_m.max_duration);

    println!("\n  Примеры OLD:");
    for (i, s) in old_segs.iter().enumerate() {
        println!("    [{:.1}s - {:.1}s] ({:.2}s) \"{}\"", s.start, s.end, s.end - s.start, s.text);
    }
    println!("\n  Примеры NEW:");
    for (i, s) in new_segs.iter().enumerate() {
        println!("    [{:.1}s - {:.1}s] ({:.2}s) \"{}\"", s.start, s.end, s.end - s.start, s.text);
    }

    println!("\n===============================================================================");
    println!("                         ИТОГОВЫЙ СРАВНИТЕЛЬНЫЙ АНАЛИЗ                         ");
    println!("===============================================================================");
}
