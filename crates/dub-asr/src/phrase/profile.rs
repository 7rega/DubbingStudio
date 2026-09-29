use crate::phrase::{
    config::SegmentationConfig,
    time::Seconds,
    types::TextUnit,
};
use std::collections::HashSet;
use std::sync::Arc;

/// Нормализация тега языка по BCP-47.
/// "ru-RU", "ru_RU", "RU" -> "ru"
/// "auto", пустые строки, пробелы -> None
pub fn normalize_language_tag(lang: Option<&str>) -> Option<String> {
    let raw = lang?.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("auto") {
        return None;
    }
    let primary = raw
        .split(['-', '_'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    Some(primary)
}

/// Единственный канонический приоритет определения языка источника:
/// 1. Пользовательский оверрайд (из config.language)
/// 2. Детектированный ASR язык (asr.language)
/// 3. Fallback "und" (DefaultProfile)
pub fn resolve_source_language(
    user_override: Option<&str>,
    asr_detected: Option<&str>,
) -> String {
    normalize_language_tag(user_override)
        .or_else(|| normalize_language_tag(asr_detected))
        .unwrap_or_else(|| "und".to_string())
}

/// Проверка, является ли символ знаком пунктуации CJK.
pub fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '。' | '！'
            | '？'
            | '、'
            | '，'
            | '；'
            | '：'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '【'
            | '】'
            | '《'
            | '》'
            | '…'
            | '—'
            | '～'
    )
}

/// Очистка текста от пунктуации и пробелов с приведением к нижнему регистру.
pub fn clean_word(text: &str) -> String {
    text.trim_matches(|c: char| {
        c.is_ascii_punctuation() || c.is_whitespace() || is_cjk_punct(c)
    })
    .to_lowercase()
}

/// Проверка завершения строки терминальной пунктуацией (с учётом кавычек и скобок).
pub fn ends_with_punct(text: &str, punct_list: &[&str]) -> bool {
    let trimmed = text.trim_end_matches(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '\''
            || c == '»'
            || c == '”'
            || c == '’'
            || c == '」'
            || c == '』'
            || c == ')'
            || c == '）'
            || c == ']'
            || c == '}'
    });
    punct_list.iter().any(|&p| trimmed.ends_with(p))
}

pub trait LanguageProfile: Send + Sync {
    fn name(&self) -> &str;
    fn strong_punctuation(&self) -> &[&str];
    fn weak_punctuation(&self) -> &[&str];
    fn ends_sentence(&self, text: &str) -> bool;
    fn ends_clause(&self, text: &str) -> bool;

    /// Проверка, начинается ли текст со строчной буквы или сочинительного/подчинительного
    /// союза (продолжение прерванной фразы).
    fn starts_with_continuation(&self, text: &str) -> bool;

    /// Модификатор синтаксической связи границы между current и next.
    fn syntax_boundary_modifier(
        &self,
        current: &dyn TextUnit,
        next: &dyn TextUnit,
        gap: Seconds,
    ) -> f64;

    /// Обобщённое правило допустимости границы в DP.
    fn is_valid_boundary(
        &self,
        current: &dyn TextUnit,
        _next: &dyn TextUnit,
        gap: Seconds,
        cfg: &SegmentationConfig,
    ) -> bool {
        if !current.can_split_after() {
            return false;
        }
        if self.uses_whitespace_join() {
            true
        } else {
            self.ends_sentence(current.text())
                || self.ends_clause(current.text())
                || gap >= cfg.pause_min_sec()
        }
    }

    fn uses_whitespace_join(&self) -> bool;
    fn join_units(&self, units: &[&str]) -> String;
}

// ---------------------------------------------------------------------------
// Константы пунктуации
// ---------------------------------------------------------------------------

const LATIN_STRONG_PUNCT: &[&str] = &[".", "!", "?", "...", "…"];
const LATIN_WEAK_PUNCT: &[&str] = &[",", ";", ":", "—", "-", "–"];

const CJK_STRONG_PUNCT: &[&str] = &["。", "！", "？", "...", "…", ".", "!", "?"];
const CJK_WEAK_PUNCT: &[&str] = &["、", "，", "；", "：", "—", "–", ",", ";", ":", "-"];

// ---------------------------------------------------------------------------
// Русский профиль (RussianProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct RussianProfile;

impl RussianProfile {
    fn is_forward_binding(word: &str) -> bool {
        matches!(
            word,
            // Вводные конструкции и связки
            "думаю"
                | "думаем"
                | "кажется"
                | "считаю"
                | "считаем"
                | "полагаю"
                | "полагаем"
                | "надеюсь"
                | "знаю"
                | "понимаю"
                | "помню"
                | "скажу"
                | "скажи"
                | "видишь"
                | "слышишь"
                | "чувствую"
                // Союзы требующие продолжения
                | "потому"
                | "так"
                | "как"
                | "не"
                | "ни"
                | "то"
                | "ли"
                | "и"
                | "а"
                | "но"
                | "или"
                | "либо"
                // Предлоги
                | "в"
                | "во"
                | "на"
                | "из"
                | "из-за"
                | "из-под"
                | "к"
                | "ко"
                | "по"
                | "с"
                | "со"
                | "у"
                | "о"
                | "об"
                | "обо"
                | "от"
                | "ото"
                | "до"
                | "для"
                | "без"
                | "при"
                | "про"
                | "через"
                | "сквозь"
                | "под"
                | "подо"
                | "над"
                | "надо"
                | "перед"
                | "передо"
                | "между"
                | "ради"
        )
    }

    fn is_backward_binding(word: &str) -> bool {
        matches!(
            word,
            "что"
                | "чтобы"
                | "который"
                | "которая"
                | "которое"
                | "которые"
                | "которого"
                | "которой"
                | "которых"
                | "где"
                | "куда"
                | "откуда"
                | "когда"
                | "почему"
                | "зачем"
                | "как"
                | "если"
                | "хотя"
        )
    }
}

impl LanguageProfile for RussianProfile {
    fn name(&self) -> &str {
        "ru"
    }

    fn strong_punctuation(&self) -> &[&str] {
        LATIN_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        LATIN_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        if trimmed.chars().next().map_or(false, |c| c.is_lowercase()) {
            return true;
        }
        let first_word = clean_word(trimmed.split_whitespace().next().unwrap_or(trimmed));
        matches!(
            first_word.as_str(),
            "и" | "а"
                | "но"
                | "что"
                | "чтобы"
                | "потому"
                | "так"
                | "если"
                | "хотя"
                | "или"
                | "когда"
        )
    }

    fn syntax_boundary_modifier(
        &self,
        current: &dyn TextUnit,
        next: &dyn TextUnit,
        gap: Seconds,
    ) -> f64 {
        let cur_clean = clean_word(current.text());
        let next_clean = clean_word(next.text());

        let forward_penalty = if Self::is_forward_binding(&cur_clean) {
            -1.5
        } else {
            0.0
        };

        let backward_penalty =
            if Self::is_backward_binding(&next_clean) && gap.as_f64() < 0.30 {
                -1.5
            } else {
                0.0
            };

        (forward_penalty + backward_penalty).max(-1.5)
    }

    fn uses_whitespace_join(&self) -> bool {
        true
    }

    fn join_units(&self, units: &[&str]) -> String {
        units
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ---------------------------------------------------------------------------
// Английский профиль (EnglishProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct EnglishProfile;

impl EnglishProfile {
    fn is_forward_binding(word: &str) -> bool {
        matches!(
            word,
            // Introductory verbs
            "think"
                | "thinks"
                | "thought"
                | "believe"
                | "believes"
                | "suppose"
                | "supposes"
                | "guess"
                | "guesses"
                | "know"
                | "knows"
                | "mean"
                | "means"
                | "feel"
                | "feels"
                // Prepositions
                | "in"
                | "on"
                | "at"
                | "to"
                | "into"
                | "onto"
                | "from"
                | "of"
                | "for"
                | "with"
                | "without"
                | "by"
                | "about"
                | "through"
                | "between"
                | "under"
                | "over"
                | "before"
                | "after"
                // Conjunctions / particles requiring continuation
                | "and"
                | "but"
                | "or"
                | "so"
                | "because"
                | "if"
                | "that"
                | "which"
                | "when"
                | "while"
                | "though"
                | "although"
                | "not"
        )
    }

    fn is_backward_binding(word: &str) -> bool {
        matches!(
            word,
            "that"
                | "which"
                | "who"
                | "whom"
                | "whose"
                | "where"
                | "when"
                | "why"
                | "how"
                | "because"
                | "if"
                | "since"
                | "although"
                | "though"
        )
    }
}

impl LanguageProfile for EnglishProfile {
    fn name(&self) -> &str {
        "en"
    }

    fn strong_punctuation(&self) -> &[&str] {
        LATIN_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        LATIN_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        if trimmed.chars().next().map_or(false, |c| c.is_lowercase()) {
            return true;
        }
        let first_word = clean_word(trimmed.split_whitespace().next().unwrap_or(trimmed));
        matches!(
            first_word.as_str(),
            "and"
                | "but"
                | "or"
                | "so"
                | "because"
                | "that"
                | "which"
                | "although"
                | "though"
                | "if"
                | "when"
                | "while"
        )
    }

    fn syntax_boundary_modifier(
        &self,
        current: &dyn TextUnit,
        next: &dyn TextUnit,
        gap: Seconds,
    ) -> f64 {
        let cur_clean = clean_word(current.text());
        let next_clean = clean_word(next.text());

        let forward_penalty = if Self::is_forward_binding(&cur_clean) {
            -1.5
        } else {
            0.0
        };

        let backward_penalty =
            if Self::is_backward_binding(&next_clean) && gap.as_f64() < 0.30 {
                -1.5
            } else {
                0.0
            };

        (forward_penalty + backward_penalty).max(-1.5)
    }

    fn uses_whitespace_join(&self) -> bool {
        true
    }

    fn join_units(&self, units: &[&str]) -> String {
        units
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ---------------------------------------------------------------------------
// Японский профиль (JapaneseProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct JapaneseProfile;

impl JapaneseProfile {
    fn is_continuation_marker(clean_first: &str) -> bool {
        matches!(
            clean_first,
            "そして"
                | "でも"
                | "だから"
                | "しかし"
                | "また"
                | "ので"
                | "のに"
                | "から"
                | "と"
                | "て"
                | "なら"
                | "けれど"
                | "けれども"
        )
    }
}

impl LanguageProfile for JapaneseProfile {
    fn name(&self) -> &str {
        "ja"
    }

    fn strong_punctuation(&self) -> &[&str] {
        CJK_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        CJK_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        let first_unit = trimmed.split_whitespace().next().unwrap_or(trimmed);
        let clean_first = clean_word(first_unit);
        Self::is_continuation_marker(&clean_first)
    }

    fn syntax_boundary_modifier(
        &self,
        _current: &dyn TextUnit,
        _next: &dyn TextUnit,
        _gap: Seconds,
    ) -> f64 {
        0.0
    }

    fn uses_whitespace_join(&self) -> bool {
        false
    }

    fn join_units(&self, units: &[&str]) -> String {
        units.concat()
    }
}

// ---------------------------------------------------------------------------
// Корейский профиль (KoreanProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct KoreanProfile;

impl KoreanProfile {
    fn is_continuation_marker(clean_first: &str) -> bool {
        matches!(
            clean_first,
            "그리고" | "하지만" | "그래서" | "그런데" | "그러나" | "그러면" | "그래도"
        )
    }
}

impl LanguageProfile for KoreanProfile {
    fn name(&self) -> &str {
        "ko"
    }

    fn strong_punctuation(&self) -> &[&str] {
        CJK_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        CJK_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        let first_unit = trimmed.split_whitespace().next().unwrap_or(trimmed);
        let clean_first = clean_word(first_unit);
        Self::is_continuation_marker(&clean_first)
    }

    fn syntax_boundary_modifier(
        &self,
        _current: &dyn TextUnit,
        _next: &dyn TextUnit,
        _gap: Seconds,
    ) -> f64 {
        0.0
    }

    fn uses_whitespace_join(&self) -> bool {
        true
    }

    fn join_units(&self, units: &[&str]) -> String {
        units
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ---------------------------------------------------------------------------
// Китайский профиль (ChineseProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ChineseProfile;

impl ChineseProfile {
    fn is_continuation_marker(clean_first: &str) -> bool {
        matches!(
            clean_first,
            "而且"
                | "但是"
                | "所以"
                | "然后"
                | "并且"
                | "不过"
                | "然而"
                | "因为"
                | "如果"
                | "虽然"
        )
    }
}

impl LanguageProfile for ChineseProfile {
    fn name(&self) -> &str {
        "zh"
    }

    fn strong_punctuation(&self) -> &[&str] {
        CJK_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        CJK_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        let first_unit = trimmed.split_whitespace().next().unwrap_or(trimmed);
        let clean_first = clean_word(first_unit);
        Self::is_continuation_marker(&clean_first)
    }

    fn syntax_boundary_modifier(
        &self,
        _current: &dyn TextUnit,
        _next: &dyn TextUnit,
        _gap: Seconds,
    ) -> f64 {
        0.0
    }

    fn uses_whitespace_join(&self) -> bool {
        false
    }

    fn join_units(&self, units: &[&str]) -> String {
        units.concat()
    }
}

// ---------------------------------------------------------------------------
// Профиль по умолчанию (DefaultProfile)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct DefaultProfile;

impl LanguageProfile for DefaultProfile {
    fn name(&self) -> &str {
        "und"
    }

    fn strong_punctuation(&self) -> &[&str] {
        LATIN_STRONG_PUNCT
    }

    fn weak_punctuation(&self) -> &[&str] {
        LATIN_WEAK_PUNCT
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, text: &str) -> bool {
        let trimmed = text.trim_start();
        if trimmed.chars().next().map_or(false, |c| c.is_lowercase()) {
            return true;
        }
        let first_word = clean_word(trimmed.split_whitespace().next().unwrap_or(trimmed));
        matches!(
            first_word.as_str(),
            "and"
                | "but"
                | "or"
                | "so"
                | "because"
                | "that"
                | "which"
                | "if"
                | "when"
                | "while"
        )
    }

    fn syntax_boundary_modifier(
        &self,
        _current: &dyn TextUnit,
        _next: &dyn TextUnit,
        _gap: Seconds,
    ) -> f64 {
        0.0
    }

    fn uses_whitespace_join(&self) -> bool {
        true
    }

    fn join_units(&self, units: &[&str]) -> String {
        units
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ---------------------------------------------------------------------------
// Реестр профилей (ProfileRegistry)
// ---------------------------------------------------------------------------

pub struct ProfileRegistry;

impl ProfileRegistry {
    pub fn get(lang: Option<&str>) -> Arc<dyn LanguageProfile> {
        let normalized = normalize_language_tag(lang);
        match normalized.as_deref() {
            Some("ru") => Arc::new(RussianProfile),
            Some("en") => Arc::new(EnglishProfile),
            Some("ja") => Arc::new(JapaneseProfile),
            Some("ko") => Arc::new(KoreanProfile),
            Some("zh") => Arc::new(ChineseProfile),
            _ => Arc::new(DefaultProfile),
        }
    }
}

// ---------------------------------------------------------------------------
// Модульные тесты
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::types::AsrUnit;

    #[test]
    fn test_normalize_language_tag() {
        assert_eq!(normalize_language_tag(Some("ru-RU")), Some("ru".to_string()));
        assert_eq!(normalize_language_tag(Some("ru_RU")), Some("ru".to_string()));
        assert_eq!(normalize_language_tag(Some("RU")), Some("ru".to_string()));
        assert_eq!(normalize_language_tag(Some("en-US")), Some("en".to_string()));
        assert_eq!(normalize_language_tag(Some("JA")), Some("ja".to_string()));
        assert_eq!(normalize_language_tag(Some("zh-CN")), Some("zh".to_string()));
        assert_eq!(normalize_language_tag(Some("ko-KR")), Some("ko".to_string()));
        assert_eq!(normalize_language_tag(Some("auto")), None);
        assert_eq!(normalize_language_tag(Some("  ")), None);
        assert_eq!(normalize_language_tag(None), None);
    }

    #[test]
    fn test_source_language_resolution_priority() {
        // config (Some) > asr (Some) > fallback "und"
        assert_eq!(
            resolve_source_language(Some("ru-RU"), Some("en-US")),
            "ru"
        );
        assert_eq!(resolve_source_language(None, Some("EN")), "en");
        assert_eq!(resolve_source_language(Some("auto"), Some("ja")), "ja");
        assert_eq!(resolve_source_language(None, None), "und");
        assert_eq!(resolve_source_language(Some(""), Some("auto")), "und");
    }

    #[test]
    fn test_profile_registry() {
        assert_eq!(ProfileRegistry::get(Some("ru")).name(), "ru");
        assert_eq!(ProfileRegistry::get(Some("ru-RU")).name(), "ru");
        assert_eq!(ProfileRegistry::get(Some("en")).name(), "en");
        assert_eq!(ProfileRegistry::get(Some("ja")).name(), "ja");
        assert_eq!(ProfileRegistry::get(Some("ko")).name(), "ko");
        assert_eq!(ProfileRegistry::get(Some("zh")).name(), "zh");
        assert_eq!(ProfileRegistry::get(Some("fr")).name(), "und");
        assert_eq!(ProfileRegistry::get(None).name(), "und");
    }

    #[test]
    fn test_cjk_no_whitespace_joining() {
        let ja = JapaneseProfile;
        assert_eq!(
            ja.join_units(&["これ", "は", "テスト", "です"]),
            "これはテストです"
        );
        assert!(!ja.uses_whitespace_join());

        let zh = ChineseProfile;
        assert_eq!(
            zh.join_units(&["这", "是", "测试"]),
            "这是测试"
        );
        assert!(!zh.uses_whitespace_join());

        let ru = RussianProfile;
        assert_eq!(ru.join_units(&["Это", "тест"]), "Это тест");
        assert!(ru.uses_whitespace_join());

        let en = EnglishProfile;
        assert_eq!(en.join_units(&["This", "is", "a", "test"]), "This is a test");
        assert!(en.uses_whitespace_join());

        let ko = KoreanProfile;
        assert_eq!(ko.join_units(&["이것은", "테스트입니다"]), "이것은 테스트입니다");
        assert!(ko.uses_whitespace_join());
    }

    #[test]
    fn test_ends_sentence_and_clause() {
        let ru = RussianProfile;
        assert!(ru.ends_sentence("Привет мир."));
        assert!(ru.ends_sentence("Привет мир!"));
        assert!(ru.ends_sentence("Привет мир?"));
        assert!(ru.ends_sentence("Привет мир..."));
        assert!(ru.ends_sentence("«Привет мир.»"));
        assert!(!ru.ends_sentence("Привет мир,"));
        assert!(!ru.ends_sentence("Привет мир"));

        assert!(ru.ends_clause("Привет,"));
        assert!(ru.ends_clause("Привет —"));
        assert!(ru.ends_clause("Привет:"));
        assert!(!ru.ends_clause("Привет."));

        let ja = JapaneseProfile;
        assert!(ja.ends_sentence("テストです。"));
        assert!(ja.ends_sentence("テストです！"));
        assert!(ja.ends_clause("テストですが、"));
        assert!(!ja.ends_sentence("テストですが、"));
    }

    #[test]
    fn test_starts_with_continuation() {
        let ru = RussianProfile;
        assert!(ru.starts_with_continuation("привет"));
        assert!(ru.starts_with_continuation("И поехали"));
        assert!(ru.starts_with_continuation("Потому что"));
        assert!(!ru.starts_with_continuation("Привет"));

        let en = EnglishProfile;
        assert!(en.starts_with_continuation("hello"));
        assert!(en.starts_with_continuation("And we went"));
        assert!(en.starts_with_continuation("Because it is"));
        assert!(!en.starts_with_continuation("Hello"));

        let ja = JapaneseProfile;
        assert!(ja.starts_with_continuation("そして次の日"));
        assert!(ja.starts_with_continuation("でも大丈夫"));
        assert!(!ja.starts_with_continuation("明日は晴れ"));

        let zh = ChineseProfile;
        assert!(zh.starts_with_continuation("而且我们去了"));
        assert!(zh.starts_with_continuation("但是很好"));
        assert!(!zh.starts_with_continuation("明天会晴天"));

        let ko = KoreanProfile;
        assert!(ko.starts_with_continuation("그리고 우리는 갔다"));
        assert!(ko.starts_with_continuation("하지만 괜찮아"));
        assert!(!ko.starts_with_continuation("내일은 맑을 것이다"));
    }

    #[test]
    fn test_syntax_boundary_modifier() {
        let ru = RussianProfile;
        let u_duma = AsrUnit {
            text: "думаю,".to_string(),
            start: Seconds(0.0),
            end: Seconds(3.9),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let u_chto = AsrUnit {
            text: "что...".to_string(),
            start: Seconds(4.5),
            end: Seconds(8.4),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };

        // Forward binding on "думаю" (gap 0.6s >= 0.3s so no backward binding, but forward is -1.5)
        let modifier = ru.syntax_boundary_modifier(&u_duma, &u_chto, Seconds(0.6));
        assert_eq!(modifier, -1.5);

        // Backward binding on "что" with short gap (< 0.3s) without forward binding
        let u_skazal = AsrUnit {
            text: "сказал,".to_string(),
            start: Seconds(0.0),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let modifier_short = ru.syntax_boundary_modifier(&u_skazal, &u_chto, Seconds(0.2));
        assert_eq!(modifier_short, -1.5);

        // Both forward and backward: clamped to -1.5
        let modifier_both = ru.syntax_boundary_modifier(&u_duma, &u_chto, Seconds(0.2));
        assert_eq!(modifier_both, -1.5);

        // Neutral boundary
        let u_end = AsrUnit {
            text: "сказал.".to_string(),
            start: Seconds(0.0),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let u_next = AsrUnit {
            text: "Мы поехали.".to_string(),
            start: Seconds(1.5),
            end: Seconds(3.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let modifier_neutral = ru.syntax_boundary_modifier(&u_end, &u_next, Seconds(0.5));
        assert_eq!(modifier_neutral, 0.0);
    }

    #[test]
    fn test_cjk_clause_splitting_threshold() {
        let ja = JapaneseProfile;
        let cfg = SegmentationConfig::default();

        let u_no_punct = AsrUnit {
            text: "これ".to_string(),
            start: Seconds(0.0),
            end: Seconds(0.5),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let u_next = AsrUnit {
            text: "は".to_string(),
            start: Seconds(0.6),
            end: Seconds(1.0),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };

        // CJK without punctuation and small gap (< 250ms): invalid boundary
        assert!(!ja.is_valid_boundary(&u_no_punct, &u_next, Seconds(0.1), &cfg));

        // CJK without punctuation but large gap (>= 250ms): valid boundary
        assert!(ja.is_valid_boundary(&u_no_punct, &u_next, Seconds(0.3), &cfg));

        // CJK with punctuation even with small gap: valid boundary
        let u_punct = AsrUnit {
            text: "です。".to_string(),
            start: Seconds(0.0),
            end: Seconds(0.5),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: true,
        };
        assert!(ja.is_valid_boundary(&u_punct, &u_next, Seconds(0.1), &cfg));

        // can_split_after == false is unconditionally invalid
        let u_locked = AsrUnit {
            text: "です。".to_string(),
            start: Seconds(0.0),
            end: Seconds(0.5),
            confidence: None,
            is_whisper_boundary: false,
            can_split_after: false,
        };
        assert!(!ja.is_valid_boundary(&u_locked, &u_next, Seconds(0.5), &cfg));
    }
}
