//! dub-translate — стадия перевода + vision над dub-llm (Gemma через llama-server).
//!
//! Порт dubengine/translate.py (плоский MT + rewrite) и ctx_translate.py (единый Gemma-проход: vision
//! layout/scene + audio-контекст + перевод всего транскрипта с контекстом). Промпты и параметры сэмплинга
//! перенесены ДОСЛОВНО — они выверены на тест-сете продукта. Дефолт-модель Gemma-4 12B QAT + mmproj.

mod ctx;
mod seg;
mod translate;
mod vision;

pub use ctx::{run as ctx_run, CtxConfig, CtxResult};
pub use seg::Seg;
pub use translate::{rewrite as flat_rewrite, run as flat_run};
pub use vision::{analyze_layout, classify_content_type, is_counter, scene_context, Layout, FONTS};

use thiserror::Error;

/// Код языка -> английское имя для промпта Gemma. Полный набор Whisper large-v3 (99 языков) —
/// совпадает со списком, который распознаёт Whisper-ASR (истинный потолок source). Для незнакомого
/// кода вызывающая сторона подставляет сам код. Единый источник для translate.rs и ctx.rs.
pub const WHISPER_LANGS: &[(&str, &str)] = &[
    ("en", "English"), ("zh", "Chinese"), ("de", "German"), ("es", "Spanish"), ("ru", "Russian"),
    ("ko", "Korean"), ("fr", "French"), ("ja", "Japanese"), ("pt", "Portuguese"), ("tr", "Turkish"),
    ("pl", "Polish"), ("ca", "Catalan"), ("nl", "Dutch"), ("ar", "Arabic"), ("sv", "Swedish"),
    ("it", "Italian"), ("id", "Indonesian"), ("hi", "Hindi"), ("fi", "Finnish"), ("vi", "Vietnamese"),
    ("he", "Hebrew"), ("uk", "Ukrainian"), ("el", "Greek"), ("ms", "Malay"), ("cs", "Czech"),
    ("ro", "Romanian"), ("da", "Danish"), ("hu", "Hungarian"), ("ta", "Tamil"), ("no", "Norwegian"),
    ("th", "Thai"), ("ur", "Urdu"), ("hr", "Croatian"), ("bg", "Bulgarian"), ("lt", "Lithuanian"),
    ("la", "Latin"), ("mi", "Maori"), ("ml", "Malayalam"), ("cy", "Welsh"), ("sk", "Slovak"),
    ("te", "Telugu"), ("fa", "Persian"), ("lv", "Latvian"), ("bn", "Bengali"), ("sr", "Serbian"),
    ("az", "Azerbaijani"), ("sl", "Slovenian"), ("kn", "Kannada"), ("et", "Estonian"), ("mk", "Macedonian"),
    ("br", "Breton"), ("eu", "Basque"), ("is", "Icelandic"), ("hy", "Armenian"), ("ne", "Nepali"),
    ("mn", "Mongolian"), ("bs", "Bosnian"), ("kk", "Kazakh"), ("sq", "Albanian"), ("sw", "Swahili"),
    ("gl", "Galician"), ("mr", "Marathi"), ("pa", "Punjabi"), ("si", "Sinhala"), ("km", "Khmer"),
    ("sn", "Shona"), ("yo", "Yoruba"), ("so", "Somali"), ("af", "Afrikaans"), ("oc", "Occitan"),
    ("ka", "Georgian"), ("be", "Belarusian"), ("tg", "Tajik"), ("sd", "Sindhi"), ("gu", "Gujarati"),
    ("am", "Amharic"), ("yi", "Yiddish"), ("lo", "Lao"), ("uz", "Uzbek"), ("fo", "Faroese"),
    ("ht", "Haitian Creole"), ("ps", "Pashto"), ("tk", "Turkmen"), ("nn", "Nynorsk"), ("mt", "Maltese"),
    ("sa", "Sanskrit"), ("lb", "Luxembourgish"), ("my", "Burmese"), ("bo", "Tibetan"), ("tl", "Tagalog"),
    ("mg", "Malagasy"), ("as", "Assamese"), ("tt", "Tatar"), ("haw", "Hawaiian"), ("ln", "Lingala"),
    ("ha", "Hausa"), ("ba", "Bashkir"), ("jw", "Javanese"), ("su", "Sundanese"), ("yue", "Cantonese"),
];

#[derive(Debug, Error)]
pub enum TranslateError {
    #[error("llm: {0}")]
    Llm(#[from] dub_llm::LlmError),
    #[error("frame extract: {0}")]
    Frame(String),
    #[error("audio ctx: {0}")]
    Audio(String),
    #[error("MT returned empty for all {0} segments")]
    Empty(usize),
}
