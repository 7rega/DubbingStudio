//! DubbingSegmenter — сегментатор реплик для дубляжа (Natural / Dubbing Mode для Higgs v3 TTS).
//!
//! СООТВЕТСТВИЕ ТЗ:
//! 1. Устраняет искусственные разрывы фраз на 8-й секунде (диапазоны: 0–12.5с sweet spot, 12.5–14.0с target, >15с hard fallback).
//! 2. Двухуровневая архитектура: строгое разделение по speaker boundaries, затем фразовая сегментация внутри speaker turn.
//! 3. 100% language-agnostic (Unicode пунктуация, без внешних словарей и нейросетей).
//! 4. Совместимость с Classic pipeline и merge_short_turns().
//! 5. Подробный диагностический вывод (BoundaryDebugInfo).

use serde::{Deserialize, Serialize};

pub const DEFAULT_IDEAL_DURATION: f64 = 12.5;
pub const DEFAULT_TARGET_DURATION: f64 = 14.0;
pub const DEFAULT_HARD_DURATION: f64 = 15.0;

/// Слово с таймкодом и опциональным спикером.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WordWithTimestamp {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
}

impl WordWithTimestamp {
    pub fn new(word: impl Into<String>, start: f64, end: f64) -> Self {
        Self {
            word: word.into(),
            start,
            end,
            speaker: None,
        }
    }

    pub fn with_speaker(mut self, speaker: impl Into<String>) -> Self {
        self.speaker = Some(speaker.into());
        self
    }
}

impl From<crate::segment::Word> for WordWithTimestamp {
    fn from(w: crate::segment::Word) -> Self {
        Self {
            word: w.word,
            start: w.start,
            end: w.end,
            speaker: None,
        }
    }
}

impl From<WordWithTimestamp> for crate::segment::Word {
    fn from(w: WordWithTimestamp) -> Self {
        crate::segment::Word {
            word: w.word,
            start: w.start,
            end: w.end,
        }
    }
}

/// Сегмент дубляжа с привязанным спикером и пословными таймкодами.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DubbingSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: Option<String>,
    pub words: Vec<WordWithTimestamp>,
}

impl DubbingSegment {
    /// Конвертация в стандартный Segment крейта dub-asr (для совместимости).
    pub fn to_asr_segment(&self) -> crate::segment::Segment {
        crate::segment::Segment {
            start: self.start,
            end: self.end,
            text: self.text.clone(),
            words: self.words.iter().cloned().map(Into::into).collect(),
        }
    }
}

/// Непрерывный участок речи одного спикера (Speaker Turn).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerTurn {
    pub speaker: Option<String>,
    pub start: f64,
    pub end: f64,
    pub words: Vec<WordWithTimestamp>,
}

/// Конфигурация сегментатора Natural Mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmenterConfig {
    /// Идеальная верхняя граница длительности фразы (сек). До неё сплит не форсируется (дефолт 12.5с).
    pub ideal_duration: f64,
    /// Целевая граница длительности (сек). В диапазоне 12.5–14.0с активно ищется естественный конец мысли.
    pub target_duration: f64,
    /// Жесткий потолок длительности (сек). После 15.0с срабатывает аварийный fallback split.
    pub hard_duration: f64,
    /// Порог паузы для безусловного разрыва между сценами/репликами внутри одного спикера (сек).
    pub scene_silence_gap: f64,
    /// Минимальный порог скора для признания границы "хорошей" в окне 12.5–14.0с.
    pub good_score_threshold: f64,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            ideal_duration: DEFAULT_IDEAL_DURATION,
            target_duration: DEFAULT_TARGET_DURATION,
            hard_duration: DEFAULT_HARD_DURATION,
            scene_silence_gap: 1.8,
            good_score_threshold: 45.0,
        }
    }
}

/// Подробная диагностическая информация о выбранной границе сегмента (по ТЗ раздел 12).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryDebugInfo {
    pub segment_index: usize,
    pub speaker_id: Option<String>,
    pub speaker_turn_start: f64,
    pub speaker_turn_end: f64,
    pub segment_start: f64,
    pub segment_end: f64,
    pub segment_duration: f64,
    pub boundary_timestamp: f64,
    pub boundary_type: String,
    pub pause_duration: f64,
    pub punctuation: String,
    pub score: f64,
    pub reason: String,
    pub is_speaker_boundary: bool,
    pub is_phrase_split: bool,
    pub is_fallback: bool,
}

impl BoundaryDebugInfo {
    /// Человекочитаемый форматированный лог границы по стандарту ТЗ.
    pub fn format_debug(&self) -> String {
        format!(
            "Segment {}\nDuration: {:.1}s\nBoundary: {}\nPause: {:.2}s\nFallback: {}",
            self.segment_index + 1,
            self.segment_duration,
            self.boundary_type,
            self.pause_duration,
            self.is_fallback
        )
    }
}

/// Проверка терминальной пунктуации (сильные границы: . ! ? … 。 ！？ ؟ ۔ ։ । ॥).
pub fn is_terminal_punct(c: char) -> bool {
    matches!(
        c,
        '.' | '!' | '?' | '…'
            | '。' | '！' | '？'
            | '؟' | '۔'
            | '։' | '՜' | '՞'
            | '।' | '॥'
    )
}

/// Проверка промежуточной пунктуации (слабые границы: , ; : - — – 、 ， ； ：).
pub fn is_clause_punct(c: char) -> bool {
    matches!(
        c,
        ',' | ';' | ':' | '-' | '—' | '–'
            | '、' | '，' | '；' | '：'
            | '،' | '؛'
    )
}

/// Заканчивается ли слово сильным знаком препинания (конец предложения).
pub fn word_ends_sentence(word: &str) -> bool {
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
            || c == '›'
    });
    trimmed.ends_with(is_terminal_punct) || trimmed.ends_with("...")
}

/// Заканчивается ли слово слабым знаком препинания (клауза, перечисление).
pub fn word_ends_clause(word: &str) -> bool {
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
            || c == '›'
    });
    trimmed.ends_with(is_clause_punct)
}

/// Оценка качества потенциальной границы внутри одного speaker turn между словом `i` и `i+1`.
#[derive(Debug, Clone)]
struct BoundaryCandidate {
    pub word_index: usize,
    pub pause: f64,
    pub boundary_type: String,
    pub ends_sentence: bool,
    pub ends_clause: bool,
    pub score: f64,
    pub reason: String,
}

/// Сегментатор Natural / Dubbing Mode.
#[derive(Debug, Clone)]
pub struct DubbingSegmenter {
    pub config: SegmenterConfig,
}

impl Default for DubbingSegmenter {
    fn default() -> Self {
        Self::new()
    }
}

impl DubbingSegmenter {
    pub fn new() -> Self {
        Self {
            config: SegmenterConfig::default(),
        }
    }

    pub fn with_config(config: SegmenterConfig) -> Self {
        Self { config }
    }

    /// Разбить входной поток слов на изолированные Speaker Turn'ы (HARD BOUNDARY).
    pub fn partition_speaker_turns(&self, words: &[WordWithTimestamp]) -> Vec<SpeakerTurn> {
        if words.is_empty() {
            return Vec::new();
        }

        let mut turns: Vec<SpeakerTurn> = Vec::new();
        let mut cur_words: Vec<WordWithTimestamp> = Vec::new();
        let mut cur_speaker = words[0].speaker.clone();

        for w in words {
            if w.speaker != cur_speaker && !cur_words.is_empty() {
                let start = cur_words.first().unwrap().start;
                let end = cur_words.last().unwrap().end;
                turns.push(SpeakerTurn {
                    speaker: cur_speaker,
                    start,
                    end,
                    words: std::mem::take(&mut cur_words),
                });
                cur_speaker = w.speaker.clone();
            }
            cur_words.push(w.clone());
        }

        if !cur_words.is_empty() {
            let start = cur_words.first().unwrap().start;
            let end = cur_words.last().unwrap().end;
            turns.push(SpeakerTurn {
                speaker: cur_speaker,
                start,
                end,
                words: cur_words,
            });
        }

        turns
    }

    /// Оценка потенциальной границы между словом word_idx и следующим внутри одного turn.
    fn score_boundary(
        &self,
        words: &[WordWithTimestamp],
        word_idx: usize,
        segment_start: f64,
    ) -> BoundaryCandidate {
        let curr = &words[word_idx];
        let next = words.get(word_idx + 1);

        let pause = next.map_or(0.0, |nxt| (nxt.start - curr.end).max(0.0));
        let dur = (curr.end - segment_start).max(0.0);
        let ends_sent = word_ends_sentence(&curr.word);
        let ends_cl = word_ends_clause(&curr.word);

        let mut score = 0.0f64;
        let mut reasons = Vec::new();
        let boundary_type;

        // 1. Пунктуация (сильная граница . ! ? … -> наивысший приоритет)
        if ends_sent {
            boundary_type = "sentence_end".to_string();
            score += 60.0;
            reasons.push("sentence_end(+60)");
            if pause >= 0.20 {
                score += 20.0;
                reasons.push("sentence_end_with_pause(+20)");
            }
        } else if ends_cl {
            boundary_type = "clause".to_string();
            score += 25.0;
            reasons.push("clause_punct(+25)");
            if pause >= 0.20 {
                score += 10.0;
                reasons.push("clause_with_pause(+10)");
            }
        } else if pause >= 0.5 {
            boundary_type = "pause".to_string();
        } else {
            boundary_type = "natural_flow".to_string();
        }

        // 2. Пауза как сигнал
        if pause >= 1.0 {
            score += 35.0;
            reasons.push("long_pause(+35)");
        } else if pause >= 0.5 {
            score += 20.0;
            reasons.push("medium_pause(+20)");
        } else if pause >= 0.25 {
            score += 10.0;
            reasons.push("small_pause(+10)");
        } else if pause <= 0.03 && !ends_sent {
            score -= 25.0;
            reasons.push("continuous_speech_penalty(-25)");
        }

        // 3. Зоны длительности (sweet spot до 12.5с, target 12.5–14.0с)
        if (self.config.ideal_duration..=self.config.target_duration).contains(&dur) {
            score += 15.0;
            reasons.push("in_target_window(+15)");
        } else if dur > self.config.target_duration {
            let over = dur - self.config.target_duration;
            let penalty = (over * 8.0).min(30.0);
            score -= penalty;
            reasons.push("over_target_penalty");
        } else if dur < self.config.ideal_duration && !ends_sent {
            // Штраф за ранний разрыв, если нет естественного окончания мысли
            let early = (self.config.ideal_duration - dur) / self.config.ideal_duration;
            score -= early * 20.0;
            reasons.push("early_split_penalty");
        }

        // 4. Защита от создания огрызка в 1 слово на следующем шаге
        if let Some(_) = next {
            let remaining_words = words.len() - (word_idx + 1);
            if remaining_words == 1 && !ends_sent {
                score -= 30.0;
                reasons.push("orphan_tail_penalty(-30)");
            }
        }

        BoundaryCandidate {
            word_index: word_idx,
            pause,
            boundary_type,
            ends_sentence: ends_sent,
            ends_clause: ends_cl,
            score,
            reason: reasons.join(", "),
        }
    }

    /// Сегментация одного изолированного SpeakerTurn на DubbingSegment'ы.
    fn segment_speaker_turn(
        &self,
        turn: &SpeakerTurn,
        start_segment_index: usize,
    ) -> (Vec<DubbingSegment>, Vec<BoundaryDebugInfo>) {
        let words = &turn.words;
        if words.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let mut segments: Vec<DubbingSegment> = Vec::new();
        let mut debug_infos: Vec<BoundaryDebugInfo> = Vec::new();

        let n = words.len();
        let mut start_idx = 0usize;

        while start_idx < n {
            let cand_start = words[start_idx].start;

            let mut best_boundary: Option<BoundaryCandidate> = None;
            let mut chosen_end_idx = start_idx;
            let mut is_emergency_fallback = false;
            let mut is_turn_end = false;

            let mut j = start_idx;
            while j < n {
                let curr = &words[j];
                let dur = (curr.end - cand_start).max(0.0);
                let is_last_word_of_turn = j + 1 == n;

                // 1. Конец Speaker Turn — граница обязательна
                if is_last_word_of_turn {
                    chosen_end_idx = j;
                    is_turn_end = true;
                    let btype = if word_ends_sentence(&curr.word) {
                        "sentence_end".to_string()
                    } else {
                        "speaker_boundary".to_string()
                    };
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        pause: 0.0,
                        boundary_type: btype,
                        ends_sentence: word_ends_sentence(&curr.word),
                        ends_clause: word_ends_clause(&curr.word),
                        score: 100.0,
                        reason: "speaker_turn_end".to_string(),
                    });
                    break;
                }

                let next = &words[j + 1];
                let pause = (next.start - curr.end).max(0.0);

                // 2. Длинная межсценарная тишина внутри одного спикера (>= scene_silence_gap, напр. 1.8с)
                if pause >= self.config.scene_silence_gap
                    && (word_ends_sentence(&curr.word) || dur >= 3.0)
                {
                    chosen_end_idx = j;
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        pause,
                        boundary_type: "scene_silence".to_string(),
                        ends_sentence: word_ends_sentence(&curr.word),
                        ends_clause: word_ends_clause(&curr.word),
                        score: 90.0,
                        reason: "scene_silence_gap".to_string(),
                    });
                    break;
                }

                // Оцениваем кандидата j
                let candidate = self.score_boundary(words, j, cand_start);

                // 3. Зона < ideal_duration (0–12.5с):
                // Накапливаем слова, не режем фразу из-за времени внутри turn.
                if dur < self.config.ideal_duration {
                    if best_boundary.as_ref().map_or(true, |b| candidate.score > b.score) {
                        best_boundary = Some(candidate);
                    }
                    j += 1;
                    continue;
                }

                // 4. Зона 12.5–14.0с:
                // При нахождении естественного завершения мысли (конец предложения / высокий скор) — фиксируем разрез!
                if dur <= self.config.target_duration {
                    if candidate.ends_sentence
                        || candidate.score >= self.config.good_score_threshold
                    {
                        chosen_end_idx = j;
                        best_boundary = Some(candidate);
                        break;
                    }
                    if best_boundary.as_ref().map_or(true, |b| candidate.score > b.score) {
                        best_boundary = Some(candidate);
                    }
                    j += 1;
                    continue;
                }

                // 5. Зона 14.0–15.0с (Lookahead поиск):
                // Активно ищем лучшую доступную границу
                if dur <= self.config.hard_duration {
                    if candidate.ends_sentence || candidate.ends_clause || candidate.pause >= 0.35
                    {
                        chosen_end_idx = j;
                        best_boundary = Some(candidate);
                        break;
                    }
                    if best_boundary.as_ref().map_or(true, |b| candidate.score > b.score) {
                        best_boundary = Some(candidate);
                    }
                    j += 1;
                    continue;
                }

                // 6. Зона > 15.0с: Превышен Hard Limit (Emergency Fallback)
                is_emergency_fallback = true;
                if let Some(b) = best_boundary.take() {
                    if b.word_index >= start_idx {
                        chosen_end_idx = b.word_index;
                        best_boundary = Some(BoundaryCandidate {
                            boundary_type: "fallback".to_string(),
                            reason: format!(
                                "emergency_fallback_hard_limit (picked best: {})",
                                b.reason
                            ),
                            ..b
                        });
                    } else {
                        chosen_end_idx = j;
                        best_boundary = Some(BoundaryCandidate {
                            boundary_type: "fallback".to_string(),
                            ..candidate
                        });
                    }
                } else {
                    chosen_end_idx = j;
                    best_boundary = Some(BoundaryCandidate {
                        boundary_type: "fallback".to_string(),
                        ..candidate
                    });
                }
                break;
            }

            // Формируем DubbingSegment строго для текущего speaker turn
            let seg_words: Vec<WordWithTimestamp> =
                words[start_idx..=chosen_end_idx].to_vec();

            let text = seg_words
                .iter()
                .map(|w| w.word.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();

            let seg_start = seg_words.first().unwrap().start;
            let seg_end = seg_words.last().unwrap().end;
            let seg_dur = (seg_end - seg_start).max(0.0);

            let bound_info = best_boundary.unwrap_or_else(|| BoundaryCandidate {
                word_index: chosen_end_idx,
                pause: 0.0,
                boundary_type: "fallback".to_string(),
                ends_sentence: false,
                ends_clause: false,
                score: 0.0,
                reason: "default_fallback".to_string(),
            });

            let global_seg_idx = start_segment_index + segments.len();

            debug_infos.push(BoundaryDebugInfo {
                segment_index: global_seg_idx,
                speaker_id: turn.speaker.clone(),
                speaker_turn_start: turn.start,
                speaker_turn_end: turn.end,
                segment_start: seg_start,
                segment_end: seg_end,
                segment_duration: seg_dur,
                boundary_timestamp: seg_end,
                boundary_type: bound_info.boundary_type,
                pause_duration: bound_info.pause,
                punctuation: words[chosen_end_idx]
                    .word
                    .chars()
                    .last()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                score: bound_info.score,
                reason: bound_info.reason,
                is_speaker_boundary: is_turn_end,
                is_phrase_split: !is_turn_end,
                is_fallback: is_emergency_fallback,
            });

            segments.push(DubbingSegment {
                start: seg_start,
                end: seg_end,
                text,
                speaker: turn.speaker.clone(),
                words: seg_words,
            });

            start_idx = chosen_end_idx + 1;
        }

        (segments, debug_infos)
    }

    /// Основная точка входа: двухуровневая сегментация (Speaker Turns -> Phrase Segments).
    pub fn segment_with_debug(
        &self,
        words: &[WordWithTimestamp],
    ) -> (Vec<DubbingSegment>, Vec<BoundaryDebugInfo>) {
        if words.is_empty() {
            return (Vec::new(), Vec::new());
        }

        // Шаг 1: Разделение на непрерывные Speaker Turn'ы
        let turns = self.partition_speaker_turns(words);

        // Шаг 2: Фразовая сегментация строго внутри каждого Speaker Turn
        let mut all_segments: Vec<DubbingSegment> = Vec::new();
        let mut all_debug_infos: Vec<BoundaryDebugInfo> = Vec::new();

        for turn in &turns {
            let (turn_segs, turn_debugs) =
                self.segment_speaker_turn(turn, all_segments.len());
            all_segments.extend(turn_segs);
            all_debug_infos.extend(turn_debugs);
        }

        (all_segments, all_debug_infos)
    }

    /// Собрать слова в DubbingSegment'ы.
    pub fn segment(&self, words: &[WordWithTimestamp]) -> Vec<DubbingSegment> {
        let (segs, _) = self.segment_with_debug(words);
        segs
    }

    /// Обратная совместимость: возвращает Vec<crate::segment::Segment>.
    pub fn segment_as_asr_segments(&self, words: &[WordWithTimestamp]) -> Vec<crate::segment::Segment> {
        self.segment(words)
            .into_iter()
            .map(|s| s.to_asr_segment())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(word: &str, start: f64, end: f64) -> WordWithTimestamp {
        WordWithTimestamp::new(word, start, end)
    }

    fn spk_w(word: &str, start: f64, end: f64, spk: &str) -> WordWithTimestamp {
        WordWithTimestamp::new(word, start, end).with_speaker(spk)
    }

    fn assert_one_speaker_per_segment(segs: &[DubbingSegment]) {
        for (i, s) in segs.iter().enumerate() {
            for (w_i, w) in s.words.iter().enumerate() {
                assert_eq!(
                    w.speaker, s.speaker,
                    "Сегмент #{} ({:?}) содержит слово #{} с чужим спикером {:?}",
                    i, s.speaker, w_i, w.speaker
                );
            }
        }
    }

    /// TEST 1: Короткая реплика <12.5 сек -> 1 segment.
    #[test]
    fn test_1_short_phrase_under_12_5s() {
        let words = vec![
            w("Hello", 0.0, 0.4),
            w("everyone,", 0.5, 1.0),
            w("welcome", 1.1, 1.5),
            w("to", 1.6, 1.7),
            w("DubbingStudio.", 1.8, 2.5),
        ];
        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Hello everyone, welcome to DubbingStudio.");
        assert_eq!(segs[0].start, 0.0);
        assert_eq!(segs[0].end, 2.5);
    }

    /// TEST 2: 8 секунд не являются hard limit (фраза 10–11 сек без естественной границы до 8 сек -> не резать на 8-й сек).
    #[test]
    fn test_2_eight_seconds_is_not_hard_limit() {
        let mut words = Vec::new();
        // 22 слова по ~0.48с -> общая длительность 10.5с без знаков препинания
        for i in 0..22 {
            let st = i as f64 * 0.48;
            let en = st + 0.44;
            words.push(w("continuous", st, en));
        }
        assert!((words.last().unwrap().end - words.first().unwrap().start) > 10.0);

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1, "Фраза 10.5с не должна разрезаться на 8.0с");
        assert!((segs[0].end - segs[0].start) > 10.0);
    }

    /// TEST 3: Естественное окончание фразы около 12–13 сек -> split/end на sentence boundary.
    #[test]
    fn test_3_natural_sentence_boundary_at_12_13s() {
        let mut words = Vec::new();
        // Предложение 1 (0.0 .. 12.8с)
        for i in 0..20 {
            let st = i as f64 * 0.64;
            let en = st + 0.55;
            let word = if i == 19 { "completed." } else { "speech" };
            words.push(w(word, st, en));
        }
        // Предложение 2 (13.4 .. 17.0с)
        for i in 0..8 {
            let st = 13.4 + i as f64 * 0.45;
            let en = st + 0.40;
            let word = if i == 7 { "done." } else { "next" };
            words.push(w(word, st, en));
        }

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].text.ends_with("completed."));
        assert!(segs[1].text.starts_with("next"));
        assert_eq!(debug[0].boundary_type, "sentence_end");
        assert!(!debug[0].is_fallback);
    }

    /// TEST 4: Длинная цельная фраза около 14 сек -> сохранить как один segment, если есть естественное завершение.
    #[test]
    fn test_4_long_complete_phrase_around_14s() {
        let mut words = Vec::new();
        for i in 0..24 {
            let st = i as f64 * 0.58;
            let en = st + 0.52;
            let word = if i == 23 { "finished." } else { "word" };
            words.push(w(word, st, en));
        }
        let total_dur = words.last().unwrap().end - words.first().unwrap().start;
        assert!(total_dur >= 13.5 && total_dur <= 14.2);

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1, "Цельная фраза 13.9с с точкой в конце должна сохраняться единым сегментом");
    }

    /// TEST 5: Длинная фраза >14 сек -> найти естественную границу, а не резать механически ровно на 14 сек.
    #[test]
    fn test_5_long_phrase_over_14s_finds_natural_boundary() {
        let mut words = Vec::new();
        // Слова до естественной границы на 13.6с (запятая + пауза)
        for i in 0..20 {
            let st = i as f64 * 0.68;
            let en = st + 0.58;
            let word = if i == 19 { "statement," } else { "talking" };
            words.push(w(word, st, en));
        }
        // Продолжение мысли до 18 сек
        let t_split = words.last().unwrap().end;
        for i in 0..8 {
            let st = t_split + 0.45 + i as f64 * 0.5;
            let en = st + 0.45;
            words.push(w("continuation", st, en));
        }

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);
        assert!(segs.len() >= 2);
        assert!(segs[0].text.ends_with("statement,"));
        assert_eq!(debug[0].boundary_type, "clause");
    }

    /// TEST 6: Отсутствует хорошая граница (>15 сек) -> fallback split около hard limit.
    #[test]
    fn test_6_no_good_boundary_over_15s_uses_fallback() {
        let mut words = Vec::new();
        for i in 0..30 {
            let st = i as f64 * 0.58;
            let en = st + 0.54;
            words.push(w("unbroken", st, en));
        }
        assert!((words.last().unwrap().end - words.first().unwrap().start) > 16.5);

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);
        assert!(segs.len() >= 2);
        assert!((segs[0].end - segs[0].start) <= 15.0);
        assert!(debug[0].is_fallback);
    }

    /// TEST 7: Длинная пауза внутри фразы (вдох 0.7с) -> не ломает грамматически связанную реплику < 12.5с.
    #[test]
    fn test_7_pause_inside_phrase_does_not_break_connected_speech() {
        let words = vec![
            w("I", 0.0, 0.4),
            w("wanted", 0.5, 0.9),
            w("to", 1.0, 1.2),
            w("tell", 1.3, 1.6),
            w("you", 1.7, 2.0),
            w("that", 2.1, 2.5),
            // пауза на вдох 0.75с посреди предложения
            w("we", 3.25, 3.6),
            w("should", 3.7, 4.0),
            w("leave", 4.1, 4.5),
            w("early.", 4.6, 5.2),
        ];

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1, "Пауза вдоха 0.75с внутри связной мысли не должна вызывать разрыв");
        assert_eq!(segs[0].text, "I wanted to tell you that we should leave early.");
    }

    /// TEST 8: Пунктуация (. ! ? …) как сильные границы.
    #[test]
    fn test_8_punctuation_marks_as_strong_boundaries() {
        let puncts = vec![".", "!", "?", "…"];
        let segmenter = DubbingSegmenter::new();

        for p in puncts {
            let words = vec![
                w(&format!("Stop{}", p), 0.0, 0.5),
                w("Next", 0.8, 1.2),
                w(&format!("sentence{}", p), 1.3, 2.0),
            ];
            assert!(word_ends_sentence(&words[0].word));
            assert!(word_ends_sentence(&words[2].word));
            let (segs, _) = segmenter.segment_with_debug(&words);
            assert_eq!(segs.len(), 1); // < 12.5с группирует короткие предложения, если нет большой паузы
        }
    }

    /// TEST 9: Разные языки: English, Russian, Chinese, Japanese (без языковых словарей).
    #[test]
    fn test_9_multilingual_no_dictionaries_en_ru_zh_ja() {
        let segmenter = DubbingSegmenter::new();

        // 1. English
        let en = vec![
            w("The", 0.0, 0.3),
            w("quick", 0.4, 0.8),
            w("brown", 0.9, 1.3),
            w("fox.", 1.4, 1.9),
        ];
        let en_s = segmenter.segment(&en);
        assert_eq!(en_s.len(), 1);
        assert_eq!(en_s[0].text, "The quick brown fox.");

        // 2. Russian
        let ru = vec![
            w("Мы", 0.0, 0.4),
            w("сохраняем", 0.5, 1.2),
            w("цельные", 1.3, 1.8),
            w("фразы.", 1.9, 2.5),
        ];
        let ru_s = segmenter.segment(&ru);
        assert_eq!(ru_s.len(), 1);
        assert_eq!(ru_s[0].text, "Мы сохраняем цельные фразы.");

        // 3. Chinese (CJK 。 and ，)
        let zh = vec![
            w("这是", 0.0, 0.5),
            w("一个，", 0.6, 1.0),
            w("完整句子。", 1.1, 1.8),
        ];
        let zh_s = segmenter.segment(&zh);
        assert_eq!(zh_s.len(), 1);
        assert_eq!(zh_s[0].text, "这是 一个， 完整句子。");

        // 4. Japanese (CJK 、 and 。)
        let ja = vec![
            w("これ", 0.0, 0.4),
            w("は、", 0.5, 0.8),
            w("自然な音声です。", 0.9, 1.8),
        ];
        let ja_s = segmenter.segment(&ja);
        assert_eq!(ja_s.len(), 1);
        assert_eq!(ja_s[0].text, "これ は、 自然な音声です。");
    }

    /// TEST 10: Существующие speaker boundaries (не объединяет уже разделённые speaker segments).
    #[test]
    fn test_10_existing_speaker_boundaries_preserved() {
        let words = vec![
            spk_w("Hello", 0.0, 0.4, "A"),
            spk_w("there.", 0.5, 0.9, "A"),
            spk_w("Hi!", 1.0, 1.4, "B"),
            spk_w("How", 1.5, 1.8, "B"),
            spk_w("are", 1.9, 2.1, "B"),
            spk_w("you?", 2.2, 2.6, "B"),
            spk_w("Good.", 2.8, 3.2, "A"),
        ];

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);

        assert_eq!(segs.len(), 3, "A -> B -> A обязано дать 3 независимых сегмента");
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[0].text, "Hello there.");
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[1].text, "Hi! How are you?");
        assert_eq!(segs[2].speaker.as_deref(), Some("A"));
        assert_eq!(segs[2].text, "Good.");

        assert_one_speaker_per_segment(&segs);
        assert!(debug[0].is_speaker_boundary);
        assert!(debug[1].is_speaker_boundary);
        assert!(debug[2].is_speaker_boundary);
    }
}
