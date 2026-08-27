//! DubbingSegmenter — экспериментальный language-agnostic сегментатор для дубляжа (TTS / Higgs v3).
//!
//! ДВУХУРОВНЕВАЯ АРХИТЕКТУРА:
//! 1. ASR Words -> СТРОГОЕ разделение на Speaker Turns (границы спикеров — абсолютный HARD BOUNDARY).
//! 2. Phrase Segmentation внутри каждого Speaker Turn (sweet spot 0–12.5с, target 14.0с, hard cap 15.0с).
//!
//! ГАРАНТИЯ: ONE DUBBING SEGMENT = ONE SPEAKER. Ни один сегмент не может содержать слова разных спикеров.

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
    /// Конвертация в стандартный Segment крейта dub-asr (для обратной совместимости).
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

/// Конфигурация DubbingSegmenter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmenterConfig {
    /// Идеальная верхняя граница длительности фразы (сек). До неё сплит не форсируется.
    pub ideal_duration: f64,
    /// Целевая граница длительности (сек). В диапазоне 12.5–14.0с активно ищется хорошая граница.
    pub target_duration: f64,
    /// Жесткий потолок длительности (сек). После 15.0с срабатывает emergency fallback.
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

/// Диагностическая информация о выбранной границе сегмента.
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
    pub pause_duration: f64,
    pub punctuation: String,
    pub score: f64,
    pub reason: String,
    pub is_speaker_boundary: bool,
    pub is_phrase_split: bool,
    pub is_fallback: bool,
}

/// Проверка терминальной пунктуации (завершение предложения) на любых языках (Unicode-agnostic).
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

/// Проверка промежуточной пунктуации (клаузы, паузы, перечисления).
pub fn is_clause_punct(c: char) -> bool {
    matches!(
        c,
        ',' | ';' | ':' | '-' | '—' | '–'
            | '、' | '，' | '；' | '：'
            | '،' | '؛'
    )
}

/// Заканчивается ли слово знаком конца предложения.
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

/// Заканчивается ли слово промежуточным знаком препинания.
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
    pub ends_sentence: bool,
    pub ends_clause: bool,
    pub score: f64,
    pub reason: String,
}

/// Language-agnostic сегментатор для дубляжа со строгими границами спикеров.
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

    /// Разделить входной поток слов на непрерывные SpeakerTurn'ы (HARD BOUNDARY).
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

    /// Оценить одну потенциальную границу после слова word_idx внутри одного turn.
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

        // 1. Терминальная пунктуация (. ! ? …) — наивысший приоритет
        if ends_sent {
            score += 60.0;
            reasons.push("sentence_end(+60)");
            if pause >= 0.20 {
                score += 20.0;
                reasons.push("sentence_end_with_pause(+20)");
            }
        } else if ends_cl {
            score += 25.0;
            reasons.push("clause_punct(+25)");
            if pause >= 0.20 {
                score += 10.0;
                reasons.push("clause_with_pause(+10)");
            }
        }

        // 2. Пауза
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

        // 3. Фактор близости к целевому хронометражу (12.5–14.0с)
        if (self.config.ideal_duration..=self.config.target_duration).contains(&dur) {
            score += 15.0;
            reasons.push("in_target_window(+15)");
        } else if dur > self.config.target_duration {
            let over = dur - self.config.target_duration;
            let penalty = (over * 8.0).min(30.0);
            score -= penalty;
            reasons.push("over_target_penalty");
        } else if dur < self.config.ideal_duration {
            // Штраф за ранний сплит, если нет сильного завершения предложения
            if !ends_sent {
                let early = (self.config.ideal_duration - dur) / self.config.ideal_duration;
                score -= early * 20.0;
                reasons.push("early_split_penalty");
            }
        }

        // 4. Защита от создания одинокого огрызка в 1 слово на следующем шаге
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
            ends_sentence: ends_sent,
            ends_clause: ends_cl,
            score,
            reason: reasons.join(", "),
        }
    }

    /// Сегментировать один изолированный SpeakerTurn на DubbingSegment'ы.
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

                // 1. Конец speaker turn: граница обязательна
                if is_last_word_of_turn {
                    chosen_end_idx = j;
                    is_turn_end = true;
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        pause: 0.0,
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
                // Если текущая граница хорошая (конец предложения или высокий скор) — фиксируем сплит!
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

                // 5. Зона 14.0–15.0с (Lookahead):
                // Активно ищем лучшую границу
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
                            reason: format!(
                                "emergency_fallback_hard_limit (picked best: {})",
                                b.reason
                            ),
                            ..b
                        });
                    } else {
                        chosen_end_idx = j;
                        best_boundary = Some(candidate);
                    }
                } else {
                    chosen_end_idx = j;
                    best_boundary = Some(candidate);
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

    /// Проверка фундаментального инварианта: ONE DUBBING SEGMENT = ONE SPEAKER
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

    /// CASE 1:
    /// A: "Hello, I wanted to tell you something."
    /// B: "What?"
    /// A: "It's important."
    /// Обязательно минимум 3 отдельных dubbing segments. Никакого объединения A+B+A.
    #[test]
    fn case_1_aba_dialogue_must_not_merge() {
        let words = vec![
            spk_w("Hello,", 0.0, 0.4, "A"),
            spk_w("I", 0.5, 0.7, "A"),
            spk_w("wanted", 0.8, 1.2, "A"),
            spk_w("to", 1.3, 1.4, "A"),
            spk_w("tell", 1.5, 1.8, "A"),
            spk_w("you", 1.9, 2.1, "A"),
            spk_w("something.", 2.2, 3.0, "A"),
            spk_w("What?", 3.2, 3.8, "B"),
            spk_w("It's", 4.0, 4.3, "A"),
            spk_w("important.", 4.4, 5.2, "A"),
        ];

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);

        assert_eq!(segs.len(), 3, "A -> B -> A обязано дать ровно 3 сегмента");
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[0].text, "Hello, I wanted to tell you something.");
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[1].text, "What?");
        assert_eq!(segs[2].speaker.as_deref(), Some("A"));
        assert_eq!(segs[2].text, "It's important.");

        assert_one_speaker_per_segment(&segs);
        assert!(debug[0].is_speaker_boundary);
        assert!(debug[1].is_speaker_boundary);
        assert!(debug[2].is_speaker_boundary);
    }

    /// CASE 2: A говорит 10–13 секунд одной законченной фразой -> один segment A.
    #[test]
    fn case_2_single_speaker_long_phrase() {
        let mut words = Vec::new();
        for i in 0..20 {
            let st = i as f64 * 0.55;
            let en = st + 0.50;
            let word = if i == 19 { "finished." } else { "speech" };
            words.push(spk_w(word, st, en, "A"));
        }

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_one_speaker_per_segment(&segs);
    }

    /// CASE 3: A говорит 17 секунд одной фразой -> разбивается на несколько segments A по естественным границам.
    #[test]
    fn case_3_single_speaker_over_hard_limit() {
        let mut words = Vec::new();
        for i in 0..32 {
            let st = i as f64 * 0.55;
            let en = st + 0.50;
            let word = if i == 20 { "pause," } else { "word" };
            words.push(spk_w(word, st, en, "A"));
        }

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert!(segs.len() >= 2);
        for s in &segs {
            assert_eq!(s.speaker.as_deref(), Some("A"));
            assert!((s.end - s.start) <= 15.0);
        }
        assert_one_speaker_per_segment(&segs);
    }

    /// CASE 4: A говорит 6 секунд, B говорит 0.5 секунды, A говорит ещё 6 секунд -> A (6s), B (0.5s), A (6s).
    #[test]
    fn case_4_short_b_between_long_a_no_merge() {
        let mut words = Vec::new();
        // A: 0..6s
        for i in 0..10 {
            words.push(spk_w("speechA", i as f64 * 0.6, i as f64 * 0.6 + 0.55, "A"));
        }
        // B: 6.2..6.7s
        words.push(spk_w("Uh-huh.", 6.2, 6.7, "B"));
        // A: 6.9..12.9s
        for i in 0..10 {
            words.push(spk_w("speechA2", 6.9 + i as f64 * 0.6, 6.9 + i as f64 * 0.6 + 0.55, "A"));
        }

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 3, "A(6s) -> B(0.5s) -> A(6s) обязано оставаться 3 сегментами");
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[2].speaker.as_deref(), Some("A"));
        assert_one_speaker_per_segment(&segs);
    }

    /// CASE 5: Короткие реплики одного спикера (A: "Yes.", A: "I know.") объединяются,
    /// но A: "Yes.", B: "What?", A: "I know." никогда не объединяются.
    #[test]
    fn case_5_short_turns_same_vs_diff_speakers() {
        let segmenter = DubbingSegmenter::new();

        // 1. Одинаковый спикер A подряд -> объединяются в 1 фразу для комфортного TTS
        let same_words = vec![
            spk_w("Yes.", 0.0, 0.4, "A"),
            spk_w("I", 0.6, 0.9, "A"),
            spk_w("know.", 1.0, 1.4, "A"),
        ];
        let same_segs = segmenter.segment(&same_words);
        assert_eq!(same_segs.len(), 1);
        assert_eq!(same_segs[0].text, "Yes. I know.");
        assert_one_speaker_per_segment(&same_segs);

        // 2. Спикер B посредине -> 3 отдельных сегмента
        let diff_words = vec![
            spk_w("Yes.", 0.0, 0.4, "A"),
            spk_w("What?", 0.6, 1.0, "B"),
            spk_w("I", 1.2, 1.5, "A"),
            spk_w("know.", 1.6, 2.0, "A"),
        ];
        let diff_segs = segmenter.segment(&diff_words);
        assert_eq!(diff_segs.len(), 3);
        assert_eq!(diff_segs[0].text, "Yes.");
        assert_eq!(diff_segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(diff_segs[1].text, "What?");
        assert_eq!(diff_segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(diff_segs[2].text, "I know.");
        assert_eq!(diff_segs[2].speaker.as_deref(), Some("A"));
        assert_one_speaker_per_segment(&diff_segs);
    }

    /// CASE 6: Speaker boundary внутри грамматически единого предложения -> speaker boundary важнее грамматики.
    #[test]
    fn case_6_sentence_cut_by_speaker_turn() {
        let words = vec![
            spk_w("I", 0.0, 0.3, "A"),
            spk_w("wanted", 0.4, 0.8, "A"),
            spk_w("to", 0.9, 1.0, "A"),
            spk_w("tell", 1.1, 1.4, "A"),
            spk_w("you", 1.5, 1.7, "A"),
            spk_w("that", 1.8, 2.2, "A"),
            spk_w("What?", 2.4, 2.9, "B"),
            spk_w("we", 3.1, 3.3, "A"),
            spk_w("should", 3.4, 3.7, "A"),
            spk_w("leave.", 3.8, 4.4, "A"),
        ];

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].text, "I wanted to tell you that");
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[1].text, "What?");
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[2].text, "we should leave.");
        assert_eq!(segs[2].speaker.as_deref(), Some("A"));
        assert_one_speaker_per_segment(&segs);
    }

    // ── REGRESSION TESTS ──

    /// REGRESSION 1: A -> B
    #[test]
    fn regression_a_to_b() {
        let words = vec![
            spk_w("Hello", 0.0, 0.5, "A"),
            spk_w("there.", 0.6, 1.0, "A"),
            spk_w("Hi!", 1.2, 1.6, "B"),
        ];
        let segs = DubbingSegmenter::new().segment(&words);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_one_speaker_per_segment(&segs);
    }

    /// REGRESSION 2: A -> A -> B
    #[test]
    fn regression_a_a_to_b() {
        let words = vec![
            spk_w("First", 0.0, 0.4, "A"),
            spk_w("sentence.", 0.5, 1.0, "A"),
            spk_w("Second", 1.2, 1.6, "A"),
            spk_w("sentence.", 1.7, 2.2, "A"),
            spk_w("Response.", 2.4, 3.0, "B"),
        ];
        let segs = DubbingSegmenter::new().segment(&words);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[0].text, "First sentence. Second sentence.");
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[1].text, "Response.");
        assert_one_speaker_per_segment(&segs);
    }

    /// REGRESSION 3: A -> B -> B -> A
    #[test]
    fn regression_a_b_b_a() {
        let words = vec![
            spk_w("Hello.", 0.0, 0.5, "A"),
            spk_w("Hi.", 0.7, 1.0, "B"),
            spk_w("How are you?", 1.2, 2.0, "B"),
            spk_w("Great, thanks.", 2.2, 3.2, "A"),
        ];
        let segs = DubbingSegmenter::new().segment(&words);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].speaker.as_deref(), Some("A"));
        assert_eq!(segs[1].speaker.as_deref(), Some("B"));
        assert_eq!(segs[1].text, "Hi. How are you?");
        assert_eq!(segs[2].speaker.as_deref(), Some("A"));
        assert_one_speaker_per_segment(&segs);
    }

    /// REGRESSION 4: Multilingual preservation inside speaker turn
    #[test]
    fn regression_multilingual_inside_turns() {
        let words = vec![
            spk_w("Здравствуйте,", 0.0, 0.8, "RU_1"),
            spk_w("как", 0.9, 1.2, "RU_1"),
            spk_w("дела?", 1.3, 1.8, "RU_1"),
            spk_w("非常好！", 2.0, 2.8, "ZH_1"),
            spk_w("元気です。", 3.0, 3.8, "JA_1"),
        ];
        let segs = DubbingSegmenter::new().segment(&words);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].speaker.as_deref(), Some("RU_1"));
        assert_eq!(segs[1].speaker.as_deref(), Some("ZH_1"));
        assert_eq!(segs[2].speaker.as_deref(), Some("JA_1"));
        assert_one_speaker_per_segment(&segs);
    }
}
