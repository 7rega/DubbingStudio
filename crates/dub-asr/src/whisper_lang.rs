//! Преобразование кодов языка в токены Whisper (<|ru|>, <|en|>, ...) для модели CrisperWhisper.

/// Языки, поддерживаемые Whisper (100 языков).
pub const WHISPER_LANGUAGES: &[&str] = &[
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy", "da", "de", "el", "en", "es", "et",
    "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw", "he", "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka",
    "kk", "km", "kn", "ko", "la", "lb", "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "ne", "nl",
    "nn", "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si", "sk", "sl", "sn", "so", "sq", "sr", "su", "sv", "sw",
    "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz", "vi", "yi", "yo", "yue", "zh",
];

/// Языки VoxLingua107, для которых в словаре Whisper нет токенов.
const NOT_IN_WHISPER: &[(&str, &str)] = &[
    ("ab", "Abkhazian"),
    ("ceb", "Cebuano"),
    ("eo", "Esperanto"),
    ("gn", "Guarani"),
    ("gv", "Manx"),
    ("ia", "Interlingua"),
    ("sco", "Scots"),
    ("war", "Waray"),
];

/// Преобразование кода языка (VoxLingua107 или пользовательского вроде "pt-BR", "ru-RU") в код Whisper.
pub fn whisper_code(code: &str) -> Result<&'static str, String> {
    let lower = code.trim().to_ascii_lowercase();
    let primary = lower.split(['-', '_']).next().unwrap_or("");
    let code = match primary {
        "iw" | "he" => "he",
        "jv" | "jw" => "jw",
        "fil" | "tl" => "tl",
        "nb" | "no" => "no",
        other => other,
    };

    if let Some((_, name)) = NOT_IN_WHISPER.iter().find(|(c, _)| *c == code) {
        return Err(format!(
            "язык аудио определён как {name}, но CrisperWhisper его не поддерживает: укажите язык оригинала вручную"
        ));
    }

    WHISPER_LANGUAGES
        .iter()
        .copied()
        .find(|c| *c == code)
        .ok_or_else(|| {
            format!("код языка «{code}» не поддерживается словарем Whisper: укажите язык оригинала вручную")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whisper_code_mappings() {
        assert_eq!(whisper_code("ru").unwrap(), "ru");
        assert_eq!(whisper_code("ru-RU").unwrap(), "ru");
        assert_eq!(whisper_code("pt-BR").unwrap(), "pt");
        assert_eq!(whisper_code("en-US").unwrap(), "en");
        assert_eq!(whisper_code("iw").unwrap(), "he");
        assert_eq!(whisper_code("jw").unwrap(), "jw");
        assert!(whisper_code("eo").is_err());
        assert!(whisper_code("unknown_lang").is_err());
    }
}
