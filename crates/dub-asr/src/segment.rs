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

/// Сокращения, за которыми фраза всегда продолжается (титул перед именем, «т.е.», «z.B.»). Нижний регистр, с точкой.
pub const PREFIX_ABBREVIATIONS: &[&str] = &[
    // en
    "mr.", "mrs.", "ms.", "dr.", "st.", "sr.", "prof.", "vs.", "e.g.", "i.e.",
    // ru
    "т.е.", "ул.",
    // de
    "z.b.", "bzw.", "hr.", "fr.",
    // fr
    "mme.",
    // es, pt
    "sra.", "dra.", "srta.",
];

/// Сокращения, которыми предложение может и закончиться («… и т.д.», «at 5 p.m.»): конец предложения
/// только перед словом с заглавной буквы или в конце потока.
pub const TERMINAL_ABBREVIATIONS: &[&str] = &[
    // en
    "etc.", "a.m.", "p.m.", "jr.", "no.",
    // ru
    "т.д.", "т.п.", "г.", "гг.", "им.", "др.", "стр.", "рис.", "см.",
    // de
    "u.a.", "usw.", "nr.",
];

/// Открывающие кавычки и скобки перед словом.
const OPENING: &[char] = &['"', '\'', '«', '„', '“', '‘', '(', '[', '{', '¿', '¡'];

/// Закрывающие кавычки и скобки после слова.
const CLOSING: &[char] = &['"', '\'', '»', '”', '’', ')', ']', '}'];

/// Одна заглавная буква с точкой — инициал («J.», «А.»). «I.» — английское местоимение в конце фразы.
fn is_initial(core: &str) -> bool {
    let mut chars = core.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some('.'), None) if c.is_uppercase() && c != 'I')
}

/// Буквы через точку, не меньше двух: «U.S.», «a.m.».
fn is_dotted_letters(core: &str) -> bool {
    let chars: Vec<char> = core.chars().collect();
    chars.len() >= 4
        && chars.len() % 2 == 0
        && chars.iter().enumerate().all(|(i, c)| if i % 2 == 0 { c.is_alphabetic() } else { *c == '.' })
}

/// Кончается ли на слове `word` предложение; `next` — следующее слово потока (None если слово последнее).
pub fn ends_sentence(word: &str, next: Option<&str>) -> bool {
    let trimmed = word.trim_end().trim_end_matches(|c: char| CLOSING.contains(&c));
    if !trimmed.ends_with(['.', '!', '?', '…', '。', '！', '？', '؟', '۔']) && !trimmed.ends_with("...") {
        return false;
    }
    // Восклицательный/вопросительный знаки, многоточие или специальные символы всегда завершают фразу
    if !trimmed.ends_with('.') || trimmed.ends_with("..") {
        return true;
    }
    let core = trimmed.trim_start_matches(OPENING);
    let lower = core.to_lowercase();
    let next_first = next.and_then(|n| n.trim_start_matches(OPENING).chars().next());
    let next_capital = next_first.map_or(true, char::is_uppercase);
    let next_digit = next_first.map_or(false, |c| c.is_ascii_digit());

    if PREFIX_ABBREVIATIONS.contains(&lower.as_str()) || is_initial(core) {
        return false;
    }
    if TERMINAL_ABBREVIATIONS.contains(&lower.as_str()) || is_dotted_letters(core) {
        return next_capital;
    }
    let before_dot = core.trim_end_matches('.').chars().last();
    if before_dot.map_or(false, |c| c.is_ascii_digit()) && next_digit {
        return false;
    }
    true
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

    for (i, w) in words.iter().enumerate() {
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
        let next_w = words.get(i + 1).map(|nw| nw.word.as_str());
        if ends_sentence(&w.word, next_w) {
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
/// 1. Защита слитной речи: слова внутри непрерывного речевого потока (gap < 0.20с без знаков препинания)
///    НЕ разрываются из-за единичного шума классификатора Sortformer на 1 слове.
/// 2. Смена спикера разрешается на естественных границах: знаки препинания (.!?…), клаузы (,, ;, :)
///    или при паузе между репликами gap >= 0.20с.
/// 3. Итоговый спикер каждого сегмента определяется по доминантному перекрытию всего интервала [start, end]
///    с интервалами диаризации через DiarIndex.
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

    // Сглаживание одиночных выбросов классификатора на границах слов:
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

    let mut raw_segs: Vec<Vec<Word>> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    let mut cur_spk = tagged[0].1;

    for (i, (w, spk)) in tagged.iter().enumerate() {
        if let (Some(last), Some(first)) = (cur.last(), cur.first()) {
            let gap = (w.start - last.end).max(0.0);
            let dur = (last.end - first.start).max(0.0);

            // 1. Пауза больше допустимого (напр. >0.8с)
            let is_long_pause = gap > max_gap;

            // 2. Смена спикера: разрешается ТОЛЬКО если есть пауза между словами (gap >= 0.20с) ИЛИ
            //    предыдущее слово завершило законченное предложение (.!?…). Слитная речь защищена
            //    от ложного разрыва на запятых и клаузах из-за единичных флуктуаций классификатора.
            let next_last = Some(w.word.as_str());
            let is_speaker_change = *spk != cur_spk && (gap >= 0.20 || ends_sentence(&last.word, next_last));

            // 3. Жёлтая зона (12–15с): мягкий разрыв по клаузе/VAD/паузе
            let is_soft_split = dur >= SEG_IDEAL_DUR
                && (last.is_asr_boundary || ends_clause(&last.word) || gap >= 0.35);

            // 4. Жёсткий лимит (>15с)
            let is_hard_limit = dur >= max_dur;

            if is_speaker_change || is_long_pause || is_soft_split || is_hard_limit {
                raw_segs.push(std::mem::take(&mut cur));
                cur_spk = *spk;
            }
        }
        cur.push(w.clone());
        let next_w = tagged.get(i + 1).map(|t| t.0.word.as_str());
        if ends_sentence(&w.word, next_w) {
            raw_segs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        raw_segs.push(cur);
    }

    // Для каждого сегмента определяем доминантного спикера по всему интервалу [start, end].
    // Акустические пословные метки Whisper являются абсолютным авторитетом для начала и конца речи.
    raw_segs
        .into_iter()
        .filter(|ws| !ws.is_empty())
        .map(|mut ws| {
            let start = ws.first().unwrap().start;
            let mut end = ws.last().unwrap().end;
            let spk = diar_index.assign(start, end);

            // Защита от багов отдельных ASR-моделей (напр. Parakeet), когда последнее слово аномально
            // растянуто в тишину (>2.0с длительности одного слова и вылезает далеко за turn_end).
            // Обычные слова никогда не обрезаются.
            if let Some(last_w) = ws.last_mut() {
                if let Some(turn_end) = diar_index.active_turn_end(start, spk) {
                    if last_w.end > turn_end + 1.0 && (last_w.end - last_w.start) > 2.0 {
                        let clamped = (turn_end + 0.20).max(last_w.start + 0.30);
                        if clamped < last_w.end {
                            last_w.end = clamped;
                            end = clamped;
                        }
                    }
                }
            }
            Segment {
                start,
                end,
                text: join_words(&ws),
                words: ws,
                speaker: Some(spk.to_string()),
            }
        })
        .collect()
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
    fn diarization_protects_continuous_speech_from_mid_phrase_split() {
        // Слитная фраза "Shut the fuck up." без пауз и пунктуации внутри, но с шумом в turns
        let words = vec![
            w("Shut", 11.0, 11.2),
            w("the", 11.21, 11.3),
            w("fuck", 11.31, 11.5),
            w("up.", 11.51, 11.8),
        ];
        let turns = vec![
            Turn { start: 10.0, end: 11.22, speaker: 1 },
            Turn { start: 11.22, end: 12.0, speaker: 4 }, // шум классификатора посреди слитной фразы
        ];
        let segs = segment_words_with_diarization(&words, &turns, 0.8, 15.0);
        assert_eq!(segs.len(), 1, "Слитная фраза не должна рваться на полуслове");
        assert_eq!(segs[0].text, "Shut the fuck up.");
    }

    #[test]
    fn abbreviations_and_initials_do_not_split() {
        let words = vec![
            w("Dr.", 0.0, 0.3),
            w("Watson", 0.35, 0.8),
            w("bought", 0.85, 1.2),
            w("3.5", 1.25, 1.6),
            w("apples.", 1.65, 2.1),
        ];
        let segs = segment_words(&words, 0.8, 15.0);
        assert_eq!(segs.len(), 1, "Титул 'Dr.' и дробь '3.5' не должны дробить предложение: {segs:?}");
        assert_eq!(segs[0].text, "Dr. Watson bought 3.5 apples.");

        let words_ru = vec![
            w("Это", 0.0, 0.3),
            w("т.е.", 0.35, 0.7),
            w("правда.", 0.75, 1.2),
        ];
        let segs_ru = segment_words(&words_ru, 0.8, 15.0);
        assert_eq!(segs_ru.len(), 1, "Сокращение 'т.е.' не должно дробить: {segs_ru:?}");
    }

    #[test]
    fn diarization_clamps_stretched_tail_to_speaker_turn() {
        // Симулируем баг Parakeet: спикер 0 закончил говорить на 15.3с, но слово "There!" растянуто до 24.9с
        let words = vec![
            w("There!", 13.5, 24.9),
        ];
        let turns = vec![
            Turn { start: 13.4, end: 15.3, speaker: 0 },
            Turn { start: 24.9, end: 27.0, speaker: 1 },
        ];
        let segs = segment_words_with_diarization(&words, &turns, 0.8, 15.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "There!");
        assert!(segs[0].end <= 15.5, "Конец реплики должен быть притянут к turn.end (<=15.5с), факт: {}", segs[0].end);
        assert!(segs[0].words[0].end <= 15.5, "Конец слова также должен быть ограничен, факт: {}", segs[0].words[0].end);
    }
}
