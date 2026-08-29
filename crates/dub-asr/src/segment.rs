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

/// Сегмент дубляжа: [start,end] + текст + список слов + опциональный спикер. Тот же контракт, что в Python-движке.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
    #[serde(default)]
    pub speaker: Option<String>,
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
            speaker: None,
        })
        .collect()
}

/// Разбить поток слов на сегменты с учётом спикеров из диаризации (Word-Level Diarization):
/// 1. Каждому слову назначается спикер по интервалам turns через DiarIndex.
/// 2. Фильтрация микро-джиттера: единичное изолированное слово, окруженное речью другого спикера
///    с зазором < 0.25с, сглаживается до спикера соседей (устранение граничного шума Sortformer).
/// 3. Жесткий разрыв сегмента при смене спикера (cur_spk != word_spk).
/// 4. Внутри одного спикера — стандартные правила: конец предложения (.!?), пауза > max_gap,
///    жёлтая зона 12-15с по границам клауз/VAD, жесткий лимит max_dur.
pub fn segment_words_with_diarization(
    words: &[Word],
    turns: &[crate::Turn],
    max_gap: f64,
    max_dur: f64,
) -> Vec<Segment> {
    if words.is_empty() {
        return Vec::new();
    }
    if turns.is_empty() {
        return segment_words(words, max_gap, max_dur);
    }

    let diar_index = crate::reconcile::DiarIndex::new(turns);
    let mut tagged: Vec<(Word, i32)> = words
        .iter()
        .map(|w| {
            let spk = diar_index.assign(w.start, w.end);
            (w.clone(), spk)
        })
        .collect();

    // Сглаживание одиночных выбросов классификатора на границах реплик:
    if tagged.len() >= 3 {
        for i in 1..(tagged.len() - 1) {
            let prev_spk = tagged[i - 1].1;
            let next_spk = tagged[i + 1].1;
            let cur_spk = tagged[i].1;
            if prev_spk == next_spk && cur_spk != prev_spk {
                let gap_prev = (tagged[i].0.start - tagged[i - 1].0.end).max(0.0);
                let gap_next = (tagged[i + 1].0.start - tagged[i].0.end).max(0.0);
                if gap_prev < 0.25 && gap_next < 0.25 {
                    tagged[i].1 = prev_spk;
                }
            }
        }
    }

    let mut segs: Vec<Segment> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    let mut cur_spk = tagged[0].1;

    for (w, spk) in tagged {
        if let (Some(last), Some(first)) = (cur.last(), cur.first()) {
            let gap = (w.start - last.end).max(0.0);
            let dur = (last.end - first.start).max(0.0);

            let is_speaker_change = spk != cur_spk;
            let is_long_pause = gap > max_gap;
            let is_soft_split = dur >= SEG_IDEAL_DUR
                && (last.is_asr_boundary || ends_clause(&last.word) || gap >= 0.35);
            let is_hard_limit = dur >= max_dur;

            if is_speaker_change || is_long_pause || is_soft_split || is_hard_limit {
                segs.push(Segment {
                    start: first.start,
                    end: last.end,
                    text: join_words(&cur),
                    words: std::mem::take(&mut cur),
                    speaker: Some(cur_spk.to_string()),
                });
                cur_spk = spk;
            }
        }
        cur.push(w.clone());
        if ends_sentence(&w.word) {
            if let (Some(first), Some(last)) = (cur.first(), cur.last()) {
                segs.push(Segment {
                    start: first.start,
                    end: last.end,
                    text: join_words(&cur),
                    words: std::mem::take(&mut cur),
                    speaker: Some(cur_spk.to_string()),
                });
            }
        }
    }
    if !cur.is_empty() {
        if let (Some(first), Some(last)) = (cur.first(), cur.last()) {
            segs.push(Segment {
                start: first.start,
                end: last.end,
                text: join_words(&cur),
                words: cur,
                speaker: Some(cur_spk.to_string()),
            });
        }
    }

    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Turn;

    fn w(word: &str, start: f64, end: f64) -> Word {
        Word { word: word.into(), start, end, is_asr_boundary: false }
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

    #[test]
    fn diarization_splits_on_speaker_change_even_with_short_gap() {
        // Два спикера говорят с зазором 0.2с:
        // Спикер 0: "Привет" [0.0..0.4], "как" [0.4..0.6], "дела" [0.6..0.9]
        // Спикер 1: "Все" [1.1..1.3], "отлично" [1.3..1.8]
        let words = vec![
            w("Привет", 0.0, 0.4),
            w("как", 0.4, 0.6),
            w("дела", 0.6, 0.9),
            w("Все", 1.1, 1.3),
            w("отлично", 1.3, 1.8),
        ];
        let turns = vec![
            Turn { start: 0.0, end: 1.0, speaker: 0 },
            Turn { start: 1.0, end: 2.0, speaker: 1 },
        ];
        let segs = segment_words_with_diarization(&words, &turns, 0.8, 15.0);
        assert_eq!(segs.len(), 2, "Должно быть ровно 2 сегмента для двух спикеров");
        assert_eq!(segs[0].text, "Привет как дела");
        assert_eq!(segs[0].speaker.as_deref(), Some("0"));
        assert_eq!(segs[1].text, "Все отлично");
        assert_eq!(segs[1].speaker.as_deref(), Some("1"));
    }

    #[test]
    fn diarization_smooths_single_word_boundary_jitter() {
        // Спикер 0 говорит фразу, но одно слово из-за шума границы на 10мс попало в Turn другого:
        let words = vec![
            w("Я", 0.0, 0.3),
            w("иду", 0.35, 0.6),
            w("домой.", 0.65, 1.0),
        ];
        // turns с ложным микро-терном посредине
        let turns = vec![
            Turn { start: 0.0, end: 0.32, speaker: 0 },
            Turn { start: 0.33, end: 0.62, speaker: 1 }, // ложный джиттер на коротком слове
            Turn { start: 0.63, end: 1.2, speaker: 0 },
        ];
        let segs = segment_words_with_diarization(&words, &turns, 0.8, 15.0);
        assert_eq!(segs.len(), 1, "Одиночный джиттер должен сгладиться");
        assert_eq!(segs[0].text, "Я иду домой.");
        assert_eq!(segs[0].speaker.as_deref(), Some("0"));
    }
}
