//! Подсистема формирования естественных фраз (Natural Phrases) для нейродубляжа.
//!
//! Модуль обеспечивает объединение ASR-токенов/слов и интервалов диаризации
//! в естественные реплики на основе динамического программирования (DP)
//! с оптимизацией пауз, пунктуации и синтаксических связей.

pub mod config;
pub mod time;
pub mod types;

pub use config::{ConfigError, SegmentationConfig};
pub use time::{Millis, Seconds};
pub use types::*;
