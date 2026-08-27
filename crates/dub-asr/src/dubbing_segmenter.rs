//! DubbingSegmenter — экспериментальный language-agnostic сегментатор для дубляжа (TTS / Higgs v3).
//!
//! Цель: максимально избегать обрыва грамматически и логически цельных фраз.
//! В отличие от субтитрового сегментатора, ориентирован на хронометраж и естественные
//! интонационные границы нейросетевой озвучки (sweet spot 0–12.5с, target 14.0с, hard cap 15.0с).

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

/// Конфигурация DubbingSegmenter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmenterConfig {
    /// Идеальная верхняя граница длительности фразы (сек). До неё сплит не форсируется.
    pub ideal_duration: f64,
    /// Целевая граница длительности (сек). В диапазоне 12.5–14.0с активно ищется хорошая граница.
    pub target_duration: f64,
    /// Жесткий потолок длительности (сек). После 15.0с срабатывает emergency fallback.
    pub hard_duration: f64,
    /// Порог паузы для безусловного разрыва между сценами/репликами (сек).
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
    pub segment_duration: f64,
    pub boundary_timestamp: f64,
    pub pause_duration: f64,
    pub punctuation: String,
    pub score: f64,
    pub reason: String,
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

/// Оценка качества потенциальной границы между словом `i` и `i+1`.
#[derive(Debug, Clone)]
struct BoundaryCandidate {
    pub word_index: usize,
    pub timestamp: f64,
    pub pause: f64,
    pub duration_from_start: f64,
    pub ends_sentence: bool,
    pub ends_clause: bool,
    pub score: f64,
    pub reason: String,
}

/// Language-agnostic сегментатор для дубляжа.
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

    /// Собрать слова в сегменты (совместимо со стандартным Vec<crate::segment::Segment>).
    pub fn segment(&self, words: &[WordWithTimestamp]) -> Vec<crate::segment::Segment> {
        let (segs, _) = self.segment_with_debug(words);
        segs
    }

    /// Оценить одну потенциальную границу после слова word_idx.
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
            timestamp: curr.end,
            pause,
            duration_from_start: dur,
            ends_sentence: ends_sent,
            ends_clause: ends_cl,
            score,
            reason: reasons.join(", "),
        }
    }

    /// Сегментировать поток слов с подробной диагностикой каждой границы.
    pub fn segment_with_debug(
        &self,
        words: &[WordWithTimestamp],
    ) -> (Vec<crate::segment::Segment>, Vec<BoundaryDebugInfo>) {
        if words.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let mut segments: Vec<crate::segment::Segment> = Vec::new();
        let mut debug_infos: Vec<BoundaryDebugInfo> = Vec::new();

        let n = words.len();
        let mut start_idx = 0usize;

        while start_idx < n {
            let cand_start = words[start_idx].start;
            let cand_speaker = words[start_idx].speaker.as_deref();

            // Сканируем вперед, определяя наилучшую точку границы
            let mut best_boundary: Option<BoundaryCandidate> = None;
            let mut chosen_end_idx = start_idx;
            let mut is_emergency_fallback = false;

            let mut j = start_idx;
            while j < n {
                let curr = &words[j];
                let dur = (curr.end - cand_start).max(0.0);
                let is_last_word = j + 1 == n;

                // 1. Смена спикера: граница обязательна
                let speaker_changed = if !is_last_word {
                    words[j + 1].speaker.as_deref() != cand_speaker
                } else {
                    false
                };

                if speaker_changed {
                    let pause = (words[j + 1].start - curr.end).max(0.0);
                    chosen_end_idx = j;
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        timestamp: curr.end,
                        pause,
                        duration_from_start: dur,
                        ends_sentence: word_ends_sentence(&curr.word),
                        ends_clause: word_ends_clause(&curr.word),
                        score: 100.0,
                        reason: "speaker_turn_boundary".to_string(),
                    });
                    break;
                }

                // 2. Последнее слово потока: закрываем сегмент
                if is_last_word {
                    chosen_end_idx = j;
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        timestamp: curr.end,
                        pause: 0.0,
                        duration_from_start: dur,
                        ends_sentence: word_ends_sentence(&curr.word),
                        ends_clause: word_ends_clause(&curr.word),
                        score: 100.0,
                        reason: "end_of_stream".to_string(),
                    });
                    break;
                }

                let next = &words[j + 1];
                let pause = (next.start - curr.end).max(0.0);

                // 3. Длинная межсценарная тишина (>= scene_silence_gap, напр. 1.8с)
                if pause >= self.config.scene_silence_gap && (word_ends_sentence(&curr.word) || dur >= 3.0) {
                    chosen_end_idx = j;
                    best_boundary = Some(BoundaryCandidate {
                        word_index: j,
                        timestamp: curr.end,
                        pause,
                        duration_from_start: dur,
                        ends_sentence: word_ends_sentence(&curr.word),
                        ends_clause: word_ends_clause(&curr.word),
                        score: 90.0,
                        reason: "scene_silence_gap".to_string(),
                    });
                    break;
                }

                // Оцениваем кандидата j
                let candidate = self.score_boundary(words, j, cand_start);

                // 4. Зона < ideal_duration (0–12.5с):
                // Накапливаем слова, не режем фразу из-за времени.
                if dur < self.config.ideal_duration {
                    // Обновляем лучшего кандидата на случай, если дальше придется выбирать
                    if best_boundary.as_ref().map_or(true, |b| candidate.score > b.score) {
                        best_boundary = Some(candidate);
                    }
                    j += 1;
                    continue;
                }

                // 5. Зона 12.5–14.0с:
                // Если текущая граница хорошая (конец предложения или высокая оценка) — фиксируем сплит!
                if dur <= self.config.target_duration {
                    if candidate.ends_sentence || candidate.score >= self.config.good_score_threshold {
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

                // 6. Зона 14.0–15.0с (Lookahead):
                // Активно ищем лучшую границу
                if dur <= self.config.hard_duration {
                    if candidate.ends_sentence || candidate.ends_clause || candidate.pause >= 0.35 {
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

                // 7. Зона > 15.0с: Превышен Hard Limit (Emergency Fallback)
                // Берем лучшую найденную границу до 15с
                is_emergency_fallback = true;
                if let Some(b) = best_boundary.take() {
                    if b.word_index >= start_idx {
                        chosen_end_idx = b.word_index;
                        best_boundary = Some(BoundaryCandidate {
                            reason: format!("emergency_fallback_hard_limit (picked best: {})", b.reason),
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

            // Формируем сегмент из [start_idx ..= chosen_end_idx]
            let seg_words: Vec<crate::segment::Word> = words[start_idx..=chosen_end_idx]
                .iter()
                .cloned()
                .map(Into::into)
                .collect();

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
                timestamp: seg_end,
                pause: 0.0,
                duration_from_start: seg_dur,
                ends_sentence: false,
                ends_clause: false,
                score: 0.0,
                reason: "default_fallback".to_string(),
            });

            debug_infos.push(BoundaryDebugInfo {
                segment_index: segments.len(),
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
                is_fallback: is_emergency_fallback,
            });

            segments.push(crate::segment::Segment {
                start: seg_start,
                end: seg_end,
                text,
                words: seg_words,
            });

            start_idx = chosen_end_idx + 1;
        }

        (segments, debug_infos)
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

    /// CASE A: Короткая законченная фраза < 12.5 sec → один segment.
    #[test]
    fn case_a_short_finished_sentence() {
        let words = vec![
            w("Hello", 0.0, 0.5),
            w("world,", 0.6, 1.2),
            w("this", 1.3, 1.8),
            w("is", 1.9, 2.2),
            w("great.", 2.3, 3.0),
        ];
        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Hello world, this is great.");
        assert_eq!(segs[0].start, 0.0);
        assert_eq!(segs[0].end, 3.0);
    }

    /// CASE B: Фраза 10 sec без punctuation → не резать просто из-за времени.
    #[test]
    fn case_b_long_phrase_without_punct_under_ideal_dur() {
        // Создаем фразу на 10.0 секунд без знаков препинания
        let mut words = Vec::new();
        for i in 0..20 {
            let st = i as f64 * 0.5;
            let en = st + 0.45;
            words.push(w("word", st, en));
        }
        assert_eq!(words.last().unwrap().end, 9.95);

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        // Не должна быть разрезана на 8.0с, как старый ASR
        assert_eq!(segs.len(), 1, "Фраза 10с без пунктуации должна оставаться единым сегментом");
        assert!((segs[0].end - segs[0].start) > 9.0);
    }

    /// CASE C: Фраза 13 sec с хорошим sentence boundary → split/end at sentence boundary.
    #[test]
    fn case_c_sentence_boundary_in_target_window() {
        let mut words = Vec::new();
        // Предложение 1: 0.0 .. 13.0с
        for i in 0..20 {
            let st = i as f64 * 0.6;
            let en = st + 0.5;
            let word = if i == 19 { "finished." } else { "talking" };
            words.push(w(word, st, en));
        }
        // Предложение 2: 13.5 .. 18.0с
        for i in 0..10 {
            let st = 13.5 + i as f64 * 0.45;
            let en = st + 0.4;
            let word = if i == 9 { "done." } else { "second" };
            words.push(w(word, st, en));
        }

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].text.ends_with("finished."));
        assert!(segs[1].text.starts_with("second"));
        assert!(!debug[0].is_fallback);
    }

    /// CASE D: Фраза >14 sec, но естественная boundary есть на 13.7 sec → использовать её.
    #[test]
    fn case_d_natural_boundary_at_13_7s() {
        let mut words = Vec::new();
        // Слова до 13.7s, где стоит точка с запятой или пауза
        for i in 0..20 {
            let st = i as f64 * 0.65;
            let en = st + 0.55;
            let word = if i == 19 { "here;" } else { "phrase" };
            words.push(w(word, st, en));
        }
        // В районе 13.7s есть пауза 0.5s и точка с запятой
        let t_split = words.last().unwrap().end; // ~13.55s
        for i in 0..10 {
            let st = t_split + 0.5 + i as f64 * 0.5;
            let en = st + 0.45;
            words.push(w("continuation", st, en));
        }

        let segmenter = DubbingSegmenter::new();
        let (segs, _) = segmenter.segment_with_debug(&words);
        assert!(segs.len() >= 2);
        assert!(segs[0].text.ends_with("here;"));
    }

    /// CASE E: Фраза 17 sec, естественная boundary только около 15 sec → fallback boundary около hard limit.
    #[test]
    fn case_e_emergency_fallback_at_hard_limit() {
        let mut words = Vec::new();
        // Сплошной поток без пунктуации на 17 секунд, но с небольшой паузой на 14.2с
        for i in 0..30 {
            let st = if i == 20 {
                // пауза на 20-м слове (~14.2s)
                i as f64 * 0.55 + 0.4
            } else {
                i as f64 * 0.55
            };
            let en = st + 0.5;
            words.push(w("continuous", st, en));
        }

        let segmenter = DubbingSegmenter::new();
        let (segs, debug) = segmenter.segment_with_debug(&words);
        assert!(segs.len() >= 2);
        // Первый сегмент не должен превышать hard_duration (15.0с)
        assert!((segs[0].end - segs[0].start) <= 15.0);
        assert!(debug[0].is_fallback || debug[0].reason.contains("emergency") || (segs[0].end - segs[0].start) <= 15.0);
    }

    /// CASE F: Длинная фраза с паузой внутри предложения → не считать pause автоматически окончанием.
    #[test]
    fn case_f_pause_inside_sentence_not_split_early() {
        let mut words = Vec::new();
        // Слово 1..5 (до 3 сек)
        for i in 0..5 {
            words.push(w("we", i as f64 * 0.6, i as f64 * 0.6 + 0.5));
        }
        // Пауза 0.7с (вдох человека) на 3-й секунде
        words.push(w("took", 3.7, 4.2));
        words.push(w("a", 4.3, 4.6));
        words.push(w("breath", 4.7, 5.2));
        words.push(w("and", 5.3, 5.7));
        words.push(w("kept", 5.8, 6.3));
        words.push(w("going.", 6.4, 7.0));

        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        // Старый ASR разрезал бы на паузе 0.7с (SEG_MAX_GAP=0.6). Новый держит фразу вместе!
        assert_eq!(segs.len(), 1, "Пауза вдоха 0.7с внутри предложения не должна резать фразу < 12.5с");
        assert_eq!(segs[0].text, "we we we we we took a breath and kept going.");
    }

    /// CASE G: Несколько коротких фраз подряд → новый алгоритм не создаёт чрезмерное количество микросегментов.
    #[test]
    fn case_g_multiple_short_phrases_grouped() {
        let words = vec![
            w("Yes.", 0.0, 0.4),
            w("I", 0.6, 0.9),
            w("know.", 1.0, 1.4),
            w("Let's", 1.7, 2.1),
            w("go.", 2.2, 2.6),
        ];
        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        // Короткие реплики без большой паузы собираются в комфортный для TTS блок
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "Yes. I know. Let's go.");
    }

    /// CASE H: Different languages: English, Russian, Chinese, Japanese.
    #[test]
    fn case_h_multilingual_support() {
        let segmenter = DubbingSegmenter::new();

        // 1. Russian
        let ru_words = vec![
            w("Мы", 0.0, 0.4),
            w("вчера", 0.5, 0.9),
            w("пошли", 1.0, 1.4),
            w("в", 1.5, 1.7),
            w("парк,", 1.8, 2.2),
            w("чтобы", 2.3, 2.7),
            w("отдохнуть.", 2.8, 3.5),
        ];
        let ru_segs = segmenter.segment(&ru_words);
        assert_eq!(ru_segs.len(), 1);
        assert_eq!(ru_segs[0].text, "Мы вчера пошли в парк, чтобы отдохнуть.");

        // 2. Chinese (CJK punctuation 。 and ，)
        let zh_words = vec![
            w("今天", 0.0, 0.6),
            w("天气", 0.7, 1.2),
            w("很好，", 1.3, 1.8),
            w("我们", 1.9, 2.4),
            w("去公园。", 2.5, 3.2),
        ];
        let zh_segs = segmenter.segment(&zh_words);
        assert_eq!(zh_segs.len(), 1);
        assert_eq!(zh_segs[0].text, "今天 天气 很好， 我们 去公园。");

        // 3. Japanese (CJK punctuation 、 and 。)
        let ja_words = vec![
            w("今日", 0.0, 0.5),
            w("は、", 0.6, 1.0),
            w("とても", 1.1, 1.5),
            w("良い", 1.6, 2.0),
            w("天気です。", 2.1, 2.8),
        ];
        let ja_segs = segmenter.segment(&ja_words);
        assert_eq!(ja_segs.len(), 1);
        assert_eq!(ja_segs[0].text, "今日 は、 とても 良い 天气です。");
    }

    /// Тест смены спикера: разные спикеры обязаны разделяться на границы
    #[test]
    fn speaker_turn_splits_immediately() {
        let words = vec![
            spk_w("Hello", 0.0, 0.5, "spk0"),
            spk_w("there.", 0.6, 1.0, "spk0"),
            spk_w("Hi", 1.2, 1.6, "spk1"),
            spk_w("friend!", 1.7, 2.2, "spk1"),
        ];
        let segmenter = DubbingSegmenter::new();
        let segs = segmenter.segment(&words);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Hello there.");
        assert_eq!(segs[1].text, "Hi friend!");
    }
}
