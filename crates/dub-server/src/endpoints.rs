//! Остаток REST-контракта (раунд 5): каталоги (/fonts /voices /presets), PATCH /engine/opts,
//! POST /remix (Gemma переписывает весь транскрипт), PUT /projects/{pid} (undo/redo снапшот),
//! GET /waveform (пики аудио), GET /preview?t&rev + GET /original?t (ОДИН PNG-кадр через GPU-воркер).
//! Порт соответствующих ручек backend/app.py 1:1.

use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use dub_core::Project;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::{jobs, AppState};

// ─── GET /fonts ─────────────────────────────────────────────────────────────
pub async fn fonts() -> Json<Value> {
    Json(json!({ "fonts": dub_captions::fonts_catalog() }))
}


// ─── GET /presets ───────────────────────────────────────────────────────────
pub async fn presets() -> Json<Value> {
    let (presets, reveals) = dub_captions::presets_catalog();
    Json(json!({ "presets": presets, "reveals": reveals }))
}

// ─── GET /engine/openrouter/models?kind=llm|vision|tts|asr ──────────────────
// Модели каталога OpenRouter, подходящие стадии (llm = текст -> текст; vision = картинка -> текст;
// tts = синтез речи; asr = транскрипция): id, имя, цены (строки USD за токен, как у OpenRouter), контекст,
// голоса. Каталог — из кэша (память -> диск -> сеть), ключ не нужен.
pub async fn openrouter_models(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let kind = q.get("kind").map(String::as_str).unwrap_or("llm");
    let Some(capability) = dub_llm::openrouter::Capability::parse(kind) else {
        return (StatusCode::BAD_REQUEST, format!("unknown model kind: {kind:?}")).into_response();
    };
    let models_root = st.models_root.clone();
    match tokio::task::spawn_blocking(move || crate::openrouter::catalog(&models_root)).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(cached) => {
            let models: Vec<Value> = cached.catalog().models_for(capability).map(model_view).collect();
            Json(json!({ "models": models, "refreshed_at": cached.refreshed_at })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("каталог OpenRouter: {e}")).into_response(),
    }
}

/// Модель каталога для окна: без служебных полей чат-клиента.
fn model_view(model: &dub_llm::openrouter::CatalogModel) -> Value {
    json!({
        "id": model.id,
        "name": model.name,
        "context_length": model.context_length,
        "pricing": model.pricing,
        "input_modalities": model.input_modalities,
        "voices": model.voices,
    })
}

// ─── GET /engine/openrouter/catalog, POST /engine/openrouter/catalog/refresh ─
// Сводка каталога: сколько моделей по стадиям и когда он обновлялся; refresh — скачать заново.
fn catalog_summary(cached: &crate::openrouter::CachedCatalog) -> Value {
    use dub_llm::openrouter::Capability;
    let catalog = cached.catalog();
    let count = |capability| catalog.models_for(capability).count();
    json!({
        "refreshed_at": cached.refreshed_at,
        "total": cached.models.len(),
        "counts": {
            "llm": count(Capability::Text),
            "vision": count(Capability::Vision),
            "tts": count(Capability::Speech),
            "asr": count(Capability::Transcription),
        },
    })
}

pub async fn openrouter_catalog(State(st): State<AppState>) -> Response {
    let models_root = st.models_root.clone();
    match tokio::task::spawn_blocking(move || crate::openrouter::catalog(&models_root)).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(cached) => Json(catalog_summary(&cached)).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("каталог OpenRouter: {e}")).into_response(),
    }
}

pub async fn openrouter_catalog_refresh(State(st): State<AppState>) -> Response {
    let models_root = st.models_root.clone();
    match tokio::task::spawn_blocking(move || crate::openrouter::refresh(&models_root)).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(cached) => Json(catalog_summary(&cached)).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("каталог OpenRouter: {e}")).into_response(),
    }
}

// ─── POST /engine/openrouter/verify {key} ───────────────────────────────────
// Проверка ключа OpenRouter (GET /key) без сохранения. Ключ из формы, НЕ логируется. Ответ:
// {ok:true, data:{label, limit, usage, …}} | {ok:false, error}.
pub async fn openrouter_verify(Json(body): Json<Value>) -> Response {
    let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if key.is_empty() {
        return (StatusCode::BAD_REQUEST, "пустой ключ").into_response();
    }
    let res = tokio::task::spawn_blocking(move || {
        dub_llm::openrouter::OpenRouter::new(Some(key)).and_then(|client| client.check_key()).map_err(|e| format!("{e:#}"))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(dub_llm::openrouter::KeyCheck::Accepted(data)) => Json(json!({ "ok": true, "data": data })).into_response(),
        Ok(dub_llm::openrouter::KeyCheck::Rejected(detail)) => Json(json!({ "ok": false, "error": detail })).into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

// ─── GET /engine/server/models?url= — модели локального OpenAI-совместимого сервера ──────────────────────
// Сервер пользователя (Ollama, LM Studio, vLLM, llama-server) отвечает на GET <адрес>/v1/models списком
// {data:[{id}]}. Спрашиваем через сервер студии, чтобы окно не ходило на чужой адрес само. Без url — адрес из
// настроек. Ключ из хранилища уходит только на адрес, для которого его сохранили.
pub async fn server_models(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let url = q
        .get("url")
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| crate::models::server_url(&st.models_root));
    let res = tokio::task::spawn_blocking(move || list_server_models(&url, crate::credentials::local_server_key_for(&url)))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(models) => Json(json!({ "models": models })).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": "server_unreachable", "detail": e }))).into_response(),
    }
}

/// id моделей сервера по адресу `url` (с `/v1` или без).
pub(crate) fn list_server_models(url: &str, key: Option<String>) -> Result<Vec<String>, String> {
    let base = dub_llm::server_base(url);
    if base.is_empty() {
        return Err("адрес сервера не задан".into());
    }
    let endpoint = format!("{base}/v1/models");
    let client = dub_llm::net::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let mut request = client.get(&endpoint);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().map_err(|e| format!("сервер {base} не ответил: {}", dub_llm::net::why(&e)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{endpoint} ответил {status}"));
    }
    let body: Value = response.json().map_err(|e| format!("{endpoint} вернул не JSON: {e}"))?;
    let models = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{endpoint} вернул не список моделей OpenAI (нет поля data)"))?;
    Ok(models.iter().filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string)).collect())
}

// ─── POST /engine/proxy/test {mode?, kind?, url, password?} — проверить связность ──────────────────────────
// Отвечают ли HF (закачка моделей) и OpenRouter (облако) при таком прокси — до сохранения. mode по умолчанию
// custom (прежняя форма {url}); прямой доступ — mode=off, прокси Windows — mode=system. Ответ с любым
// HTTP-статусом считается: меряем транспорт. 15 с на каждый, оба параллельно, причины ошибок цепочкой.
pub async fn proxy_test(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    let mode = match body.get("mode").and_then(Value::as_str) {
        None => dub_llm::net::ProxyMode::Custom,
        Some(mode) => match dub_llm::net::ProxyMode::parse(mode) {
            Some(mode) => mode,
            None => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_proxy_mode", "detail": mode }))).into_response(),
        },
    };
    let kind = match body.get("kind").and_then(Value::as_str) {
        None => crate::models::proxy_kind(&crate::models::load_selection(&st.models_root)),
        Some(kind) => match dub_llm::net::ProxyKind::parse(kind) {
            Some(kind) => kind,
            None => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_proxy_kind", "detail": kind }))).into_response(),
        },
    };
    let address = crate::secrets_api::proxy_test_address(
        &st.models_root,
        crate::credentials::secrets_dir().as_deref(),
        body.get("url").and_then(Value::as_str).unwrap_or(""),
        body.get("password").and_then(Value::as_str),
    );
    let settings = dub_llm::net::ProxySettings { mode, kind, address: Some(address).filter(|a| !a.trim().is_empty()) };
    let res = tokio::task::spawn_blocking(move || dub_llm::net::test(settings).map_err(|e| format!("{e:#}")))
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(v) => {
            let hf = v["huggingface"].as_bool() == Some(true);
            let or = v["openrouter"].as_bool() == Some(true);
            Json(json!({
                "ok": hf && or,
                "hf": hf,
                "openrouter": or,
                "hf_error": v["huggingface_error"],
                "openrouter_error": v["openrouter_error"],
            }))
            .into_response()
        }
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

// ─── Пресеты железа: GET /engine/presets (список + детект GPU + рекомендация), POST /engine/preset {id} ──
pub async fn presets_list(State(_st): State<AppState>) -> Response {
    let rec = tokio::task::spawn_blocking(crate::presets::recommend)
        .await
        .unwrap_or_else(|_| crate::presets::recommend());
    let list: Vec<Value> = crate::presets::PRESETS
        .iter()
        .map(|p| json!({ "id": p.id, "title": p.title, "subtitle": p.subtitle }))
        .collect();
    Json(json!({ "presets": list, "hardware": rec })).into_response()
}

pub async fn preset_apply(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match crate::presets::apply(&st.models_root, &id) {
        Ok(applied) => Json(json!({
            "ok": true,
            "id": id,
            "applied": applied.iter().map(|(k, v)| json!({ "key": k, "value": v })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// GET /engine/openrouter/voices?model=<id> — голоса TTS-модели с полом/возрастом/русским из встроенного
// справочника (voice_db, собран роем). Для дропдауна выбора голоса + автокастинга. Нет в справочнике ->
// пустой список (юзер введёт голос вручную).
pub async fn openrouter_voices(Query(q): Query<HashMap<String, String>>) -> Response {
    let model = q.get("model").cloned().unwrap_or_default();
    let voices = crate::cloud_voices::list(&model).cloned().unwrap_or_default();
    let ru = crate::cloud_voices::model_supports_russian(&model);
    Json(json!({ "voices": voices, "supportsRussian": ru })).into_response()
}

// ─── PATCH /engine/opts ─────────────────────────────────────────────────────
// Свап слота модели (asr/tts/llm/vision) в рантайме. У порта OPTS иммутабелен внутри AppState
// (Arc<EngineOpts>), а модели резолвятся путями из окружения/дефолтов — рантайм-свап без пересоздания
// стейта не предусмотрен архитектурой (в отличие от питоновского живого OPTS). Валидируем вход как
// питон (непустые строки -> 400 иначе) и возвращаем ТЕКУЩИЙ стек — контракт формы {models:{...}}
// соблюдён; фактический свап слота — вне scope порта (модель-стек задаётся при старте сервера).
pub async fn set_opts(State(st): State<AppState>, Json(edit): Json<Value>) -> Response {
    for key in ["asr", "tts", "llm", "vision"] {
        if let Some(v) = edit.get(key) {
            let ok = v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false);
            if !ok {
                return (StatusCode::BAD_REQUEST, format!("{key:?} must be a non-empty path"))
                    .into_response();
            }
        }
    }
    let o = &st.opts;
    Json(json!({ "models": {
        "asr": o.asr_model,
        "llm": o.mt_model_path.to_string_lossy(),
        "vision": o.mmproj_path.to_string_lossy(),
        "tts": o.tts_model,
    }}))
    .into_response()
}

// ─── POST /engine/select ────────────────────────────────────────────────────
// Выбрать активный вариант модели (квант) для движка. Тело: {"id":"higgs-q6_k"} — id компонента из
// манифеста настроек. Пишем models/active.json; резолв при следующей генерации подхватит выбор без
// рестарта. Это ФАКТИЧЕСКИЙ свап-слот (в отличие от set_opts, который остался эхо-совместимостью).
pub async fn select_model(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    // Форма 1: {"key":"asr_engine","value":"whisper"} — прямая установка слота (движок/модель/квант ASR
    // и т.п.) из настроек, без скачивания. Ключ валидируем по белому списку.
    if let (Some(key), Some(val)) =
        (body.get("key").and_then(Value::as_str), body.get("value").and_then(Value::as_str))
    {
        if !crate::models::is_selection_key(key) {
            return (StatusCode::BAD_REQUEST, format!("unknown selection key: {key:?}")).into_response();
        }
        if val.trim().is_empty() {
            return (StatusCode::BAD_REQUEST, "empty value").into_response();
        }
        if !crate::models::is_selection_value(key, val) {
            return (StatusCode::BAD_REQUEST, format!("invalid value for {key}: {val:?}")).into_response();
        }
        if let Err(e) = crate::models::set_selection(&st.models_root, key, val) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("write selection: {e}")).into_response();
        }
        return Json(crate::models::public_selection(&st.models_root)).into_response();
    }
    // Форма 2: {"id":"whisper-small"} — id компонента манифеста -> набор слотов (активация при скачивании
    // и переключение варианта). Пустой набор -> неизвестный компонент.
    let id = body.get("id").and_then(Value::as_str).unwrap_or("");
    let pairs = crate::models::component_selection(id);
    if pairs.is_empty() {
        return (StatusCode::BAD_REQUEST, format!("unknown model component: {id:?}")).into_response();
    }
    for (engine, variant) in pairs {
        if let Err(e) = crate::models::set_selection(&st.models_root, engine, &variant) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("write selection: {e}")).into_response();
        }
    }
    Json(crate::models::public_selection(&st.models_root)).into_response()
}

// ─── PUT /projects/{pid} ────────────────────────────────────────────────────
// Полная замена Project (undo/redo снапшот). Порт app.py.put_project: валидировать тело как Project,
// сохранить атомарно, вернуть Project.
pub async fn put_project(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Json(body): Json<Value>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let mut proj: Project = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("bad project: {e}")).into_response(),
    };
    // The restored segment WAVs have their own audio revisions, but the mixed
    // master on disk still belongs to the state we are undoing.
    proj.audio.mix_dirty = true;
    if let Err(e) = crate::save_project_atomic(&dir, &proj) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    match proj.to_json() {
        Ok(s) => ([("content-type", "application/json")], s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /projects/{pid}/waveform?track=vocals|bgm|dub|master&n=600 ──────────
// Даунсэмпл-пики аудио (ffmpeg s16le 8kHz), кэш waveform_[track].json. CPU, вне GPU-воркера.
pub async fn waveform(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let track = q.get("track").map(|s| s.as_str()).unwrap_or("master");
    let cache_name = match track {
        "vocals" => "waveform_vocals.json",
        "bgm" | "instrumental" => "waveform_bgm.json",
        "dub" => "waveform_dub.json",
        _ => "waveform.json",
    };
    let proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let dur = proj.meta.duration.max(1.0);
    let dynamic_n = (dur * 20.0).round() as usize;
    let n: usize = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(dynamic_n.clamp(600, 200_000));
    let cache = dir.join(cache_name);

    let target_file: Option<PathBuf> = match track {
        "vocals" => {
            let p1 = dir.join("stems/vocals.wav");
            let p2 = dir.join("vocals16_clean.wav");
            let p3 = dir.join("vocals16.wav");
            if p1.is_file() { Some(p1) } else if p2.is_file() { Some(p2) } else if p3.is_file() { Some(p3) } else { None }
        }
        "bgm" | "instrumental" => {
            let p1 = dir.join("stems/instrumental.wav");
            let p2 = dir.join("bgm.wav");
            if p1.is_file() { Some(p1) } else if p2.is_file() { Some(p2) } else { None }
        }
        "dub" => {
            let p1 = dir.join("dub_vocals.wav");
            let p2 = dir.join("dub_vocals_raw.wav");
            let p3 = dir.join("dub_audio.m4a");
            if p1.is_file() { Some(p1) } else if p2.is_file() { Some(p2) } else if p3.is_file() { Some(p3) } else { None }
        }
        _ => Some(PathBuf::from(&proj.meta.video)),
    };

    if let Some(ref tf) = target_file {
        if let (Ok(c_meta), Ok(t_meta)) = (cache.metadata(), tf.metadata()) {
            if let (Ok(c_mtime), Ok(t_mtime)) = (c_meta.modified(), t_meta.modified()) {
                if c_mtime >= t_mtime {
                    if let Ok(txt) = std::fs::read_to_string(&cache) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                            if let Some(arr) = val.get("peaks").and_then(|v| v.as_array()) {
                                if arr.len() >= n.min(600) {
                                    return ([("content-type", "application/json")], txt).into_response();
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if let Ok(txt) = std::fs::read_to_string(&cache) {
        return ([("content-type", "application/json")], txt).into_response();
    }

    let peaks = if let Some(tf) = target_file {
        tokio::task::spawn_blocking(move || crate::wavio::waveform_peaks(&tf, n))
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };
    let empty = peaks.is_empty();
    let out = json!({ "peaks": peaks });
    if !empty {
        let _ = std::fs::write(&cache, out.to_string());
    }
    Json(out).into_response()
}

// ─── POST /projects/{pid}/remix?instruction= ────────────────────────────────
// Творческий ремикс: Gemma переписывает ВЕСЬ транскрипт по теме/инструкции, помечает всё dirty.
pub async fn remix_project(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let instruction = q.get("instruction").cloned().unwrap_or_default();
    if instruction.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "empty remix instruction").into_response();
    }
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let proj_path = dir.join("project.json");
    if !proj_path.is_file() {
        return (StatusCode::CONFLICT, "project not analyzed yet").into_response();
    }
    let llama_bin = st.llama_bin.clone();
    let mt_model = st.opts.mt_model_path.clone();
    let models_root = st.models_root.clone();
    let instr = instruction.trim().to_string();
    let dir_for_job = dir.clone();

    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        use dub_translate::{flat_rewrite, Seg};

        let text = std::fs::read_to_string(&proj_path).map_err(|e| e.to_string())?;
        let mut p = Project::from_json(&text).map_err(|e| e.to_string())?;
        if p.segments.is_empty() {
            return Err("no transcript to remix — analyze first".into());
        }
        let before = p.clone();
        let expected = crate::alignment::fingerprint(&before)?;
        progress(json!({ "type": "progress",
            "msg": format!("ремикс {} строк → {}", p.segments.len(),
                           &instr[..instr.len().min(60)]) }));

        let prov = crate::llm_provider::open(
            &crate::llm_provider::LlmOpen {
                llama_bin: &llama_bin,
                mt_model: &mt_model,
                mmproj: std::path::Path::new(""),
                models_root: &models_root,
            },
            crate::llm_provider::LlmMode::Text,
        )
        .map_err(|e| format!("ремикс: LLM недоступен — {e}"))?;
        let client = prov.client();

        let mut segs: Vec<Seg> = p
            .segments
            .iter()
            .map(|s| {
                let spk = crate::analyze::speaker_to_i64(s.speaker.as_deref());
                Seg::new(s.src_text.clone(), spk)
            })
            .collect();
        let r = flat_rewrite(client, &mut segs, &instr, "auto", &p.tgt_lang, false, &p.audio.translate_style);
        drop(prov);
        r.map_err(|e| format!("remix: {e}"))?;

        for (i, s) in p.segments.iter_mut().enumerate() {
            if let Some(sg) = segs.get(i) {
                if !sg.tgt.trim().is_empty() {
                    s.tgt_text = sg.tgt.clone();
                }
            }
            crate::segment_cache::invalidate_audio(s);
        }
        p.audio.rewrite = Some(instr.clone());
        p.audio.mix_dirty = true;
        let _guard = crate::PROJECT_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
        let current_text = std::fs::read_to_string(&proj_path).map_err(|e| e.to_string())?;
        let current = Project::from_json(&current_text).map_err(|e| e.to_string())?;
        if crate::alignment::fingerprint(&current)? != expected {
            return Err("REMIX_PROJECT_CHANGED: Проект изменён во время ремикса. Повторите запуск.".into());
        }
        crate::save_project_unlocked(&dir_for_job, &p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    });
    let job_id = st.jobs.enqueue_with_meta("remix", &pid, job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

// ─── GET /projects/{pid}/preview?t=&rev= ────────────────────────────────────
// ОДИН превью-кадр (JPEG) через быстрый spawn_blocking (не блокирует JobQueue).
pub async fn preview(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let input = match std::fs::read_to_string(dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source uploaded").into_response(),
    };
    let t: f64 = q.get("t").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let lowres = q.get("lr").map(|v| v == "1" || v == "true").unwrap_or(false);
    let fonts_dir = st.fonts_dir.clone();
    let work_dir = dir.clone();

    match tokio::task::spawn_blocking(move || {
        crate::frame::preview_frame(&proj, &input, &work_dir, &fonts_dir, t, lowres)
    })
    .await
    {
        Ok(Ok(bytes)) => ([("content-type", "image/jpeg")], bytes).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /projects/{pid}/original?t= ────────────────────────────────────────
// Сырой кадр ОРИГИНАЛА (PNG) на t — для before/after.
pub async fn original_frame(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let input = match std::fs::read_to_string(dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source uploaded").into_response(),
    };
    let t: f64 = q.get("t").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let work_dir = dir.clone();

    match tokio::task::spawn_blocking(move || crate::frame::source_frame(&input, &work_dir, t))
        .await
    {
        Ok(Ok(bytes)) => ([("content-type", "image/png")], bytes).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod provider_endpoint_tests {
    use super::*;
    use dub_llm::test_http::{serve, Reply};

    #[test]
    fn the_saved_server_key_goes_only_to_its_own_address() {
        let saved = serve(vec![Reply::json(200, r#"{"data":[{"id":"m"}]}"#)]);
        let foreign = serve(vec![Reply::json(200, r#"{"data":[{"id":"m"}]}"#)]);
        let dir = std::env::temp_dir().join(format!("dub-server-key-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        crate::credentials::store_local_server_key_in(&dir, &format!("{}/v1", saved.base()), Some("lm-secret")).unwrap();
        let key_for = |url: &str| crate::credentials::local_server_key_in(&dir, url);

        list_server_models(&foreign.base(), key_for(&foreign.base())).unwrap();
        let sent = foreign.request(0).to_ascii_lowercase();
        assert!(!sent.contains("authorization:") && !sent.contains("lm-secret"), "{sent}");

        list_server_models(&saved.base(), key_for(&saved.base())).unwrap();
        assert!(saved.request(0).to_ascii_lowercase().contains("authorization: bearer lm-secret"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_local_server_lists_its_models_in_any_address_form() {
        let server = serve(vec![
            Reply::json(200, r#"{"object":"list","data":[{"id":"gemma3:12b"},{"id":"qwen2.5vl:7b"}]}"#),
            Reply::json(200, r#"{"data":[]}"#),
            Reply::json(200, r#"{"models":[]}"#),
            Reply::json(401, r#"{"error":"no key"}"#),
        ]);
        assert_eq!(list_server_models(&format!("{}/v1/", server.base()), Some("k".into())).unwrap(), ["gemma3:12b", "qwen2.5vl:7b"]);
        let first = server.request(0);
        assert!(first.starts_with("GET /v1/models "), "{first}");
        assert!(first.to_ascii_lowercase().contains("authorization: bearer k"));
        assert!(list_server_models(&server.base(), None).unwrap().is_empty());
        assert!(!server.request(1).to_ascii_lowercase().contains("authorization:"));
        assert!(list_server_models(&server.base(), None).unwrap_err().contains("data"));
        assert!(list_server_models(&server.base(), None).unwrap_err().contains("401"));
        assert!(list_server_models("  ", None).is_err());
    }
}
