//! Выбор LLM-провайдера для одной стадии: локальный llama-server (Gemma+mmproj) ИЛИ облако OpenRouter.
//! Переключение — по настройкам active.json (or_key + or_llm_on/or_vision_on). Абстрагирует 5 call-site'ов
//! (translate/compose/endpoints/analyze), где раньше был прямой `LlamaServer::start + ChatClient::new`.
//!
//! Инвариант: локальный путь неизменен (тот же ServerOpts/ubatch/mmproj). Облако требует НЕПУСТОЙ or_key —
//! иначе фабрика откатывается на локаль (fail-safe, как и вся translate-стадия).

use std::path::Path;

use dub_llm::{ChatClient, LlamaServer, ServerOpts};

/// Режим вызова: плоский текст (перевод/ремикс) или мультимодальный (vision-анализ кадров).
#[derive(Clone, Copy, PartialEq)]
pub enum LlmMode {
    Text,
    Vision,
}

/// Готовый провайдер: держит клиент чата + (для локали) живой llama-server, который глушится по Drop.
pub enum LlmProvider {
    /// Локальный сайдкар: сервер держим живым, пока провайдер в скоупе (Drop останавливает процесс).
    Local {
        _server: LlamaServer,
        client: ChatClient,
    },
    /// Облако OpenRouter: только HTTP-клиент, сервер не нужен.
    Remote {
        client: ChatClient,
    },
}

impl LlmProvider {
    pub fn client(&self) -> &ChatClient {
        match self {
            LlmProvider::Local { client, .. } => client,
            LlmProvider::Remote { client } => client,
        }
    }

    /// true, если это облачный путь (для логов/веток, где vision требует multimodal-модель).
    pub fn is_remote(&self) -> bool {
        matches!(self, LlmProvider::Remote { .. })
    }
}

/// Параметры открытия провайдера (пути локальных весов + models_root для чтения настроек).
pub struct LlmOpen<'a> {
    pub llama_bin: &'a Path,
    pub mt_model: &'a Path,
    pub mmproj: &'a Path,
    pub models_root: &'a Path,
}

/// Открыть провайдер для стадии. Облако — если включено в настройках и есть ключ; иначе локальный
/// llama-server (для Vision добавляем mmproj, если файл есть). Возвращает Err с человекочитаемой причиной.
pub fn open(o: &LlmOpen, mode: LlmMode) -> Result<LlmProvider, String> {
    let stage = if mode == LlmMode::Vision { "vision" } else { "llm" };
    if crate::models::openrouter_stage_on(o.models_root, stage) {
        let key = crate::models::openrouter_key(o.models_root)
            .ok_or("OpenRouter включён, но ключ не задан")?;
        let model = crate::models::openrouter_model(o.models_root, stage);
        // Модель не выбрана (без хардкода id) — облако невозможно; тихо откатываемся на локаль
        // вместо пустого id в API (fail-safe, как и вся стадия).
        if !model.trim().is_empty() {
            let client = ChatClient::openrouter(key, model).map_err(|e| e.to_string())?;
            return Ok(LlmProvider::Remote { client });
        }
    }

    if !o.llama_bin.is_file() {
        return Err(format!("llama-server не найден ({})", o.llama_bin.display()));
    }
    if !o.mt_model.is_file() {
        return Err(format!("GGUF Gemma не найден ({})", o.mt_model.display()));
    }
    let mut opts = ServerOpts::new(o.llama_bin, o.mt_model)
        .with_ubatch(crate::models::sel_num(o.models_root, "llama_ubatch").map(|f| f as u32));
    if mode == LlmMode::Vision && o.mmproj.is_file() {
        opts = opts.with_mmproj(o.mmproj);
    }
    let server = LlamaServer::start(&opts).map_err(|e| format!("llama-server: {e}"))?;
    let client = ChatClient::new(server.base_url()).map_err(|e| format!("клиент чата: {e}"))?;
    Ok(LlmProvider::Local { _server: server, client })
}
