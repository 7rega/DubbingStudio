//! Подсистема формирования естественных фраз (Natural Phrases) для нейродубляжа.
//!
//! Модуль обеспечивает объединение ASR-токенов/слов и интервалов диаризации
//! в естественные реплики на основе динамического программирования (DP)
//! с оптимизацией пауз, пунктуации и синтаксических связей.

pub mod config;
pub mod profile;
pub mod time;
pub mod types;

pub use config::{ConfigError, SegmentationConfig};
pub use profile::{
    normalize_language_tag, resolve_source_language, ChineseProfile, DefaultProfile,
    EnglishProfile, JapaneseProfile, KoreanProfile, LanguageProfile, ProfileRegistry,
    RussianProfile,
};
pub use time::{Millis, Seconds};
pub use types::*;
