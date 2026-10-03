//! Облачная транскрипция (ASR) через OpenRouter (`/audio/transcriptions`, verbose_json с сегментами) напрямую
//! из Rust. Возвращает сегменты (start, end, text) — analyze раздаёт их по спикерам через диаризацию, как
//! локальный Parakeet/Whisper. Тяжёлые локальные ASR-модели тогда не нужны (облачный пресет для слабых ПК).

use std::path::Path;

use dub_llm::openrouter::OpenRouter;

/// Транскрибировать wav выбранной STT-моделью OpenRouter -> сегменты (start, end, text). Нет ключа/модели ->
/// Err. `src_lang` — ISO-639-1 ("ru"/"en"/…) или "auto"/"" (авто-детект). Модель без сегментов (не
/// verbose_json) -> один сегмент на весь текст.
pub fn transcribe(models_root: &Path, wav: &Path, src_lang: &str) -> Result<Vec<(f64, f64, String)>, String> {
    let key = crate::models::openrouter_key().ok_or("облачный ASR включён, но ключ OpenRouter не задан")?;
    let model = crate::models::openrouter_model(models_root, "asr");
    if model.is_empty() {
        return Err("STT-модель OpenRouter не выбрана в настройках".into());
    }
    let audio = std::fs::read(wav).map_err(|e| format!("облачный ASR: {}: {e}", wav.display()))?;
    let client = OpenRouter::new(Some(key)).map_err(|e| format!("облачный ASR: {e:#}"))?;
    let transcript = client
        .transcribe(&model, &audio, "wav", src_lang)
        .map_err(|e| format!("облачный ASR: {e:#}"))?;

    let out: Vec<(f64, f64, String)> = transcript.segments.into_iter().map(|s| (s.start, s.end, s.text)).collect();
    if !out.is_empty() {
        return Ok(out);
    }
    // Не verbose / без сегментов -> один сегмент на весь текст (лучше, чем ничего).
    if transcript.text.is_empty() {
        return Err("облачный STT вернул пустой транскрипт".into());
    }
    Ok(vec![(0.0, transcript.duration.unwrap_or(0.0).max(0.1), transcript.text)])
}
