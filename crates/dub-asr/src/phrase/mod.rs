//! Подсистема формирования естественных фраз (Natural Phrases) для нейродубляжа.
//!
//! Модуль обеспечивает объединение ASR-токенов/слов и интервалов диаризации
//! в естественные реплики на основе динамического программирования (DP)
//! с оптимизацией пауз, пунктуации и синтаксических связей.

pub mod aligner;
pub mod config;
pub mod dp;
pub mod profile;
pub mod time;
pub mod turns;
pub mod types;

#[cfg(test)]
pub mod test_utils;

pub use aligner::{align_and_smooth, align_units, smooth_aligned_units};
pub use config::{ConfigError, SegmentationConfig};
pub use dp::{
    compute_boundary_score, compute_effective_gap, compute_length_penalty, compute_segment_cost,
    segment_turn_dp, DpResult, RawSegment,
};
pub use profile::{
    normalize_language_tag, resolve_source_language, ChineseProfile, DefaultProfile,
    EnglishProfile, JapaneseProfile, KoreanProfile, LanguageProfile, ProfileRegistry,
    RussianProfile,
};
pub use time::{Millis, Seconds};
pub use turns::{build_speaker_turns, detect_overlaps};
pub use types::*;
