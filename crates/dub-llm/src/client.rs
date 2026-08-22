//! OpenAI-совместимый чат-клиент к llama-server (/v1/chat/completions). Порт вызовов llama_cpp
//! create_chat_completion из translate.py / ctx_translate.py: system+user сообщения, sampling-параметры
//! (temperature/top_p/top_k/repeat_penalty), max_tokens; content как строка ИЛИ массив частей
//! (text / image_url data:base64 / input_audio) для мультимодальности через mmproj.
//!
//! <think>...</think> вырезаем на стороне вызывающего (как в питоне) — тут отдаём сырой content.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::LlmError;

/// Часть сообщения content. Соответствует list-форме content в питоне:
///   {"type":"text","text":...} | {"type":"image_url","image_url":{"url":"data:image/png;base64,..."}}
///   | {"type":"input_audio","input_audio":{"data":<base64>,"format":"wav"}}
#[derive(Clone)]
pub enum Part {
    Text(String),
    /// PNG-кадр как base64 (без префикса) — обёрнём в data:image/png;base64,.
    ImagePngB64(String),
    /// WAV-аудио как base64 (без префикса) + формат ("wav").
    AudioB64 { data: String, format: String },
}

impl Part {
    fn to_value(&self) -> Value {
        match self {
            Part::Text(t) => json!({"type":"text","text": t}),
            Part::ImagePngB64(b64) => json!({
                "type":"image_url",
                "image_url": {"url": format!("data:image/png;base64,{b64}")}
            }),
            Part::AudioB64 { data, format } => json!({
                "type":"input_audio",
                "input_audio": {"data": data, "format": format}
            }),
        }
    }
}

/// Роль + content одного сообщения. content — либо строка, либо массив частей (мультимодальность).
#[derive(Clone)]
pub struct Message {
    pub role: String,
    pub parts_text: Option<String>, // строковый content (быстрый путь для чисто текстовых вызовов)
    pub parts: Option<Vec<Part>>,   // массив частей (image/audio/text)
}

impl Message {
    /// Текстовое сообщение с заданной ролью (общий конструктор system/user_text).
    fn text_msg(role: &str, text: impl Into<String>) -> Self {
        Message {
            role: role.into(),
            parts_text: Some(text.into()),
            parts: None,
        }
    }
    pub fn system(text: impl Into<String>) -> Self {
        Self::text_msg("system", text)
    }
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::text_msg("user", text)
    }
    pub fn user_parts(parts: Vec<Part>) -> Self {
        Message {
            role: "user".into(),
            parts_text: None,
            parts: Some(parts),
        }
    }

    fn to_value(&self) -> Value {
        let content = if let Some(t) = &self.parts_text {
            Value::String(t.clone())
        } else if let Some(ps) = &self.parts {
            Value::Array(ps.iter().map(|p| p.to_value()).collect())
        } else {
            Value::String(String::new())
        };
        json!({"role": self.role, "content": content})
    }
}

/// Sampling-параметры одного вызова. Дефолты нейтральны; вызывающий выставляет ровно то, что в питоне.
#[derive(Clone, Serialize)]
pub struct Sampling {
    pub temperature: f32,
    pub top_p: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    pub max_tokens: u32,
}

impl Sampling {
    pub fn new(temperature: f32, top_p: f32, max_tokens: u32) -> Self {
        Sampling {
            temperature,
            top_p,
            top_k: None,
            repeat_penalty: None,
            max_tokens,
        }
    }
    pub fn top_k(mut self, k: i32) -> Self {
        self.top_k = Some(k);
        self
    }
    pub fn repeat_penalty(mut self, r: f32) -> Self {
        self.repeat_penalty = Some(r);
        self
    }
    pub fn repetition_penalty(mut self, r: f32) -> Self {
        self.repeat_penalty = Some(r);
        self
    }
}

/// Клиент чата к OpenAI-совместимому эндпоинту. По умолчанию — локальный llama-server (base_url =
/// http://127.0.0.1:PORT, без ключа и без поля model). Для облака (OpenRouter) — `openrouter()`:
/// base_url = https://openrouter.ai/api, Authorization: Bearer <key>, обязательное поле `model`,
/// repetition_penalty (вместо локального repeat_penalty), БЕЗ llama-специфичного chat_template_kwargs.
/// Держит blocking reqwest client с большим таймаутом (генерация целого транскрипта/vision-кадра может идти десятки секунд).
pub struct ChatClient {
    base_url: String,
    http: reqwest::blocking::Client,
    retries: u32,
    /// Bearer-токен (OpenRouter/OpenAI); None у локального llama-server.
    auth: Option<String>,
    /// id модели в теле запроса (обязателен для OpenRouter). None у llama-server (единственная модель).
    model: Option<String>,
    /// Заголовки атрибуции OpenRouter (HTTP-Referer, X-Title) — рекомендованы, не обязательны.
    referer: Option<String>,
    title: Option<String>,
}

impl ChatClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, LlmError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600)) // целый транскрипт/vision-кадр может генериться долго
            .build()
            .map_err(|e| LlmError::Http(e.to_string()))?;
        Ok(ChatClient {
            base_url: base_url.into(),
            http,
            retries: 2,
            auth: None,
            model: None,
            referer: None,
            title: None,
        })
    }

    /// Клиент к OpenRouter (OpenAI-совместимый): фиксированный base_url + ключ + id модели. `model`
    /// подставляется в тело каждого запроса; chat_template_kwargs НЕ шлётся (это llama-специфика).
    pub fn openrouter(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self, LlmError> {
        let mut c = Self::new("https://openrouter.ai/api")?;
        c.auth = Some(api_key.into());
        c.model = Some(model.into());
        c.referer = Some("https://github.com/timoncool/dub-studio".to_string());
        c.title = Some("Dub Studio".to_string());
        Ok(c)
    }

    pub fn with_retries(mut self, n: u32) -> Self {
        self.retries = n;
        self
    }

    /// remote-режим = задан id модели (OpenRouter/OpenAI): шлём `model` и `repetition_penalty`, не шлём chat_template_kwargs.
    pub fn is_remote(&self) -> bool {
        self.model.is_some()
    }

    /// Собрать JSON-тело запроса с учётом провайдера (OpenRouter vs локальный llama-server).
    pub fn build_body(&self, messages: &[Message], s: &Sampling) -> Value {
        let mut body = serde_json::Map::new();
        if let Some(m) = &self.model {
            body.insert("model".into(), json!(m)); // OpenRouter требует id модели; llama-server игнорит
        }
        body.insert(
            "messages".into(),
            Value::Array(messages.iter().map(|m| m.to_value()).collect()),
        );
        body.insert("temperature".into(), json!(s.temperature));
        body.insert("top_p".into(), json!(s.top_p));
        if let Some(k) = s.top_k {
            body.insert("top_k".into(), json!(k));
        }
        if let Some(rp) = s.repeat_penalty {
            if self.is_remote() {
                // OpenRouter API использует стандартный параметр repetition_penalty
                body.insert("repetition_penalty".into(), json!(rp));
            } else {
                // Локальный llama-server использует repeat_penalty
                body.insert("repeat_penalty".into(), json!(rp));
            }
        }
        body.insert("max_tokens".into(), json!(s.max_tokens));
        body.insert("stream".into(), json!(false));
        if !self.is_remote() {
            // enable_thinking=false — как Gemma4ChatHandler(enable_thinking=False): без него Gemma-4
            // сжигает max_tokens на reasoning_content, content пустой. Это llama-специфика — облаку не шлём.
            body.insert(
                "chat_template_kwargs".into(),
                json!({"enable_thinking": false}),
            );
        }
        Value::Object(body)
    }

    /// Один вызов /v1/chat/completions -> текст ассистента (сырой, без стрипа <think>). Ретраит на
    /// сетевых/5xx-ошибках (сервер мог ещё догружать слот). Пустой ответ — не ошибка (вернём "").
    pub fn chat(&self, messages: &[Message], s: &Sampling) -> Result<String, LlmError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = self.build_body(messages, s);

        let mut last_err = String::new();
        for attempt in 0..=self.retries {
            let mut req = self.http.post(&url).json(&body);
            if let Some(key) = &self.auth {
                req = req.bearer_auth(key);
            }
            if let Some(r) = &self.referer {
                req = req.header("HTTP-Referer", r);
            }
            if let Some(tt) = &self.title {
                req = req.header("X-Title", tt);
            }
            match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().unwrap_or_default();
                    match parse_chat_response(status, &text) {
                        Ok(txt) => return Ok(txt),
                        Err(e) => {
                            // 4xx (кроме временных 429) или неподдерживаемые фичи (not supported) — не ретраим.
                            let is_quota_429 = status.as_u16() == 429
                                && (text.contains("free-models-per-day")
                                    || text.contains("credits")
                                    || text.contains("quota"));
                            if (status.as_u16() != 429 && status.as_u16() < 500)
                                || text.contains("not supported")
                                || is_quota_429
                            {
                                return Err(e);
                            }
                            last_err = e.to_string();
                        }
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
            if attempt < self.retries {
                std::thread::sleep(Duration::from_millis(500 * (attempt as u64 + 1)));
            }
        }
        Err(LlmError::Api(format!(
            "chat failed after {} retries: {last_err}",
            self.retries + 1
        )))
    }
}

/// Разобрать ответ от /v1/chat/completions с извлечением текста и обработкой ошибок API.
pub(crate) fn parse_chat_response(status: reqwest::StatusCode, text: &str) -> Result<String, LlmError> {
    if status.is_success() {
        let v: Value = serde_json::from_str(text)
            .map_err(|e| LlmError::Http(format!("parse json: {e} (body: {text})")))?;

        if let Some(err) = v.get("error") {
            let msg = if let Some(m) = err.get("message").and_then(|m| m.as_str()) {
                m.to_string()
            } else {
                err.to_string()
            };
            return Err(LlmError::Api(format!("OpenRouter error: {msg}")));
        }

        let choices = v.get("choices").and_then(|c| c.as_array());
        if let Some(choices_arr) = choices {
            if let Some(first_choice) = choices_arr.first() {
                if let Some(refusal) = first_choice.pointer("/message/refusal").and_then(|r| r.as_str()) {
                    if !refusal.trim().is_empty() {
                        return Err(LlmError::Api(format!("model refusal: {refusal}")));
                    }
                }
                let content_val = first_choice.pointer("/message/content");
                if let Some(s) = content_val.and_then(|c| c.as_str()) {
                    return Ok(s.to_string());
                } else if let Some(arr) = content_val.and_then(|c| c.as_array()) {
                    let combined = arr
                        .iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(combined);
                } else if let Some(s) = first_choice.get("text").and_then(|t| t.as_str()) {
                    return Ok(s.to_string());
                } else if content_val == Some(&Value::Null) {
                    return Ok(String::new());
                }
            } else {
                return Err(LlmError::Api(format!("empty choices array in response: {text}")));
            }
        } else {
            return Err(LlmError::Api(format!("missing choices in response: {text}")));
        }
    }

    let err_msg = if let Ok(json_err) = serde_json::from_str::<Value>(text) {
        if let Some(m) = json_err.pointer("/error/message").and_then(|m| m.as_str()) {
            format!("{status}: {m}")
        } else {
            format!("{status}: {text}")
        }
    } else {
        format!("{status}: {text}")
    };

    Err(LlmError::Api(err_msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_builder() {
        let s = Sampling::new(0.7, 0.6, 512).top_k(20).repetition_penalty(1.05);
        assert_eq!(s.temperature, 0.7);
        assert_eq!(s.top_p, 0.6);
        assert_eq!(s.top_k, Some(20));
        assert_eq!(s.repeat_penalty, Some(1.05));
        assert_eq!(s.max_tokens, 512);
    }

    #[test]
    fn test_body_openrouter_vs_local() {
        let remote = ChatClient::openrouter("test-key", "google/gemini-2.5-flash").unwrap();
        let local = ChatClient::new("http://127.0.0.1:8080").unwrap();
        let s = Sampling::new(0.3, 0.9, 100).top_k(20).repeat_penalty(1.05);
        let msgs = vec![Message::user_text("Hello")];

        let r_body = remote.build_body(&msgs, &s);
        assert_eq!(r_body.get("model").and_then(|v| v.as_str()), Some("google/gemini-2.5-flash"));
        assert_eq!(r_body.get("repetition_penalty").and_then(|v| v.as_f64()), Some(1.05));
        assert!(r_body.get("repeat_penalty").is_none());
        assert!(r_body.get("chat_template_kwargs").is_none());

        let l_body = local.build_body(&msgs, &s);
        assert!(l_body.get("model").is_none());
        assert_eq!(l_body.get("repeat_penalty").and_then(|v| v.as_f64()), Some(1.05));
        assert!(l_body.get("repetition_penalty").is_none());
        assert!(l_body.get("chat_template_kwargs").is_some());
    }

    #[test]
    fn test_parse_chat_response_success() {
        let json_resp = r#"{
            "id": "gen-123",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "Привет, мир!"
                    },
                    "finish_reason": "stop"
                }
            ]
        }"#;
        let res = parse_chat_response(reqwest::StatusCode::OK, json_resp);
        assert_eq!(res.unwrap(), "Привет, мир!");
    }

    #[test]
    fn test_parse_chat_response_parts_array() {
        let json_resp = r#"{
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "Часть 1 "},
                            {"type": "text", "text": "Часть 2"}
                        ]
                    }
                }
            ]
        }"#;
        let res = parse_chat_response(reqwest::StatusCode::OK, json_resp);
        assert_eq!(res.unwrap(), "Часть 1 Часть 2");
    }

    #[test]
    fn test_parse_chat_response_api_error_in_200() {
        let json_resp = r#"{
            "error": {
                "message": "User has insufficient credits",
                "code": 402
            }
        }"#;
        let res = parse_chat_response(reqwest::StatusCode::OK, json_resp);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "api: OpenRouter error: User has insufficient credits");
    }

    #[test]
    fn test_parse_chat_response_http_error() {
        let json_resp = r#"{"error":{"message":"Invalid API key","code":401}}"#;
        let res = parse_chat_response(reqwest::StatusCode::UNAUTHORIZED, json_resp);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "api: 401 Unauthorized: Invalid API key");
    }
}
