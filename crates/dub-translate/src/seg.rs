//! Seg — минимальный сегмент для стадии перевода. Зеркало питоновского dict {text, tgt, start, end,
//! speaker}, которым оперируют translate.py / ctx_translate.py. Отдельно от dub_core::Segment, чтобы
//! стадия перевода не зависела от полной модели Project (маппинг делает сервер).

#[derive(Clone, Debug, Default)]
pub struct Seg {
    pub text: String,   // исходный текст (ASR) — s["text"]
    pub tgt: String,    // перевод — s["tgt"]
    pub start: f64,
    pub end: f64,
    pub speaker: i64,   // s.get("speaker", 0)
}

impl Seg {
    pub fn new(text: impl Into<String>, speaker: i64) -> Self {
        Seg {
            text: text.into(),
            tgt: String::new(),
            start: 0.0,
            end: 0.0,
            speaker,
        }
    }
}
