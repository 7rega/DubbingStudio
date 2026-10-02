//! Что говорят субтитры реплики в режиме субтитров проекта: основная строка и вторая строка двуязычных
//! субтитров. Одна точка для вжигания (build_ass), дорожек субтитров mkv и экспорта SRT/VTT.
//!
//! Режимы: translate — перевод; transcribe — язык оригинала; bilingual — перевод основной строкой и
//! оригинал второй; none — субтитров нет. В дубляже и закадре tgt_text — озвученный перевод, поэтому
//! субтитры на языке оригинала берутся из src_text: язык субтитров выбирается отдельно от языка дубляжа.
//! Без дубляжа (nodub, transcribe) в режиме transcribe tgt_text — сам транскрипт, который правят в
//! редакторе.

use dub_core::Segment;

/// Строки субтитра одной реплики.
#[derive(Debug, PartialEq)]
pub struct SubLines {
    pub primary: String,
    pub secondary: Option<String>,
    /// Основная строка — перевод, который звучит в дубляже: ей положена подсветка по словам дубля.
    pub primary_is_translation: bool,
}

/// Строки субтитра реплики `s`; `translation` — её перевод с учётом правки строки в редакторе.
/// Пустая основная строка — субтитра у реплики нет.
pub fn lines(mode: &str, is_dub: bool, s: &Segment, translation: &str) -> SubLines {
    let translation = translation.trim();
    let original = s.src_text.trim();
    match mode {
        "transcribe" if is_dub => SubLines { primary: original.to_string(), secondary: None, primary_is_translation: false },
        "transcribe" => SubLines { primary: translation.to_string(), secondary: None, primary_is_translation: false },
        "bilingual" if translation.is_empty() => {
            SubLines { primary: original.to_string(), secondary: None, primary_is_translation: false }
        }
        "bilingual" => SubLines {
            primary: translation.to_string(),
            secondary: (!original.is_empty() && original != translation).then(|| original.to_string()),
            primary_is_translation: true,
        },
        "none" => SubLines { primary: String::new(), secondary: None, primary_is_translation: false },
        _ => SubLines { primary: translation.to_string(), secondary: None, primary_is_translation: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(src: &str, tgt: &str) -> Segment {
        Segment { id: "s0".into(), src_text: src.into(), tgt_text: tgt.into(), ..Default::default() }
    }

    #[test]
    fn a_dub_shows_the_original_when_the_subtitles_are_in_the_original_language() {
        let s = seg("Where were you?", "Где ты был?");
        assert_eq!(lines("transcribe", true, &s, &s.tgt_text).primary, "Where were you?");
        assert_eq!(lines("translate", true, &s, &s.tgt_text).primary, "Где ты был?");
        assert!(!lines("transcribe", true, &s, &s.tgt_text).primary_is_translation);
    }

    #[test]
    fn without_a_dub_the_transcript_is_the_edited_line() {
        let s = seg("Where were you?", "Where were you, man?");
        assert_eq!(lines("transcribe", false, &s, &s.tgt_text).primary, "Where were you, man?");
    }

    #[test]
    fn bilingual_is_the_translation_over_the_original() {
        let s = seg(" Where were you? ", "Где ты был?");
        let l = lines("bilingual", true, &s, "Где же ты был?");
        assert_eq!(l.primary, "Где же ты был?");
        assert_eq!(l.secondary.as_deref(), Some("Where were you?"));
        assert!(l.primary_is_translation);
    }

    #[test]
    fn bilingual_without_a_translation_or_with_the_same_text_is_one_line() {
        let untranslated = seg("Hello", "");
        assert_eq!(lines("bilingual", false, &untranslated, ""), SubLines { primary: "Hello".into(), secondary: None, primary_is_translation: false });
        let same = seg("OK", "OK");
        assert_eq!(lines("bilingual", false, &same, "OK").secondary, None);
    }

    #[test]
    fn no_subtitles_is_an_empty_line() {
        let s = seg("a", "b");
        assert!(lines("none", true, &s, "b").primary.is_empty());
    }
}
