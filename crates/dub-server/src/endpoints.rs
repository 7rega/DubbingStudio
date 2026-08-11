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

use crate::{jobs, save_project_atomic, AppState};

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
// Динамический каталог OpenRouter через сайдкар (Go SDK, операция models с фильтром модальности):
// llm = output text; vision = input image; tts = output speech; asr = output transcription.
pub async fn openrouter_models(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let kind = q.get("kind").cloned().unwrap_or_else(|| "llm".to_string());
    let Some(key) = crate::models::openrouter_key(&st.models_root) else {
        return (StatusCode::BAD_REQUEST, "ключ OpenRouter не задан").into_response();
    };
    let repo = st.repo_root.clone();
    let payload = match kind.as_str() {
        "vision" => json!({ "input_modalities": "image" }),
        "tts" => json!({ "output_modalities": "speech" }),
        "asr" => json!({ "output_modalities": "transcription" }),
        _ => json!({ "output_modalities": "text" }),
    };
    let res = tokio::task::spawn_blocking(move || {
        crate::openrouter_cli::run_json(&repo, &key, "models", &payload)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        // helper отдаёт ModelsListResponse {data:[...]}; прокидываем data как список.
        Ok(v) => {
            let data = v.get("data").cloned().unwrap_or(v);
            Json(json!({ "models": data })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("каталог OpenRouter: {e}")).into_response(),
    }
}

// ─── POST /engine/openrouter/verify {key} ───────────────────────────────────
// Проверка ключа OpenRouter через сайдкар (Go SDK, операция verify -> credits). Ключ из формы (ввод
// перед сохранением), НЕ логируется.
pub async fn openrouter_verify(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if key.is_empty() {
        return (StatusCode::BAD_REQUEST, "пустой ключ").into_response();
    }
    let repo = st.repo_root.clone();
    let res = tokio::task::spawn_blocking(move || {
        crate::openrouter_cli::run_json(&repo, &key, "verify", &json!({}))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

// ─── POST /engine/proxy/test {url} — проверить связность через прокси ────────────────────────────────
// Пробуем достучаться до HF (закачка моделей) и OpenRouter (облако) через указанный прокси. Пустой url ->
// проверка ПРЯМОГО доступа (без прокси): юзер сразу видит, нужен ли ему прокси вообще. http_status_as_error
// выключаем — меряем транспорт (дошли до сервера через прокси), а не HTTP-статус ответа.
pub async fn proxy_test(Json(body): Json<Value>) -> Response {
    let url = body.get("url").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let res = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let agent: ureq::Agent = if url.is_empty() {
            ureq::Agent::config_builder().http_status_as_error(false).build().into()
        } else {
            let proxy = ureq::Proxy::new(&url).map_err(|e| format!("некорректный URL прокси: {e}"))?;
            ureq::Agent::config_builder()
                .proxy(Some(proxy))
                .http_status_as_error(false)
                .build()
                .into()
        };
        let probe = |target: &str| -> Option<String> {
            agent.get(target).call().err().map(|e| e.to_string())
        };
        // Лёгкие публичные JSON-эндпоинты обоих сервисов (без ключа): проверяем именно то, что проксируем.
        let hf = probe("https://huggingface.co/api/models/immich-app/buffalo_l");
        let or = probe("https://openrouter.ai/api/v1/models");
        Ok(json!({
            "ok": hf.is_none() && or.is_none(),
            "hf": hf.is_none(),
            "openrouter": or.is_none(),
            "hf_error": hf,
            "openrouter_error": or,
        }))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(v) => Json(v).into_response(),
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
        if val.trim().is_empty()
            && key != "whisper_xxl_args"
            && !key.starts_with("or_")
            && key != "proxy_url"
        {
            return (StatusCode::BAD_REQUEST, "empty value").into_response();
        }
        if let Err(e) = crate::models::set_selection(&st.models_root, key, val) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("write selection: {e}")).into_response();
        }
        return Json(crate::models::load_selection(&st.models_root)).into_response();
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
    Json(crate::models::load_selection(&st.models_root)).into_response()
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
    let proj: Project = match serde_json::from_value(body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("bad project: {e}")).into_response(),
    };
    if let Err(e) = save_project_atomic(&dir, &proj) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    match proj.to_json() {
        Ok(s) => ([("content-type", "application/json")], s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /projects/{pid}/waveform?n=600 ─────────────────────────────────────
// Даунсэмпл-пики аудио (ffmpeg s16le 8kHz), кэш waveform.json. CPU, вне GPU-воркера. Порт app.py.
pub async fn waveform(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let cache = dir.join("waveform.json");
    if let Ok(txt) = std::fs::read_to_string(&cache) {
        return ([("content-type", "application/json")], txt).into_response();
    }
    let proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let n: usize = q.get("n").and_then(|v| v.parse().ok()).unwrap_or(600);
    let video = PathBuf::from(&proj.meta.video);
    let peaks =
        tokio::task::spawn_blocking(move || crate::wavio::waveform_peaks(&video, n))
            .await
            .unwrap_or_default();
    let empty = peaks.is_empty();
    let out = json!({ "peaks": peaks });
    if !empty {
        let _ = std::fs::write(&cache, out.to_string());
    }
    Json(out).into_response()
}

// ─── POST /projects/{pid}/remix?instruction= ────────────────────────────────
// Творческий ремикс: Gemma переписывает ВЕСЬ транскрипт по теме/инструкции, помечает всё dirty.
// Порт app.py.remix_project (использует translate.rewrite = flat_rewrite dub-translate).
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
        progress(json!({ "type": "progress",
            "msg": format!("ремикс {} строк → {}", p.segments.len(),
                           &instr[..instr.len().min(60)]) }));

        // LLM-провайдер: облако OpenRouter (если включено) ИЛИ локальный llama-server (плоский rewrite).
        // Text-режим: mmproj не нужен (передаём пустой путь — фабрика его в Text-режиме игнорирует).
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
            s.dirty = true;
        }
        p.audio.rewrite = Some(instr.clone());
        save_project_atomic(&dir_for_job, &p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

// ─── GET /projects/{pid}/preview?t=&rev= ────────────────────────────────────
// ОДИН превью-кадр (PNG) через GPU-воркер (сериализовано). Порт app.py.preview (таймаут 300с).
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
        Err(resp) => return resp, // 409 если не проанализирован
    };
    let input = match std::fs::read_to_string(dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source uploaded").into_response(),
    };
    let t: f64 = q.get("t").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    // lr=1 (плей) -> низкое разрешение кадра на больших видео (решает preview_frame). JPEG-кадр.
    let lowres = q.get("lr").map(|v| v == "1" || v == "true").unwrap_or(false);
    let fonts_dir = st.fonts_dir.clone();
    let work_dir = dir.clone();
    frame_job(&st, 300, "image/jpeg", move |_p| {
        crate::frame::preview_frame(&proj, &input, &work_dir, &fonts_dir, t, lowres)
    })
    .await
}

// ─── GET /projects/{pid}/original?t= ────────────────────────────────────────
// Сырой кадр ОРИГИНАЛА (PNG) на t — для before/after. Порт app.py.original (source_frame, таймаут 60с).
// ВНИМАНИЕ: возвращает ОДИН PNG-кадр (как источник истины app.py), а не всё видео — фронт (ComparePane)
// использует это как <img src>.
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
        Err(_) => return (StatusCode::NOT_FOUND, "no source").into_response(),
    };
    if !input.is_file() {
        return (StatusCode::NOT_FOUND, "source missing").into_response();
    }
    let t: f64 = q.get("t").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let work_dir = dir.clone();
    frame_job(&st, 60, "image/png", move |_p| crate::frame::source_frame(&input, &work_dir, t)).await
}

/// Общий помощник: поставить синхронную кадр-джобу в GPU-воркер, ждать с таймаутом, вернуть
/// байты с заданным content-type (`mime`) или 504. Порт паттерна app.py.preview/original.
async fn frame_job<F>(st: &AppState, timeout_s: u64, mime: &'static str, f: F) -> Response
where
    F: FnOnce(jobs::ProgressFn) -> Result<Vec<u8>, String> + Send + 'static,
{
    let job: jobs::JobFn = Box::new(move |progress| {
        let png = f(progress)?;
        // Возвращаем байты как base64 в JSON-результат нельзя эффективно; вместо этого прокидываем через
        // канал: сериализуем как массив байт в Value (voркер отдаёт oneshot Result<Value>). Читаем ниже.
        Ok(Value::Array(png.into_iter().map(|b| Value::from(b as u64)).collect()))
    });
    let (job_id, rx) = st.jobs.enqueue_awaitable(job).await;
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_s), rx).await {
        Ok(Ok(Ok(v))) => {
            st.jobs.remove(&job_id).await;
            let bytes: Vec<u8> = v
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect())
                .unwrap_or_default();
            ([("content-type", mime)], bytes).into_response()
        }
        Ok(Ok(Err(e))) => {
            st.jobs.remove(&job_id).await;
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
        Ok(Err(_)) => {
            st.jobs.remove(&job_id).await;
            (StatusCode::INTERNAL_SERVER_ERROR, "job canceled").into_response()
        }
        Err(_) => {
            st.jobs.mark_abandoned(&job_id).await;
            (StatusCode::GATEWAY_TIMEOUT, "frame render timed out").into_response()
        }
    }
}
