//! dub-llm — сайдкар llama.cpp (llama-server) + OpenAI-совместимый чат-клиент с vision.
//!
//! Штатный способ работы llama.cpp с мультимодальностью: поднять llama-server (свободный localhost-порт),
//! слать /v1/chat/completions с картинками (base64 в content), выключить на Drop. Порт того, как
//! translate.py / ctx_translate.py грузят Gemma один раз и делают серию сфокусированных вызовов.
//!
//! Дефолты моделей (Gemma-4 12B QAT q4_0 + mmproj) — из dub-core EngineOpts; их сюда передаёт сервер.

mod client;
mod server;

pub use client::{ChatClient, Message, Part, Sampling};
pub use server::{resolve_llama_bin, LlamaServer, ServerOpts};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("llama-server spawn: {0}")]
    Spawn(String),
    #[error("http: {0}")]
    Http(String),
    #[error("api: {0}")]
    Api(String),
}

/// Обрезать блок рассуждений <think>...</think> — как re.sub(r"<think>.*?</think>", "", ...) в питоне.
/// Нужно на КАЖДОМ ответе Gemma (translate.py делает это везде).
pub fn strip_think(s: &str) -> String {
    // 1:1 с питоном: re.sub(r"<think>.*?</think>", "", DOTALL).strip() — РЕГИСТРОЗАВИСИМО, нежадно.
    // Убираем только ЗАКРЫТЫЕ пары; незакрытый <think> (нет </think>) НЕ матчится -> остаётся как есть,
    // текст после него сохраняется (Rust раньше выбрасывал хвост и был регистронезависим — обе ошибки).
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("<think>") {
        let after_open = &rest[open + "<think>".len()..];
        if let Some(close) = after_open.find("</think>") {
            out.push_str(&rest[..open]); // текст до блока
            rest = &after_open[close + "</think>".len()..]; // продолжаем после блока
        } else {
            // нет закрытия -> не матч; отдаём всё до и включая '<think>', сканируем дальше
            out.push_str(&rest[..open + "<think>".len()]);
            rest = after_open;
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_removes_block() {
        assert_eq!(strip_think("<think>reason</think>hello"), "hello");
        assert_eq!(strip_think("a<think>x</think>b"), "ab");
        assert_eq!(strip_think("plain"), "plain");
    }

    #[test]
    fn strip_think_keeps_unicode() {
        assert_eq!(strip_think("<think>y</think>привет"), "привет");
        assert_eq!(strip_think("こんにちは"), "こんにちは");
    }

    #[test]
    fn strip_think_parity_with_python() {
        // незакрытый <think> НЕ матчится питон-регексом -> остаётся вместе с хвостом
        assert_eq!(strip_think("<think>no close here"), "<think>no close here");
        assert_eq!(strip_think("keep <think>tail"), "keep <think>tail");
        // регистрозависимо: <THINK> питон (без re.I) не трогает
        assert_eq!(strip_think("<THINK>x</THINK>hi"), "<THINK>x</THINK>hi");
        // закрытая пара всё ещё убирается даже если рядом есть незакрытый ниже
        assert_eq!(strip_think("a<think>b</think>c<think>d"), "ac<think>d");
    }
}
