//! Облачная транскрипция (ASR) через сайдкар openrouter-helper (op `stt`, verbose_json с сегментами).
//! Возвращает сегменты (start, end, text) — analyze раздаёт их по спикерам через диаризацию, как локальный
//! Parakeet/Whisper. Тяжёлые локальные ASR-модели тогда не нужны (облачный пресет для слабых ПК).

use std::path::Path;

/// Транскрибировать wav выбранной STT-моделью OpenRouter -> сегменты (start, end, text). Нет ключа/модели ->
/// Err. `src_lang` — ISO-639-1 ("ru"/"en"/…) или "auto"/"" (авто-детект). Провайдер без сегментов (не
/// verbose_json) -> один сегмент на весь текст.
pub fn transcribe(models_root: &Path, wav: &Path, src_lang: &str) -> Result<Vec<(f64, f64, String)>, String> {
    let key = crate::models::openrouter_key(models_root).ok_or("облачный ASR включён, но ключ OpenRouter не задан")?;
    let model = crate::models::openrouter_model(models_root, "asr");
    if model.is_empty() {
        return Err("STT-модель OpenRouter не выбрана в настройках".into());
    }
    let repo = crate::openrouter_cli::repo_from_models(models_root);
    let lang = src_lang.trim();
    let mut payload = serde_json::json!({ "model": model, "audio": wav.to_string_lossy() });
    if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
        payload["language"] = serde_json::Value::String(lang.to_string());
    }
    let v = crate::openrouter_cli::run_json(&repo, &key, "stt", &payload)?;

    let mut out = Vec::new();
    if let Some(arr) = v.get("segments").and_then(|s| s.as_array()) {
        for s in arr {
            let st = s.get("start").and_then(|x| x.as_f64());
            let en = s.get("end").and_then(|x| x.as_f64());
            let tx = s.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if let (Some(st), Some(en)) = (st, en) {
                if !tx.is_empty() {
                    out.push((st, en, tx));
                }
            }
        }
    }
    if out.is_empty() {
        // Не verbose / без сегментов -> один сегмент на весь текст (лучше, чем ничего).
        let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Err("облачный STT вернул пустой транскрипт".into());
        }
        let dur = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
        out.push((0.0, dur.max(0.1), text));
    }
    Ok(out)
}
