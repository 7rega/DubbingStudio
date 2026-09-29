use crate::phrase::{
    profile::{ends_with_punct, LanguageProfile},
    time::Seconds,
    types::{AlignedUnit, AmbiguityStatus, SpeakerTurn, TextUnit},
};

/// Тестовый профиль языка с отключёнными синтаксическими штрафами.
///
/// Используется в Сценарии 3 Таблицы 5.1 для проверки дискриминирующего влияния
/// синтаксического модификатора на решение DP-сегментатора.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSyntaxProfile;

impl LanguageProfile for NoSyntaxProfile {
    fn name(&self) -> &str {
        "no_syntax"
    }

    fn strong_punctuation(&self) -> &[&str] {
        &[".", "!", "?", "...", "…"]
    }

    fn weak_punctuation(&self) -> &[&str] {
        &[",", ";", ":", "—", "-", "–"]
    }

    fn ends_sentence(&self, text: &str) -> bool {
        ends_with_punct(text, self.strong_punctuation())
    }

    fn ends_clause(&self, text: &str) -> bool {
        ends_with_punct(text, self.weak_punctuation())
    }

    fn starts_with_continuation(&self, _text: &str) -> bool {
        false
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
        units.join(" ")
    }
}

/// Создание синтетического юнита `AlignedUnit` для модульных тестов.
pub fn make_aligned_unit(text: &str, start: f64, end: f64, can_split_after: bool) -> AlignedUnit {
    AlignedUnit {
        text: text.to_string(),
        start: Seconds(start),
        end: Seconds(end),
        speaker: "SPEAKER_00".to_string(),
        word_confidence: Some(0.95),
        speaker_confidence: 0.95,
        overlap_ratio: 0.0,
        ambiguity: AmbiguityStatus::Clear,
        alternative_speaker: None,
        alternative_overlap_ratio: 0.0,
        is_whisper_boundary: false,
        can_split_after,
    }
}

/// Создание `SpeakerTurn` из списка юнитов для тестов.
pub fn make_turn(units: Vec<AlignedUnit>) -> SpeakerTurn {
    let start = units.first().map(|u| u.start).unwrap_or(Seconds::ZERO);
    let end = units.iter().map(|u| u.end).fold(start, |acc, e| acc.max(e));
    SpeakerTurn {
        id: "turn_001".to_string(),
        speaker: "SPEAKER_00".to_string(),
        start,
        end,
        units,
        is_overlap: false,
        context_group_id: None,
        continues_from_turn_id: None,
    }
}
