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
    /// Темп речи голоса этой реплики (символов/с, dub_core::fit) для бюджета длины перевода; None —
    /// табличный темп целевого языка.
    pub cps: Option<f64>,
}

impl Seg {
    /// Мягкий лимит длины перевода этой реплики: её длительность × темп голоса (иначе табличный темп
    /// целевого языка `tgt_lang`); без таймингов — без лимита.
    pub(crate) fn budget(&self, tgt_lang: &str) -> Option<usize> {
        let cps = self.cps.unwrap_or_else(|| dub_core::fit::table_cps(tgt_lang));
        dub_core::fit::char_budget(self.end - self.start, cps)
    }

    pub fn new(text: impl Into<String>, speaker: i64) -> Self {
        Seg {
            text: text.into(),
            tgt: String::new(),
            start: 0.0,
            end: 0.0,
            speaker,
            cps: None,
        }
    }
}
