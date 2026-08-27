//! Сегментация словного потока в реплики дубляжа. Порт _segment из dubengine/asr.py:
//! разрыв на паузах > max_gap, конце предложения (.!?…) или превышении max_dur.

use serde::{Deserialize, Serialize};

/// Дефолтные параметры сегментации: разрыв на паузе > SEG_MAX_GAP сек (вдох до 0.8с) и
/// безопасные диапазоны длины для TTS (зелёная зона до 12.0с, жёсткий потолок 15.0с).
pub const SEG_MAX_GAP: f64 = 0.8;
pub const SEG_IDEAL_DUR: f64 = 12.0;
pub const SEG_MAX_DUR: f64 = 15.0;

/// Слово со временем (секунды).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub is_asr_boundary: bool,
}

impl Word {
    pub fn new(word: impl Into<String>, start: f64, end: f64) -> Self {
        Self {
            word: word.into(),
            start,
            end,
            is_asr_boundary: false,
        }
    }

    pub fn with_boundary(mut self, is_boundary: bool) -> Self {
        self.is_asr_boundary = is_boundary;
        self
    }
}

/// Сегмент дубляжа: [start,end] + текст + список слов. Тот же контракт, что в Python-движке.
#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
}

fn ends_sentence(word: &str) -> bool {
    let trimmed = word.trim_end_matches(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '\''
            || c == '»'
            || c == '”'
            || c == '’'
            || c == ')'
            || c == ']'
            || c == '}'
    });
    trimmed.ends_with(['.', '!', '?', '…', '。', '！', '？', '؟', '۔']) || trimmed.ends_with("...")
}

fn ends_clause(word: &str) -> bool {
    let trimmed = word.trim_end_matches(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '\''
            || c == '»'
            || c == '”'
            || c == '’'
            || c == ')'
            || c == ']'
            || c == '}'
    });
    trimmed.ends_with([',', ';', ':', '-', '—', '–', '、', '，', '；', '：', '،'])
}

/// Склейка слов сегмента через пробел (эквивалент `join(" ").trim()`, без промежуточного Vec).
fn join_words(ws: &[Word]) -> String {
    let mut s = String::new();
    for x in ws {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&x.word);
    }
    s.trim().to_string()
}

/// Разбить поток слов на сегменты:
/// - До 12.0с (зелёная зона): накапливаем слова, паузы вдоха до max_gap (0.8с) не ломают фразу;
/// - 12.0–15.0с (жёлтая зона): ищем запятую или заметную паузу >=0.35с;
/// - >15.0с (жёсткий предохранитель): разрез по max_dur.
pub fn segment_words(words: &[Word], max_gap: f64, max_dur: f64) -> Vec<Segment> {
    let mut segs: Vec<Vec<Word>> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();

    for w in words {
        if let (Some(last), Some(first)) = (cur.last(), cur.first()) {
            let gap = (w.start - last.end).max(0.0);
            let dur = (last.end - first.start).max(0.0);

            // 1. Пауза больше допустимого (напр. >0.8с)
            let is_long_pause = gap > max_gap;

            // 2. Жёлтая зона (12–15с): мягкий разрыв на границе Whisper/VAD, запятой или паузе >=0.35с
            let is_soft_split = dur >= SEG_IDEAL_DUR
                && (last.is_asr_boundary || ends_clause(&last.word) || gap >= 0.35);

            // 3. Жёсткий лимит (>15с)
            let is_hard_limit = dur >= max_dur;

            if is_long_pause || is_soft_split || is_hard_limit {
                segs.push(std::mem::take(&mut cur));
            }
        }
        cur.push(w.clone());
        if ends_sentence(&w.word) {
            segs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        segs.push(cur);
    }

    segs.into_iter()
        .map(|ws| Segment {
            start: ws.first().unwrap().start,
            end: ws.last().unwrap().end,
            text: join_words(&ws),
            words: ws,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(word: &str, start: f64, end: f64) -> Word {
        Word { word: word.into(), start, end }
    }

    #[test]
    fn splits_on_sentence_end() {
        let words = vec![w("Hello", 0.0, 0.4), w("world.", 0.4, 0.8), w("Next", 0.9, 1.2)];
        let segs = segment_words(&words, 0.8, 15.0);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Hello world.");
        assert_eq!(segs[1].text, "Next");
    }

    #[test]
    fn breath_pause_under_0_8s_does_not_split() {
        // Пауза 0.7с (вдох) посреди фразы < 12.0с -> не должна разрезаться
        let words = vec![w("I", 0.0, 0.3), w("said", 0.4, 0.8), w("this.", 1.5, 2.0)];
        let segs = segment_words(&words, 0.8, 15.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "I said this.");
    }

    #[test]
    fn long_pause_over_0_8s_splits() {
        // Пауза 1.5с > 0.8с -> разрез
        let words = vec![w("a", 0.0, 0.2), w("b", 1.8, 2.0)];
        let segs = segment_words(&words, 0.8, 15.0);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn phrase_up_to_12s_without_punct_not_split() {
        // Фраза 10.5с без знаков препинания -> держится единым сегментом
        let mut words = Vec::new();
        for i in 0..20 {
            words.push(w("word", i as f64 * 0.5, i as f64 * 0.5 + 0.45));
        }
        let segs = segment_words(&words, 0.8, 15.0);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].end - segs[0].start) > 9.5);
    }

    #[test]
    fn yellow_zone_splits_on_clause() {
        // Фраза >12.0с с запятой на 12.5с -> делится на запятой
        let mut words = Vec::new();
        for i in 0..20 {
            let word = if i == 19 { "here," } else { "word" };
            words.push(w(word, i as f64 * 0.65, i as f64 * 0.65 + 0.60));
        }
        for i in 0..5 {
            words.push(w("continuation", 13.5 + i as f64 * 0.5, 13.5 + i as f64 * 0.5 + 0.45));
        }
        let segs = segment_words(&words, 0.8, 15.0);
        assert!(segs.len() >= 2);
        assert!(segs[0].text.ends_with("here,"));
    }

    #[test]
    fn yellow_zone_splits_on_asr_boundary() {
        // Фраза >12.0с без пунктуации, но на 12.5с граница сегмента Whisper/VAD -> делится по границе Whisper
        let mut words = Vec::new();
        for i in 0..20 {
            let mut word = w("word", i as f64 * 0.65, i as f64 * 0.65 + 0.60);
            if i == 19 {
                word = word.with_boundary(true);
            }
            words.push(word);
        }
        for i in 0..5 {
            words.push(w("continuation", 13.5 + i as f64 * 0.5, 13.5 + i as f64 * 0.5 + 0.45));
        }
        let segs = segment_words(&words, 0.8, 15.0);
        assert!(segs.len() >= 2);
        assert_eq!(segs[0].words.len(), 20);
    }
}
