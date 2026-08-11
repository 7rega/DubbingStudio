//! dub-server — axum-реализация REST/SSE контракта Dub Studio. Цель: SPA (frontend/dist) работает
//! без правок против этого сервера так же, как против backend/app.py.
//!
//! Раунд 1 реализует: раздачу SPA с защитой от traversal, GET /engine/capabilities, POST /projects
//! (multipart-загрузка видео -> workspace/<pid>/), GET /projects/{id}, каркас очереди джоб + SSE
//! GET /jobs/{id}/events. GPU-эндпоинты (analyze/render/preview/patch и т.д.) — каркас на следующие
//! раунды; их карта в docs/PORT-CONTRACT.md.

mod analyze;
mod bench;
mod casting;
mod casting_library;
mod cloud_asr;
mod cloud_tts;
mod cloud_voices;
mod compose;
mod endpoints;
mod llm_provider;
mod openrouter_cli;
mod f0;
mod frame;
mod hw;
mod presets;
mod jobs;
mod media;
mod models;
mod ocr;
mod patch;
mod record;
mod render;
mod setup;
mod spa;
mod subimport;
mod translate;
mod voice_slots;
mod wavio;

use axum::extract::{Multipart, Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use dub_core::{EngineOpts, Project};
use futures_util::stream::Stream;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use jobs::JobQueue;

/// E2E-верификация caption-композита БЕЗ ASR/Gemma/TTS: берём УЖЕ проанализированный project.json
/// (кэш transcript+raw_ctx), заново гоним OCR-стадию + compose (реальная детекция + матчинг титров),
/// затем вжигаем блюр+сабы в captioned.mp4. Проверка на живых данных, не
/// перезапуская тяжёлый Gemma-vision. Возвращает путь к captioned.mp4.
pub fn verify_captions_e2e(
    repo_root: &Path,
    project_json: &Path,
    input_video: &Path,
    work_dir: &Path,
) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(project_json).map_err(|e| e.to_string())?;
    let mut proj = Project::from_json(&text).map_err(|e| e.to_string())?;

    // OCR-стадия перезаписывает captions.blur_boxes/titles/sub_style/sub_px через compose. Чистим
    // предыдущий вывод, чтобы не смешивать со старым blur из project.json.
    proj.captions.blur_boxes.clear();
    proj.captions.titles.clear();

    let vw = proj.meta.width;
    let vh = proj.meta.height;
    let total = proj.meta.duration;
    let src_codec = proj.meta.src_codec.clone();

    // Пути резолвим напрямую из repo_root (без AppState — тот тянет Tokio-JobQueue). ocr::stage нужны
    // лишь models_root/caption_fps/input/work_dir; llama_bin/mt_model — только для таглайн-MT (fail-safe).
    let opts = EngineOpts::default();
    let mroot = models_root(repo_root);
    let fonts_dir = repo_root.join("fonts");
    let unused = repo_root.join("nonexistent");
    let args = analyze::AnalyzeArgs {
        tgt_lang: proj.tgt_lang.clone(),
        mode: proj.mode.clone(),
        src_lang: "auto".into(),
        subs: proj.subs.mode.clone(),
        rewrite: String::new(),
        translate_style: proj.audio.translate_style.clone(),
        burn: proj.subs.burn,
        detect_text: true,
        casting: false, // verify-путь: только капшены/OCR, кастинг не нужен
        casting_ref: String::new(),
        content_type: String::new(),
        import_translated: false,
    };
    let sel = models::load_selection(&mroot);
    let (mt_model, mmproj) = models::resolve_mt(&mroot, &sel);
    let paths = analyze::AnalyzePaths {
        input: input_video.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        repo_root: repo_root.to_path_buf(),
        asr: models::AsrChoice::Parakeet(unused.clone()), // ocr::stage не трогает ASR (verify-путь)
        sortformer_onnx: unused.clone(),
        llama_bin: dub_llm::resolve_llama_bin(&repo_root.join("tools").join("llama")),
        mt_model,
        mmproj,
        models_root: mroot,
        caption_fps: opts.caption_fps,
        import_subs: None,
        // ocr::stage сепарацию не использует — заглушки (verify-путь без BSRoformer).
        bsroformer_cli: unused.clone(),
        bsroformer_model: unused.clone(),
    };
    let cb = |ev: Value| {
        if let Some(m) = ev.get("msg").and_then(|v| v.as_str()) {
            eprintln!("[verify] {m}");
        }
    };
    ocr::stage(&args, &paths, &mut proj, vw, vh, total, &cb);

    let out_ass = work_dir.join("verify_caps.ass");
    let captioned = work_dir.join("verify_captioned.mp4");
    render::build_and_burn_captions(
        &proj, input_video, &out_ass, &captioned, &fonts_dir, vw, vh, total, &src_codec,
    )?;
    // Записать обновлённый project (с bbox титров) рядом — для инспекции.
    let dbg = work_dir.join("verify_project.json");
    let _ = std::fs::write(&dbg, proj.to_json_pretty().unwrap_or_default());
    Ok(captioned)
}

#[derive(Clone)]
pub struct AppState {
    pub repo_root: PathBuf,
    pub workspace: PathBuf,
    pub web_root: Option<PathBuf>,
    pub opts: Arc<EngineOpts>,
    pub jobs: JobQueue,
    /// Каталог TDT-модели ASR (analyze). Env DUB_STUDIO_TDT, иначе <models_root>/tdt.
    pub tdt_dir: PathBuf,
    /// Путь к Sortformer .onnx (диаризация). Env DUB_STUDIO_SORTFORMER, иначе <models_root>/sortformer/…v2.onnx.
    pub sortformer_onnx: PathBuf,
    /// llama-server(.exe) — сайдкар перевода/vision. Env DUB_STUDIO_LLAMA_BIN, иначе <repo>/tools/llama/llama-server(.exe).
    pub llama_bin: PathBuf,
    /// BSRoformer CLI (сепарация). Env DUB_STUDIO_BSROFORMER_DIR/<cli>, иначе <repo>/tools/bsroformer/bs_roformer-cli.exe.
    pub bsroformer_cli: PathBuf,
    /// GGUF модель сепарации. Env DUB_STUDIO_BSROFORMER_MODEL, иначе <repo>/models/bsroformer/voc_fv6-Q8_0.gguf.
    pub bsroformer_model: PathBuf,
    /// Higgs audiocpp_engine.dll (TTS). Env DUB_STUDIO_HIGGS_DLL, иначе <models>/higgs-engine/audiocpp_engine.dll.
    pub higgs_dll: PathBuf,
    /// Каталог весов Higgs (q8_0). Env DUB_STUDIO_HIGGS_MODEL, иначе <models>/higgs-q8_0.
    pub higgs_model_root: PathBuf,
    /// Каталог bundled-шрифтов субтитров. Env DUB_STUDIO_FONTS_DIR, иначе <repo>/fonts.
    pub fonts_dir: PathBuf,
    /// Корень моделей (для OCR: <root>/ocr/…). Env DUBENGINE_MODELS_ROOT, иначе <repo>/models.
    pub models_root: PathBuf,
    /// Каталог голосов-паков + записей с микрофона. Env DUBENGINE_VOICES, иначе <repo>/voices.
    pub voices_dir: PathBuf,
    /// Флаг отмены текущей setup-закачки (POST /setup/cancel взводит; download-loop его читает).
    pub setup_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub tts_cache: Arc<Mutex<Option<(render::EngineKey, Arc<audiocpp::AudiocppEngine>)>>>,
}

/// Корень моделей: env DUBENGINE_MODELS_ROOT, иначе <repo_root>/models.
pub(crate) fn models_root(repo_root: &Path) -> PathBuf {
    std::env::var("DUBENGINE_MODELS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root.join("models"))
}

/// Прописать в PATH процесса каталоги скачанных бинарей (ffmpeg, llama, движок Higgs с CUDA/VC-DLL),
/// чтобы `Command::new("ffmpeg")` и подобные находили их без перезапуска после автозакачки. Вызывается
/// один раз при старте сервера (main.rs / Tauri-shell). Идемпотентно: не дублирует уже присутствующие.
pub fn augment_path_for_tools(repo_root: &Path) {
    let dirs = [
        repo_root.join("tools").join("ffmpeg"),
        repo_root.join("tools").join("llama"),
        repo_root.join("models").join("higgs-engine"),
    ];
    let sep = if cfg!(windows) { ";" } else { ":" };
    let cur = std::env::var("PATH").unwrap_or_default();
    let mut prefix = String::new();
    for d in dirs {
        let s = d.to_string_lossy().to_string();
        if !s.is_empty() && !cur.split(sep).any(|p| p == s) && !prefix.split(sep).any(|p| p == s) {
            if !prefix.is_empty() {
                prefix.push_str(sep);
            }
            prefix.push_str(&s);
        }
    }
    if !prefix.is_empty() {
        std::env::set_var("PATH", format!("{prefix}{sep}{cur}"));
    }
}

/// Прописать прокси из models/active.json в env процесса (если включён), чтобы стандартные HTTP-клиенты
/// (ureq default-agent, reqwest в record.rs, Tauri-апдейтер в десктоп-процессе) шли через него. Вызывать
/// на старте — ДО первого HTTP-запроса. Закачки моделей и облако строят клиент явно и подхватывают смену
/// прокси без рестарта; остальному (апдейтер/метаданные HF) смена прокси требует рестарта.
pub fn apply_proxy_env(repo_root: &Path) {
    models::apply_proxy_env(&repo_root.join("models"));
}

/// Поднять axum-сервер БЛОКИРУЮЩЕ на собственном tokio-рантайме. Для встраивания в десктоп-оболочку ОДНИМ
/// процессом (вместо запуска dub-server.exe отдельным subprocess) — вызывать из фонового std::thread.
/// Слушает 127.0.0.1:port; augment PATH под инструменты делается здесь же.
pub fn serve_blocking(repo_root: impl AsRef<Path>, port: u16) -> anyhow::Result<()> {
    let root = repo_root.as_ref().to_path_buf();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        augment_path_for_tools(&root);
        apply_proxy_env(&root);
        let state = AppState::new(&root);
        let app = build_router(state);
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok::<(), anyhow::Error>(())
    })
}

impl AppState {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        let repo_root = repo_root.as_ref().to_path_buf();
        let workspace = repo_root.join("workspace");
        let _ = std::fs::create_dir_all(&workspace);
        let web_root = spa::find_web_root(&repo_root);
        let mroot = models_root(&repo_root);
        let tdt_dir = std::env::var("DUB_STUDIO_TDT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| mroot.join("tdt"));
        let sortformer_onnx = std::env::var("DUB_STUDIO_SORTFORMER")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                mroot
                    .join("sortformer")
                    .join("diar_streaming_sortformer_4spk-v2.onnx")
            });
        // llama-server: env-override, иначе <repo>/tools/llama/llama-server(.exe) (негитуемый каталог,
        // кладёт установщик/раунд 3). Существование проверяет сама стадия перевода (fail-safe).
        let llama_bin = dub_llm::resolve_llama_bin(&repo_root.join("tools").join("llama"));
        let bsroformer_cli = std::env::var("DUB_STUDIO_BSROFORMER_DIR")
            .map(|d| PathBuf::from(d).join(dub_sep::ENGINE_CLI_FILE))
            .unwrap_or_else(|_| dub_sep::engine_dir(&repo_root).join(dub_sep::ENGINE_CLI_FILE));
        let bsroformer_model = dub_sep::model_path(&repo_root);
        let higgs_dll = std::env::var("DUB_STUDIO_HIGGS_DLL")
            .map(PathBuf::from)
            .unwrap_or_else(|_| mroot.join("higgs-engine").join("audiocpp_engine.dll"));
        let higgs_model_root = std::env::var("DUB_STUDIO_HIGGS_MODEL")
            .map(PathBuf::from)
            .unwrap_or_else(|_| mroot.join("higgs-q8_0"));
        let fonts_dir = std::env::var("DUB_STUDIO_FONTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root.join("fonts"));
        let voices_dir = std::env::var("DUBENGINE_VOICES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root.join("voices"));
        AppState {
            repo_root,
            workspace,
            web_root,
            opts: Arc::new(EngineOpts::default()),
            jobs: JobQueue::new(),
            tdt_dir,
            sortformer_onnx,
            llama_bin,
            bsroformer_cli,
            bsroformer_model,
            higgs_dll,
            higgs_model_root,
            fonts_dir,
            models_root: mroot,
            voices_dir,
            setup_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tts_cache: Arc::new(Mutex::new(None)),
        }
    }

    fn proj_dir(&self, pid: &str) -> Result<PathBuf, Response> {
        // pid — hex uuid; отсекаем любые сепараторы/`..` до касания ФС (защита от traversal в pid).
        if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err((StatusCode::NOT_FOUND, "project not found").into_response());
        }
        let d = self.workspace.join(pid);
        if !d.exists() {
            return Err((StatusCode::NOT_FOUND, "project not found").into_response());
        }
        Ok(d)
    }

    fn load_project(&self, pid: &str) -> Result<Project, Response> {
        let d = self.proj_dir(pid)?;
        let f = d.join("project.json");
        if !f.is_file() {
            return Err((StatusCode::CONFLICT, "project not analyzed yet").into_response());
        }
        let text = std::fs::read_to_string(&f)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
        Project::from_json(&text)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
    }
}

/// Атомарная запись project.json: сериализуем в tmp рядом, затем rename (tmp+rename — как в
/// проверенных питон-паттернах; частичного файла при падении не будет).
fn save_project_atomic(dir: &Path, proj: &Project) -> Result<(), String> {
    let json = proj
        .to_json_pretty()
        .map_err(|e| format!("сериализация project.json: {e}"))?;
    let tmp = dir.join("project.json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| format!("запись tmp: {e}"))?;
    std::fs::rename(&tmp, dir.join("project.json")).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    use tower_http::cors::{Any, CorsLayer};
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/engine/capabilities", get(capabilities))
        .route("/engine/opts", axum::routing::patch(endpoints::set_opts))
        .route("/engine/select", post(endpoints::select_model))
        .route("/engine/openrouter/models", get(endpoints::openrouter_models))
        .route("/engine/openrouter/voices", get(endpoints::openrouter_voices))
        .route("/engine/openrouter/verify", post(endpoints::openrouter_verify))
        .route("/engine/proxy/test", post(endpoints::proxy_test))
        .route("/engine/presets", get(endpoints::presets_list))
        .route("/engine/preset", post(endpoints::preset_apply))
        // «Первый запуск»: статус компонентов + автозакачка недостающего (SSE через ту же job-машину).
        .route("/setup/status", get(setup_status))
        .route("/setup/download", post(setup_download))
        .route("/setup/cancel", post(setup_cancel))
        .route("/setup/browse", post(setup_browse))
        .route("/pick-folder", post(pick_folder))
        .route("/setup/import", post(setup_import))
        .route("/hw/snapshot", get(hw_snapshot))
        .route("/record/devices", get(record_devices))
        .route("/record/level", get(record_level))
        .route("/record/start", post(record_start))
        .route("/record/stop", post(record_stop))
        .route("/voices/download-pack", post(voices_download_pack))
        .route("/voices/catalog", get(voices_catalog))
        .route("/voices/sample", get(voice_sample))
        .route("/voices/get", post(voices_get))
        .route("/voices/rename", post(voices_rename))
        .route("/voices/delete", post(voices_delete))
        .route("/projects/{pid}/speaker-voice", post(speaker_voice))
        .route("/projects/{pid}/voice-slots", post(voice_slots_assign))
        .route("/projects/{pid}/casting", get(casting_get).post(casting_save))
        .route("/projects/{pid}/casting/avatar", get(casting_avatar))
        .route("/projects/{pid}/casting/voice", get(casting_voice))
        .route("/casting/library", get(casting_library_list))
        .route("/projects/{pid}/casting/library", post(casting_library_save))
        .route("/casting/library/{slug}", delete(casting_library_delete))
        .route("/casting/library/{slug}/avatar", get(casting_library_avatar))
        .route("/fonts", get(endpoints::fonts))
        .route("/voices", get(voices_list))
        .route("/presets", get(endpoints::presets))
        .route("/projects", post(create_project).get(list_projects))
        .route(
            "/projects/{pid}",
            get(get_project).patch(patch_project).put(endpoints::put_project).delete(delete_project),
        )
        .route("/projects/{pid}/analyze", post(analyze_project))
        .route("/projects/{pid}/align", post(align_project))
        .route("/projects/{pid}/remix", post(endpoints::remix_project))
        .route("/projects/{pid}/render", post(render_project))
        .route("/projects/{pid}/export-lang", post(export_lang))   // клон+ре-перевод+рендер на другом языке (экспорт-уровень мультиязыка)
        .route("/projects/{pid}/retranslate", post(retranslate_project))   // #122: смена режима из транскрипта — перевод готовых сегментов БЕЗ ASR
        .route("/projects/{pid}/waveform", get(endpoints::waveform))
        .route("/projects/{pid}/preview", get(endpoints::preview))
        .route("/projects/{pid}/output", get(output))
        .route("/projects/{pid}/open", post(open_output))
        .route("/projects/{pid}/open-folder", post(open_project_folder))
        .route("/projects/{pid}/trash", post(trash_project))
        .route("/projects/{pid}/reveal", post(reveal_file))
        .route("/projects/{pid}/save-text", post(save_text))
        .route("/projects/{pid}/save-output", post(save_output))
        .route("/projects/{pid}/dub-audio", post(dub_audio_project))
        // /original?t= отдаёт ОДИН PNG-кадр оригинала (порт app.py.original -> source_frame),
        // фронт (ComparePane) вставляет его как <img src>. Range-раздача сырого видео — /dub.
        .route("/projects/{pid}/original", get(endpoints::original_frame))
        .route("/projects/{pid}/dub", get(dub_video))
        .route("/jobs/{job_id}/events", get(job_events))
        // SPA fallback — монтируется последним, чтобы не затенять API.
        .fallback(spa_fallback)
        // Видео-аплоад — большие тела. axum по дефолту режет на 2МБ (multipart ломается на
        // реальном ролике). Питон (Starlette) лимита не ставит -> снимаем и мы.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(cors)
        .with_state(state)
}

// ─── /engine/capabilities ───────────────────────────────────────────────────

async fn capabilities(State(st): State<AppState>) -> Json<Value> {
    let o = &st.opts;
    let ffmpeg = which_ffmpeg();
    // Текущий выбор вариантов (active.json) — фронт рисует по нему селекторы движка/модели/кванта ASR.
    let sel = models::load_selection(&st.models_root);
    // Языки контента = полный набор Whisper (99). Единый источник — dub_translate::WHISPER_LANGS.
    let langs: Vec<&str> = dub_translate::WHISPER_LANGS.iter().map(|(c, _)| *c).collect();
    // Тот же JSON-контракт, что в app.py.capabilities(), плюс поля выбора ASR-движка.
    Json(json!({
        "device": o.device,
        "tts_quant": o.tts_quant,
        "asr_model": o.asr_model,
        "models": {
            "asr": o.asr_model,
            "llm": o.mt_model_path.to_string_lossy(),
            "vision": o.mmproj_path.to_string_lossy(),
            "tts": o.tts_model,
        },
        "ffmpeg": ffmpeg,
        "languages": langs,
        "voice_modes": ["clone","autocast","auto","voice"],
        // Выбор ASR: движок (parakeet|whisper), модель Whisper, квант Whisper (compute_type).
        "selection": sel,
        "asr_engines": ["parakeet","whisper"],
        "whisper_models": ["tiny","base","small","medium","large-v3","large-v3-turbo"],
        // Кванты Whisper (compute_type): float16 / int8_float16 задействуют Tensor Cores на CUDA GPU.
        "whisper_computes": ["int8","int8_float16","float16","int8_float32","float32"],
        // Видимые лимиты RAM (настройки, не авто-магия): против OOM на слабой памяти. "0" = авто (дефолт).
        "llama_ubatches": ["0","512","256","128"],      // prefill-батч Gemma (меньше = меньше RAM)
        "higgs_ref_secs_opts": ["12","8","6","4"],       // длина реф-клипа клона (сек; <12 спасает 32ГБ)
    }))
}

fn which_ffmpeg() -> bool {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
        })
        .unwrap_or(false)
}

// ─── /setup/status ; /setup/download ; /setup/cancel ────────────────────────

/// GET /setup/status — статус каждого компонента (installed/missing/размер) + драйвер/готовность.
/// Читает только диск (никаких моделей не грузит) — безопасно вызывать до любой закачки.
async fn setup_status(State(st): State<AppState>) -> Json<Value> {
    let root = st.repo_root.clone();
    // ФС-обход в блокирующий пул (десятки stat-ов), чтобы не держать реактор.
    let status = tokio::task::spawn_blocking(move || setup::setup_status(&root))
        .await
        .unwrap_or_else(|_| setup::setup_status(&st.repo_root));
    Json(serde_json::to_value(status).unwrap_or_else(|_| json!({})))
}

/// POST /setup/download {ids:[...]} — поставить джобу закачки выбранных компонентов; вернуть job_id.
/// Прогресс — по SSE GET /jobs/{id}/events (та же машина, что analyze/render).
async fn setup_download(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    let ids: Vec<String> = body
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return (StatusCode::BAD_REQUEST, "ids пуст").into_response();
    }
    let root = st.repo_root.clone();
    let cancel_flag = st.setup_cancel.clone();
    cancel_flag.store(false, std::sync::atomic::Ordering::SeqCst); // свежий старт

    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        let cancel = {
            let cf = cancel_flag.clone();
            move || cf.load(std::sync::atomic::Ordering::SeqCst)
        };
        let cb = |ev: Value| progress(ev);
        setup::download_components(&root, &ids, &cancel, &cb)
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

/// POST /setup/cancel — взвести флаг отмены текущей закачки (download-loop прервётся на следующем чанке).
async fn setup_cancel(State(st): State<AppState>) -> Json<Value> {
    st.setup_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    Json(json!({ "cancelled": true }))
}

/// ON-DEMAND: перед джобой догружаем компоненты, нужные ИМЕННО ДЛЯ ЭТОЙ функции/конфига, если их нет
/// на диске. Юзер запросил кастинг, но модели лиц не скачаны -> тянем их тут же (прогресс в тот же SSE)
/// и продолжаем — вместо «0 лиц/все SPK0». Аналогично GPU-стадия без CUDA-стека -> догружаем cuFFT/onnx-GPU/
/// cuDNN. Пустой список = ничего не качаем, идём дальше мгновенно.
fn ensure_job_components(
    repo_root: &Path,
    models_root: &Path,
    casting: bool,
    progress: &setup::ProgressCb,
) -> Result<(), String> {
    let manifest = setup::manifest();
    let missing = |id: &str| -> bool {
        manifest
            .iter()
            .find(|c| c.id == id)
            .map(|c| !setup::component_status(repo_root, c).installed)
            .unwrap_or(false)
    };
    let mut need: Vec<String> = Vec::new();
    // Кастинг персонажей (#115) → его модели (детект лиц + эмбеддинг лица/голоса + аниме/окклюдер).
    if casting && missing("casting") {
        need.push("casting".to_string());
    }
    // Любая локальная стадия на GPU → полный CUDA-стек: диаризация/ASR грузят onnxruntime-GPU + cuDNN,
    // ggml-движки (сепарация/Higgs) — cuda-runtime (cudart/cublas/cuFFT). Без cuFFT CUDA-EP не поднимется.
    let gpu = ["sep_backend", "diar_backend", "asr_backend"]
        .iter()
        .any(|k| models::stage_backend(models_root, k) == "gpu");
    if gpu {
        for id in ["cuda-runtime", "onnxruntime-gpu", "cudnn", "bsroformer-engine"] {
            if missing(id) {
                need.push(id.to_string());
            }
        }
    }
    // Whisper на GPU: CTranslate2 требует cuBLAS12 + cuDNN8 РЯДОМ с whisper-faster.exe (наш ONNX-стек
    // выше — CUDA 13, whisper'у не подходит). Без них whisper молча идёт на CPU. Тянем их только когда
    // движок ASR = Whisper И стадия ASR на GPU.
    let sel = models::load_selection(models_root);
    let asr_whisper = sel
        .get("asr_engine")
        .and_then(|v| v.as_str())
        == Some("whisper");
    let bin = models::whisper_bin(repo_root, Some(&sel));
    let is_xxl = bin
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().contains("xxl"));
    if asr_whisper
        && models::stage_backend(models_root, "asr_backend") == "gpu"
        && !is_xxl
        && missing("whisper-cuda")
    {
        need.push("whisper-cuda".to_string());
    }
    if need.is_empty() {
        return Ok(());
    }
    progress(json!({ "stage": "download", "msg": "Догружаю недостающие модели для этой функции…" }));
    let cancel = || false; // догрузка внутри джобы не отменяется отдельно
    setup::download_components(repo_root, &need, &cancel, progress).map(|_| ())
}

/// GET /hw/snapshot — снимок GPU/VRAM/темп/мощность + RAM для монитора ресурсов.
async fn hw_snapshot() -> Json<hw::HardwareSnapshot> {
    Json(tokio::task::spawn_blocking(hw::snapshot).await.unwrap_or_default())
}

/// Безопасное имя голоса: только буквы/цифры/пробел/дефис/подчёркивание (без разделителей путей).
fn sanitize_voice_name(s: &str) -> String {
    let n: String = s
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .take(48)
        .collect();
    let n = n.trim().to_string();
    if n.is_empty() { "Мой голос".to_string() } else { n }
}

/// Имена голосов в каталоге: стемы .wav/.mp3 (запись с микрофона = .wav; пак = .mp3).
fn list_voice_names(dir: &Path) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            if ext == "wav" || ext == "mp3" {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.insert(stem.to_string());
                }
            }
        }
    }
    names.into_iter().collect()
}

/// GET /voices — список голосов из каталога (пак + записи с микрофона).
async fn voices_list(State(st): State<AppState>) -> Json<Value> {
    Json(json!({ "voices": list_voice_names(&st.voices_dir) }))
}

/// GET /voices/sample?name=<name> — отдать аудио-сэмпл голоса (voices/<name>.wav|.mp3) с Range для <audio>
/// (прослушка выбранного голоса в UI). sanitize защищает от path-traversal.
async fn voice_sample(
    State(st): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let name = sanitize_voice_name(q.get("name").map(|s| s.as_str()).unwrap_or(""));
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "no name").into_response();
    }
    for ext in ["wav", "mp3"] {
        let p = st.voices_dir.join(format!("{name}.{ext}"));
        if p.is_file() {
            return serve_file_range(&p, req, None).await;
        }
    }
    (StatusCode::NOT_FOUND, "voice not found").into_response()
}

/// GET /record/devices — список микрофонов.
async fn record_devices() -> Json<Value> {
    Json(json!({ "devices": tokio::task::spawn_blocking(record::input_devices).await.unwrap_or_default() }))
}

/// GET /record/level — текущий пик 0..1 (метр уровня).
async fn record_level() -> Json<Value> {
    Json(json!({ "level": record::level() }))
}

/// POST /record/start {name, device?} — начать запись голоса в voices/<name>.wav.
async fn record_start(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let name = sanitize_voice_name(body.get("name").and_then(|v| v.as_str()).unwrap_or("Мой голос"));
    let device = body.get("device").and_then(|v| v.as_str()).map(|s| s.to_string());
    let path = st.voices_dir.join(format!("{name}.wav"));
    match tokio::task::spawn_blocking(move || record::start(&path, device)).await {
        Ok(Ok(())) => Json(json!({ "ok": true, "name": name })),
        Ok(Err(e)) => Json(json!({ "ok": false, "error": e })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

/// POST /record/stop — остановить запись; вернуть имя записанного голоса + обновлённый список.
async fn record_stop(State(st): State<AppState>) -> Json<Value> {
    let path = tokio::task::spawn_blocking(record::stop).await;
    let name = match path {
        Ok(Ok(p)) => p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()),
        _ => None,
    };
    Json(json!({ "name": name, "voices": list_voice_names(&st.voices_dir) }))
}

/// POST /projects/{pid}/speaker-voice {speaker, name} — сделать голос из спикера: вырезать его
/// длиннейшую реплику из источника → voices/<name>.wav (16k mono, <=12с). Порт «Сделать голос» Higgs.
async fn speaker_voice(
    State(st): State<AppState>,
    axum::extract::Path(pid): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let want = body.get("speaker").and_then(|v| v.as_str()).unwrap_or("0").to_string();
    let name = sanitize_voice_name(body.get("name").and_then(|v| v.as_str()).unwrap_or(""));
    // длиннейшая реплика спикера.
    let cand = proj
        .segments
        .iter()
        .filter(|s| s.speaker.as_deref().unwrap_or("0") == want)
        .max_by(|a, b| (a.end - a.start).partial_cmp(&(b.end - b.start)).unwrap_or(std::cmp::Ordering::Equal));
    let Some(cand) = cand else {
        return (StatusCode::BAD_REQUEST, "у спикера нет реплик").into_response();
    };
    let (start, end) = (cand.start, cand.end.min(cand.start + 12.0));
    let ref_text = cand.src_text.trim().to_string();   // реф-текст = расшифровка реплики (как Higgs build_speaker_reference)
    let input = std::fs::read_to_string(dir.join("source.txt")).unwrap_or_default();
    let input = if !input.trim().is_empty() { PathBuf::from(input.trim()) } else { dir.join("source.mp4") };
    let out = st.voices_dir.join(format!("{name}.wav"));
    let txt = st.voices_dir.join(format!("{name}.txt"));
    let voices_dir = st.voices_dir.clone();
    let cli = st.bsroformer_cli.clone();
    let model = st.bsroformer_model.clone();
    let tmp = dir.join("_voicecut");
    let res = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&voices_dir).ok();
        std::fs::create_dir_all(&tmp).ok();
        let raw = tmp.join("cut.wav");
        media::trim(&input, &raw, start, end, 16_000)?;
        // очистка голоса: вокал-сепарация (Mel-Band Roformer) убирает фон/музыку из рефа (как voiceclean в
        // Higgs) — клон цепляется за чистый голос. Движку нужен 44.1к; без установленного движка -> сырой клип.
        let src_for_ref = if cli.is_file() && model.is_file() {
            let clip44 = tmp.join("cut44.wav");
            match media::extract_audio(&raw, &clip44, 44_100, 2)
                .and_then(|_| dub_sep::separate(&clip44, &tmp.join("stems"), &cli, &model).map_err(|e| e.to_string()))
            {
                Ok(sep) => sep.vocals,
                Err(_) => raw.clone(),
            }
        } else {
            raw.clone()
        };
        media::to_16k_mono(&src_for_ref, &out)?;   // реф в 16k mono
        if !ref_text.is_empty() {
            let _ = std::fs::write(&txt, &ref_text);   // голос несёт свой ref-текст в библиотеке
        }
        let _ = std::fs::remove_dir_all(&tmp);
        Ok::<(), String>(())
    })
    .await;
    match res {
        Ok(Ok(())) => Json(json!({ "ok": true, "name": name, "voices": list_voice_names(&st.voices_dir) })).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// POST /projects/{pid}/voice-slots {male:[имена], female:[имена]} — авто-распределение голосов-слотов
/// по спикерам (#114). Валидирует, что имена есть в voices/; определяет пол каждого спикера по медиане F0
/// его реплик на чистом вокале, ранжирует по длительности речи, раскладывает по слотам (по кругу сверх
/// числа слотов), записывает per-speaker голоса (voice.mode/name) и метит сегменты dirty (ре-синтез).
/// Возвращает итоговый маппинг {speaker:{voice,gender,f0}}. Пустой список пола -> клон; оба пусты -> клон.
async fn voice_slots_assign(
    State(st): State<AppState>,
    axum::extract::Path(pid): axum::extract::Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(r) => return r,
    };
    // Списки имён из тела; проверяем существование каждого в voices/ (.wav|.mp3).
    let names_of = |k: &str| -> Vec<String> {
        body.get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    };
    let male = names_of("male");
    let female = names_of("female");
    let available: std::collections::BTreeSet<String> = list_voice_names(&st.voices_dir).into_iter().collect();
    for n in male.iter().chain(female.iter()) {
        if !available.contains(n) {
            return (StatusCode::BAD_REQUEST, format!("голос {n:?} не найден в voices/")).into_response();
        }
    }
    // Чистый вокал analyze: vocals16_clean.wav, фолбэк vocals16.wav (сырой 16k). Нет ни того ни другого -> 409.
    let vocals = {
        let clean = dir.join("vocals16_clean.wav");
        let raw = dir.join("vocals16.wav");
        if clean.is_file() {
            clean
        } else if raw.is_file() {
            raw
        } else {
            return (StatusCode::CONFLICT, "нет вокала для замера F0 — сначала analyze").into_response();
        }
    };

    let slots = voice_slots::Slots { male, female };
    let dir_job = dir.clone();
    let res = tokio::task::spawn_blocking(move || {
        let assigns = voice_slots::assign(&mut proj, &vocals, &dir_job, &slots);
        save_project_atomic(&dir_job, &proj)?;
        Ok::<_, String>((assigns, proj))
    })
    .await;
    match res {
        Ok(Ok((assigns, proj))) => {
            let mapping: serde_json::Map<String, Value> = assigns
                .iter()
                .map(|a| {
                    let gender = match a.gender {
                        Some(voice_slots::Gender::Male) => Value::from("male"),
                        Some(voice_slots::Gender::Female) => Value::from("female"),
                        None => Value::Null,
                    };
                    (
                        a.speaker.clone(),
                        json!({
                            "voice": a.voice.clone().map(Value::from).unwrap_or(Value::Null),
                            "gender": gender,
                            "f0": a.f0,
                        }),
                    )
                })
                .collect();
            Json(json!({
                "ok": true,
                "voice_mode": proj.audio.voice.mode,
                "voice_name": proj.audio.voice.name,
                "speakers": mapping,
            }))
            .into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── Кастинг персонажей (#115) ──────────────────────────────────────────────

/// Единый шейп персонажа для фронта (тип Character в api.ts): id/name/gender/voice/speaker_ids/
/// line_count/sample_frame_url/voice_sample_url. Возвращают ОБА эндпоинта (get + save) — фронт кладёт
/// результат в один стейт Character[]. `voice` = dub_voice бэка; line_count = число сегментов проекта у
/// speaker_ids персонажа; sample_frame_url nullable (закадровый персонаж без кадра -> null); voice_sample_url
/// nullable (нет образца голоса -> null). `proj_dir` — workspace/<pid> для проверки наличия wav-образца.
fn character_json(c: &dub_faces::Character, pid: &str, proj: &Project, proj_dir: &Path) -> Value {
    let ids: std::collections::HashSet<&str> = c.speaker_ids.iter().map(|s| s.as_str()).collect();
    let line_count = proj
        .segments
        .iter()
        .filter(|s| ids.contains(s.speaker.as_deref().unwrap_or("0")))
        .count();
    // Образец голоса: путь из voice_sample (устойчив к смене id при кросс-матче); фолбэк на id-путь для
    // старых casting.json без поля. Есть файл -> URL, иначе null.
    let voice_rel = if c.voice_sample.is_empty() { char_voice_rel(&c.id) } else { Some(c.voice_sample.clone()) };
    let voice_sample_url = if voice_rel.as_ref().map(|r| proj_dir.join(r).is_file()).unwrap_or(false) {
        Value::from(format!("/projects/{pid}/casting/voice?id={}", c.id))
    } else {
        Value::Null
    };
    json!({
        "id": c.id,
        "name": c.name,
        "gender": c.gender,
        "voice": c.dub_voice,
        "speech_note": c.speech_note,
        "speaker_ids": c.speaker_ids,
        "line_count": line_count,
        // URL аватарки эндпоинта (без раскрытия файловых путей). Нет кадра -> null (фронт рисует инициал).
        "sample_frame_url": if c.sample_frame.is_empty() {
            Value::Null
        } else {
            Value::from(format!("/projects/{pid}/casting/avatar?id={}", c.id))
        },
        // URL проигрываемого образца голоса. Нет голоса -> null.
        "voice_sample_url": voice_sample_url,
    })
}

/// Относительный путь к wav-образцу голоса персонажа внутри work_dir по его id (char_<i> -> casting/
/// char_<i>_voice.wav). Нестандартный id -> None (voice_sample_url = null).
fn char_voice_rel(char_id: &str) -> Option<String> {
    // id формата char_<i>; допускаем только безопасные символы (без traversal).
    if char_id.is_empty() || !char_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(format!("casting/{char_id}_voice.wav"))
}

/// GET /projects/{pid}/casting — карточки персонажей из casting.json в шейпе фронта (см. character_json).
/// 404 если casting.json нет (кастинг не запускался).
async fn casting_get(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = dir.join("casting.json");
    let Some(casting) = dub_faces::load_casting(&path) else {
        return (StatusCode::NOT_FOUND, "casting not run").into_response();
    };
    let chars: Vec<Value> = casting
        .characters
        .iter()
        .map(|c| character_json(c, &pid, &proj, &dir))
        .collect();
    Json(json!({ "version": casting.version, "characters": chars })).into_response()
}

/// GET /projects/{pid}/casting/avatar?id=<char_id> — кадр-аватарка персонажа (JPG, с Range).
async fn casting_avatar(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let id = q.get("id").map(|s| s.as_str()).unwrap_or("");
    let Some(casting) = dub_faces::load_casting(&dir.join("casting.json")) else {
        return (StatusCode::NOT_FOUND, "casting not run").into_response();
    };
    let Some(ch) = casting.characters.iter().find(|c| c.id == id) else {
        return (StatusCode::NOT_FOUND, "character not found").into_response();
    };
    if ch.sample_frame.is_empty() {
        return (StatusCode::NOT_FOUND, "no avatar").into_response();
    }
    // sample_frame — относительный путь внутри work_dir (casting/char_N.jpg); отсекаем traversal.
    if ch.sample_frame.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let p = dir.join(&ch.sample_frame);
    if !p.is_file() {
        return (StatusCode::NOT_FOUND, "avatar file missing").into_response();
    }
    serve_file_range(&p, req, None).await
}

/// GET /projects/{pid}/casting/voice?id=<char_id> — проигрываемый образец голоса персонажа (WAV, с Range).
/// Из casting/char_<i>_voice.wav (кладёт casting.rs). Нет файла -> 404 (voice_sample_url и так был null).
async fn casting_voice(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let id = q.get("id").map(|s| s.as_str()).unwrap_or("");
    // Существует ли такой персонаж (id из casting.json) — иначе 404, не раскрываем ФС.
    let Some(casting) = dub_faces::load_casting(&dir.join("casting.json")) else {
        return (StatusCode::NOT_FOUND, "casting not run").into_response();
    };
    let Some(ch) = casting.characters.iter().find(|c| c.id == id) else {
        return (StatusCode::NOT_FOUND, "character not found").into_response();
    };
    // Резолвим по voice_sample (id мог смениться при кросс-матче — файл назван по индексу эпизода); фолбэк
    // на id-путь для старых casting.json. Гард от traversal: только casting/*.wav без "..".
    let rel = if ch.voice_sample.is_empty() {
        match char_voice_rel(id) {
            Some(r) => r,
            None => return (StatusCode::BAD_REQUEST, "bad id").into_response(),
        }
    } else if ch.voice_sample.starts_with("casting/") && !ch.voice_sample.contains("..") {
        ch.voice_sample.clone()
    } else {
        return (StatusCode::BAD_REQUEST, "bad voice path").into_response();
    };
    let p = dir.join(&rel);
    if !p.is_file() {
        return (StatusCode::NOT_FOUND, "no voice sample").into_response();
    }
    serve_file_range(&p, req, None).await
}

// ─── App-библиотека кастингов (#115): профили, переносимые между роликами ─────

/// GET /casting/library -> {"casts":[{"slug","name","char_count"}]} — список сохранённых профилей.
async fn casting_library_list(State(st): State<AppState>) -> Response {
    let repo_root = st.repo_root.clone();
    let profiles = tokio::task::spawn_blocking(move || casting_library::list_profiles(&repo_root))
        .await
        .unwrap_or_default();
    let casts: Vec<Value> = profiles
        .into_iter()
        .map(|(slug, m)| json!({ "slug": slug, "name": m.name, "char_count": m.char_count }))
        .collect();
    Json(json!({ "casts": casts })).into_response()
}

/// POST /projects/{pid}/casting/library {name, created?} -> {"slug"} — сохранить текущий casting проекта
/// в библиотеку (casting.json + аватарки + образцы голоса). slug из name (транслит/kebab, уникализируется).
async fn casting_library_save(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Json(body): Json<Value>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }
    // created — из тела запроса (Date::now в Rust не используем); пусто допустимо.
    let created = body.get("created").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let repo_root = st.repo_root.clone();
    let res = tokio::task::spawn_blocking(move || {
        let base = casting_library::slugify(&name);
        let slug = casting_library::unique_slug(&repo_root, &base);
        casting_library::save_profile(&repo_root, &dir, &slug, &name, &created).map(|_| slug)
    })
    .await;
    match res {
        Ok(Ok(slug)) => Json(json!({ "slug": slug })).into_response(),
        Ok(Err(e)) => (StatusCode::CONFLICT, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// DELETE /casting/library/{slug} -> {"ok":true} — удалить профиль библиотеки.
async fn casting_library_delete(
    State(st): State<AppState>,
    AxPath(slug): AxPath<String>,
) -> Response {
    if !casting_library::is_safe_slug(&slug) {
        return (StatusCode::BAD_REQUEST, "bad slug").into_response();
    }
    let repo_root = st.repo_root.clone();
    let res = tokio::task::spawn_blocking(move || casting_library::delete_profile(&repo_root, &slug)).await;
    match res {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /casting/library/{slug}/avatar?id=<char_id> — аватарка персонажа профиля (JPG/PNG, с Range).
async fn casting_library_avatar(
    State(st): State<AppState>,
    AxPath(slug): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    if !casting_library::is_safe_slug(&slug) {
        return (StatusCode::BAD_REQUEST, "bad slug").into_response();
    }
    let id = q.get("id").map(|s| s.as_str()).unwrap_or("");
    // id формата char_<i>; берём аватарку avatars/char_<i>.jpg (нативный JPG), фолбэк на .png для старых
    // профилей. Эндпоинт резолвит по id персонажа.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return (StatusCode::BAD_REQUEST, "bad id").into_response();
    }
    let dir = casting_library::profile_dir(&st.repo_root, &slug);
    // Проверяем, что такой персонаж есть в профиле (не раскрываем ФС при мусорном id).
    let Some(casting) = casting_library::load_profile_casting(&st.repo_root, &slug) else {
        return (StatusCode::NOT_FOUND, "profile not found").into_response();
    };
    if !casting.characters.iter().any(|c| c.id == id) {
        return (StatusCode::NOT_FOUND, "character not found").into_response();
    }
    let avdir = dir.join("avatars");
    let p = {
        let jpg = avdir.join(format!("{id}.jpg"));
        if jpg.is_file() { jpg } else { avdir.join(format!("{id}.png")) }
    };
    if !p.is_file() {
        return (StatusCode::NOT_FOUND, "avatar file missing").into_response();
    }
    serve_file_range(&p, req, None).await
}

/// POST /projects/{pid}/casting {characters:[{id,name,speech_note,dub_voice,gender?}]} — сохранить правки
/// юзера в casting.json И применить: (1) per-speaker голоса тем же механизмом, что voice_slots/ручное
/// назначение (voice.mode="voice" + позиционный CSV по отсортированным id спикеров); (2) speech_note ->
/// единый translate_style (пер-спикерный style в ctx_run НЕ поддержан — глобальный; см. casting.rs +
/// translate.rs). Метит сегменты dirty при смене голоса (ре-синтез). Возвращает обновлённый кастинг.
async fn casting_save(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Json(body): Json<Value>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = dir.join("casting.json");
    let Some(mut casting) = dub_faces::load_casting(&path) else {
        return (StatusCode::CONFLICT, "casting not run").into_response();
    };
    // Применить правки по id: name/speech_note/dub_voice/gender.
    if let Some(edits) = body.get("characters").and_then(|v| v.as_array()) {
        for e in edits {
            let Some(id) = e.get("id").and_then(|v| v.as_str()) else { continue };
            let Some(ch) = casting.characters.iter_mut().find(|c| c.id == id) else { continue };
            if let Some(v) = e.get("name").and_then(|v| v.as_str()) {
                ch.name = v.to_string();
            }
            if let Some(v) = e.get("speech_note").and_then(|v| v.as_str()) {
                ch.speech_note = v.to_string();
            }
            // dub_voice: строка -> назначить; JSON null (фронт шлёт при очистке поля) -> СБРОС на клон ("").
            // Ключ отсутствует -> не трогаем. Иначе вернуть голос на клонирование из UI было невозможно.
            match e.get("dub_voice") {
                Some(v) if v.is_string() => ch.dub_voice = v.as_str().unwrap_or("").to_string(),
                Some(v) if v.is_null() => ch.dub_voice = String::new(),
                _ => {}
            }
            if let Some(v) = e.get("gender").and_then(|v| v.as_str()) {
                ch.gender = v.to_string();
            }
        }
    }
    // Валидация голосов. В ОБЛАЧНОМ TTS-режиме (OpenRouter) имена — облачные голоса, локального файла в
    // voices/ у них нет: проверку против voices/ пропускаем (cloud TTS сам примет имя голоса при синтезе).
    // Локальный режим — как раньше: непустой не-"clone" должен существовать в voices/.
    if !crate::models::openrouter_stage_on(&st.models_root, "tts") {
        let available: std::collections::BTreeSet<String> =
            list_voice_names(&st.voices_dir).into_iter().collect();
        for c in &casting.characters {
            let v = c.dub_voice.trim();
            if !v.is_empty() && !v.eq_ignore_ascii_case("clone") && !available.contains(v) {
                return (StatusCode::BAD_REQUEST, format!("голос {v:?} не найден в voices/")).into_response();
            }
        }
    }

    // (1) применить голоса per-speaker: позиционный CSV по лексикографически отсортированным id спикеров
    // (как читает render.rs / пишет voice_slots). Слияние ПОВЕРХ старого CSV — см. merge_voice_csv:
    // спикера на 'clone' в кастинге -> "-", спикера НЕ из кастинга -> сохраняем его позицию из #114.
    let vmap = casting::casting_voice_map(&casting);
    let mut spk_ids: Vec<String> = proj
        .segments
        .iter()
        .map(|s| s.speaker.clone().unwrap_or_else(|| "0".to_string()))
        .collect();
    spk_ids.sort();
    spk_ids.dedup();
    let any_voice = vmap.values().any(|v| v.is_some());
    // Голоса из кастинга применяем ТОЛЬКО когда юзер реально назначил хоть один (any_voice). Иначе
    // (все на "clone") НЕ трогаем proj.audio.voice — иначе перезатёрли бы библиотечные голоса из #114
    // (voice_slots) и пометили бы ВСЕ сегменты dirty на пустой ре-синтез.
    if any_voice {
        let old_name = proj.audio.voice.name.clone();
        // Новый CSV строим ПОВЕРХ текущего (см. casting::merge_voice_csv): спикеров, НЕ настроенных в
        // кастинге, НЕ трогаем — сохраняем их позицию из voice_slots (#114). "-" ставим только тем, кого
        // реально выставили в кастинге на 'clone'. Формат позиционный по отсортированным id (как render.rs).
        let old_is_voice = proj.audio.voice.mode == "voice";
        let csv = casting::merge_voice_csv(&spk_ids, &vmap, old_name.as_deref(), old_is_voice);
        proj.audio.voice.mode = "voice".to_string();
        proj.audio.voice.name = Some(csv);
        // Смена голосов -> ре-синтез: метим сегменты dirty (как voice_slots).
        if proj.audio.voice.name != old_name {
            for seg in &mut proj.segments {
                seg.dirty = true;
            }
        }
    }

    // (2) speech_note -> глобальный translate_style (документированное ограничение: ctx_run не берёт
    // пер-спикерный стиль). Дополняем существующий стиль заметками персонажей.
    let notes = casting::speech_notes_to_style(&casting);
    proj.audio.translate_style = casting::merge_speech_notes_style(&proj.audio.translate_style, &notes);

    // Сохранить casting.json + project.json.
    if let Err(e) = dub_faces::save_casting(&path, &casting) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    if let Err(e) = save_project_atomic(&dir, &proj) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    // Возвращаем ПОЛНЫЙ список персонажей в шейпе фронта (тот же, что casting_get) — фронт кладёт его
    // прямо в стейт Character[]. line_count считаем по обновлённому proj (спикеры не менялись, но шейп единый).
    let chars: Vec<Value> = casting
        .characters
        .iter()
        .map(|c| character_json(c, &pid, &proj, &dir))
        .collect();
    Json(json!({ "ok": true, "characters": chars })).into_response()
}

/// GET /voices/catalog — каталог доп-голосов (HF датасет Slait/russia_voices): имя, пол, URL-превью.
async fn voices_catalog() -> Json<Value> {
    Json(tokio::task::spawn_blocking(record::catalog).await.unwrap_or_else(|_| json!({ "voices": [] })))
}

/// POST /voices/get {name} — скачать один голос (mp3+txt) из датасета в каталог голосов.
async fn voices_get(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let dir = st.voices_dir.clone();
    let ok = tokio::task::spawn_blocking(move || record::fetch_voice(&dir, &name)).await;
    match ok {
        Ok(Ok(())) => Json(json!({ "ok": true, "voices": list_voice_names(&st.voices_dir) })),
        Ok(Err(e)) => Json(json!({ "ok": false, "error": e })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

/// POST /voices/rename {from, to} — переименовать голос (mp3/wav + txt).
async fn voices_rename(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let from = sanitize_voice_name(body.get("from").and_then(|v| v.as_str()).unwrap_or(""));
    let to = sanitize_voice_name(body.get("to").and_then(|v| v.as_str()).unwrap_or(""));
    for ext in ["wav", "mp3", "txt"] {
        let src = st.voices_dir.join(format!("{from}.{ext}"));
        if src.is_file() {
            let _ = std::fs::rename(&src, st.voices_dir.join(format!("{to}.{ext}")));
        }
    }
    Json(json!({ "voices": list_voice_names(&st.voices_dir) }))
}

/// POST /voices/delete {name} — удалить голос.
async fn voices_delete(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let name = sanitize_voice_name(body.get("name").and_then(|v| v.as_str()).unwrap_or(""));
    for ext in ["wav", "mp3", "txt"] {
        let _ = std::fs::remove_file(st.voices_dir.join(format!("{name}.{ext}")));
    }
    Json(json!({ "voices": list_voice_names(&st.voices_dir) }))
}

/// POST /voices/download-pack — скачать пак голосов (VibeVoice) и распаковать в каталог голосов.
async fn voices_download_pack(State(st): State<AppState>) -> Response {
    let dir = st.voices_dir.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        let cb = |ev: Value| progress(ev);
        record::download_pack(&dir, &cb)
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

/// POST /setup/browse — открыть нативный диалог выбора папки, импортировать оттуда готовые модели по
/// маркерам (без докачки). {picked:bool, imported:[...], status}.
async fn setup_browse(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let root = st.repo_root.clone();
    let only = body.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let res = tokio::task::spawn_blocking(move || {
        let title = if only.is_some() { "Файл(ы) модели" } else { "Папка с готовыми моделями" };
        let dir = rfd::FileDialog::new().set_title(title).pick_folder();
        match dir {
            Some(d) => (true, setup::import_from_dir(&root, &d, only.as_deref())),
            None => (false, Vec::new()),
        }
    })
    .await
    .unwrap_or((false, Vec::new()));
    Json(json!({ "picked": res.0, "imported": res.1, "status": setup::setup_status(&st.repo_root) }))
}

/// POST /setup/import {path, id?} — импортировать готовые модели из заданной папки (без диалога).
async fn setup_import(State(st): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let only = body.get("id").and_then(|v| v.as_str());
    let imported = if path.is_empty() {
        Vec::new()
    } else {
        setup::import_from_dir(&st.repo_root, std::path::Path::new(path), only)
    };
    Json(json!({ "imported": imported, "status": setup::setup_status(&st.repo_root) }))
}

// ─── POST /projects (multipart video upload) ────────────────────────────────

async fn create_project(
    State(st): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let mut pid = uuid::Uuid::new_v4().simple().to_string();
    pid.truncate(12);
    let d = st.workspace.join(&pid);
    if let Err(e) = tokio::fs::create_dir_all(&d).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Поля мультипарта: видео + (опц.) файл субтитров. Роутим по расширению — порядок неважен.
    // Субтитры (srt/ass/ssa) -> import_subs.<ext> (analyze возьмёт текст+тайминг оттуда, ASR skip).
    let mut filename: Option<String> = None;
    let mut imported_subs = false;
    while let Ok(Some(field)) = multipart.next_field().await {
        let fname = field.file_name().map(|s| s.to_string());
        let data = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };
        let ext = fname
            .as_deref()
            .and_then(|f| Path::new(f).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "srt" | "ass" | "ssa") {
            let dst = d.join(format!("import_subs.{ext}"));
            if let Err(e) = tokio::fs::write(&dst, &data).await {
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
            imported_subs = true;
            continue;
        }
        let dst = d.join(format!("source.{ext}"));
        if let Err(e) = tokio::fs::write(&dst, &data).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        // source.txt хранит абсолютный путь к видео (как в app.py).
        let _ = tokio::fs::write(d.join("source.txt"), dst.to_string_lossy().as_bytes()).await;
        // name.txt — исходное имя файла (для списка «недавние проекты»; meta.video хранит внутреннее source.*).
        if let Some(n) = fname.as_deref() {
            let _ = tokio::fs::write(d.join("name.txt"), n.as_bytes()).await;
        }
        filename = fname;
    }

    // Создаём начальный project.json, чтобы проект сразу мог быть открыт в редакторе без вызова analyze
    if let Ok(src_path_str) = tokio::fs::read_to_string(d.join("source.txt")).await {
        let src_path = Path::new(src_path_str.trim());
        let meta = match media::probe(src_path) {
            Ok(m) => dub_core::Meta {
                video: src_path.to_string_lossy().to_string(),
                duration: m.duration,
                width: m.width,
                height: m.height,
                fps: m.fps,
                src_codec: m.src_codec,
                extra: serde_json::Map::new(),
            },
            Err(_) => dub_core::Meta {
                video: src_path.to_string_lossy().to_string(),
                duration: 0.0,
                width: 1920,
                height: 1080,
                fps: 30.0,
                src_codec: String::new(),
                extra: serde_json::Map::new(),
            },
        };

        // Загрузить реплики из import_subs если они были переданы при создании
        let mut segments = Vec::new();
        for ext in ["srt", "ass", "ssa"] {
            let sub_file = d.join(format!("import_subs.{ext}"));
            if sub_file.is_file() {
                if let Ok(txt) = std::fs::read_to_string(&sub_file) {
                    let cues = subimport::parse(&txt, ext);
                    for (i, c) in cues.into_iter().enumerate() {
                        segments.push(dub_core::Segment {
                            id: format!("seg_{}", i + 1),
                            start: c.start,
                            end: c.end,
                            speaker: Some("0".to_string()),
                            src_text: c.text.clone(),
                            tgt_text: c.text,
                            voice: None,
                            dirty: true,
                            ckpt: None,
                            extra: serde_json::Map::new(),
                        });
                    }
                }
                break;
            }
        }

        let initial_proj = dub_core::Project {
            meta,
            mode: "nodub".to_string(),
            tgt_lang: "ru".to_string(),
            segments,
            audio: dub_core::Audio::default(),
            subs: dub_core::Subs::default(),
            captions: dub_core::Captions::default(),
            render: dub_core::Render::default(),
            ..Default::default()
        };
        let _ = save_project_atomic(&d, &initial_proj);
    }

    Json(json!({ "project_id": pid, "filename": filename, "imported_subs": imported_subs })).into_response()
}

// ─── GET /projects ──────────────────────────────────────────────────────────
// Список сохранённых проектов (у которых есть project.json) для экрана «Открыть/недавние».
// Всё уже персистится в workspace/<pid>/ (каждый PATCH пишет project.json — автосейв),
// здесь просто отдаём сводку, сортированную по времени последней правки (новые сверху).
async fn list_projects(State(st): State<AppState>) -> Response {
    let mut items: Vec<Value> = Vec::new();
    let mut total_bytes: u64 = 0;
    if let Ok(rd) = std::fs::read_dir(&st.workspace) {
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Ok(m) = entry.metadata() {
                        total_bytes += m.len();
                    }
                }
            }
            let pj = dir.join("project.json");
            if !pj.is_file() {
                continue; // только проанализированные (редактируемые) проекты
            }
            let pid = match dir.file_name().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let proj = match std::fs::read_to_string(&pj)
                .ok()
                .and_then(|t| Project::from_json(&t).ok())
            {
                Some(p) => p,
                None => continue,
            };
            let mtime = std::fs::metadata(&pj)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Имя: исходный файл из name.txt (новые проекты), иначе basename meta.video (старые).
            let video = std::fs::read_to_string(dir.join("name.txt"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    Path::new(&proj.meta.video)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(proj.meta.video.as_str())
                        .to_string()
                });
            let audio_only = proj.meta.width <= 0 || proj.meta.height <= 0;
            let done = dir.join("output.mp4").is_file()
                || dir.join("output.mkv").is_file()
                || dir.join("output.wav").is_file();
            items.push(json!({
                "pid": pid,
                "video": video,
                "tgt_lang": proj.tgt_lang,
                "mode": proj.mode,
                "width": proj.meta.width,
                "height": proj.meta.height,
                "duration": proj.meta.duration,
                "segments": proj.segments.len(),
                "audio_only": audio_only,
                "mtime": mtime,
                "done": done,
            }));
        }
    }
    items.sort_by(|a, b| {
        b["mtime"].as_u64().unwrap_or(0).cmp(&a["mtime"].as_u64().unwrap_or(0))
    });
    Json(json!({ "projects": items, "total_bytes": total_bytes })).into_response()
}

/// POST /projects/{pid}/open-folder — открыть папку проекта в Проводнике Windows.
async fn open_project_folder(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let path = dir.to_string_lossy().to_string();
    let _ = tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            let _ = crate::media::cmd_silent("cmd").args(["/C", "explorer", &path]).spawn();
        }
    });
    ([("content-type", "application/json")], "{\"ok\":true}").into_response()
}

// ─── GET /projects/{pid} ────────────────────────────────────────────────────

async fn get_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    match st.load_project(&pid) {
        Ok(p) => match p.to_json() {
            Ok(s) => ([("content-type", "application/json")], s).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(resp) => resp,
    }
}

// ─── DELETE /projects/{pid} — удалить проект (весь каталог workspace/<pid>) ───
// Убирает проект из «последних» на главной И реально стирает его данные с диска.
async fn delete_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp, // невалидный/несуществующий pid -> та же ошибка, что у прочих ручек
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => ([("content-type", "application/json")], "{\"ok\":true}").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("удаление проекта {}: {e}", dir.display()),
        )
            .into_response(),
    }
}

/// POST /projects/{pid}/trash — переместить проект в корзину Windows.
async fn trash_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    if !dir.is_dir() {
        return ([("content-type", "application/json")], "{\"ok\":true}").into_response();
    }
    let path = dir.to_string_lossy().to_string();
    let res = tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            let ps_cmd = format!(
                "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory('{}', 'OnlyErrorDialogs', 'SendToRecycleBin')",
                path.replace("'", "''")
            );
            let status = crate::media::cmd_silent("powershell")
                .args(["-NoProfile", "-Command", &ps_cmd])
                .status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
        std::fs::remove_dir_all(std::path::Path::new(&path)).map_err(|e| e.to_string())
    }).await;

    match res {
        Ok(Ok(())) => ([("content-type", "application/json")], "{\"ok\":true}").into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("trash failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST /projects/{pid}/analyze ───────────────────────────────────────────

async fn analyze_project(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    // Исходный путь видео из source.txt (как в app.py). Fallback — source.* в каталоге.
    let src_txt = dir.join("source.txt");
    let input = match std::fs::read_to_string(&src_txt) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => {
            return (StatusCode::CONFLICT, "no source uploaded").into_response();
        }
    };
    if !input.is_file() {
        return (StatusCode::CONFLICT, "source video missing").into_response();
    }

    // Параметры analyze из query (дефолты как в app.py.analyze_project).
    let qget = |k: &str, d: &str| q.get(k).cloned().unwrap_or_else(|| d.to_string());
    // Импортированные субтитры (если юзер загрузил их при создании) — берём текст+тайминг оттуда, ASR skip.
    let import_subs = ["srt", "ass", "ssa"]
        .iter()
        .map(|e| dir.join(format!("import_subs.{e}")))
        .find(|p| p.is_file());
    if let Some(p) = &import_subs {
        eprintln!("[analyze] импорт субтитров: {}", p.display());
    }
    let args = analyze::AnalyzeArgs {
        tgt_lang: qget("tgt_lang", "en"),
        mode: qget("mode", "auto"),
        src_lang: qget("src_lang", "auto"),
        subs: qget("subs", "auto"),
        rewrite: qget("rewrite", ""),
        translate_style: qget("translate_style", ""),
        burn: qget("burn", "1") != "0",
        detect_text: qget("detect", "1") != "0",
        // кастинг персонажей (#115): ВЫКЛ по умолчанию (дорогой скан кадров SCRFD/LVFace).
        casting: qget("casting", "0") == "1",
        // slug профиля app-библиотеки кастингов (#115): применить к этому ролику. Пусто = без применения.
        casting_ref: qget("casting_ref", ""),
        // тип контента кастинга (#115): "auto" (Gemma-детект) | "real" (SCRFD+LVFace) | "anime" (рисованные
        // лица + CCIP). Дефолт "auto" в паритет с UI (App.tsx) — при casting=off поле инертно.
        content_type: qget("content_type", "auto"),
        // «сабы уже на языке перевода» — эффективно только если сабы реально импортированы.
        import_translated: import_subs.is_some() && qget("import_translated", "0") == "1",
    };
    // Активный вариант модели резолвится ПРИ КАЖДОЙ джобе (не морозится на старте): скачал/выбрал
    // квант -> применяется без рестарта. См. models::resolve_*.
    let sel = models::load_selection(&st.models_root);
    // Backend локальных ONNX-стадий (диаризация Sortformer / Parakeet): exec_config читает DUB_ASR_BACKEND
    // при создании сессии — gpu регистрирует CUDA-EP с error_on_failure (недоступность = ошибка, не тихий
    // CPU-фоллбек), иначе CPU-провайдер. Задаём из local_backend на КАЖДУЮ джобу (переключение без рестарта).
    std::env::set_var("DUB_ASR_BACKEND", models::local_backend(&st.models_root));
    let (mt_model, mmproj) = models::resolve_mt(&st.models_root, &sel);
    let asr = models::resolve_asr_choice(&st.repo_root, &st.models_root, &sel);
    eprintln!("[models] analyze: MT={} · ASR={}", mt_model.display(), asr.describe());
    let paths = analyze::AnalyzePaths {
        input,
        work_dir: dir.clone(),
        repo_root: st.repo_root.clone(),
        asr,
        sortformer_onnx: st.sortformer_onnx.clone(),
        llama_bin: st.llama_bin.clone(),
        mt_model,
        mmproj,
        models_root: st.models_root.clone(),
        caption_fps: st.opts.caption_fps,
        import_subs,
        // Сепарация ДО диаризации/ASR (best practices): чистый вокал; stems-кэш общий с рендером.
        bsroformer_cli: dub_sep::engine_cli(&st.repo_root, models::stage_backend(&st.models_root, "sep_backend")),
        bsroformer_model: st.bsroformer_model.clone(),
    };

    // Тело джобы: analyze -> project.json (атомарно). Прогресс -> SSE.
    let dir_for_save = dir.clone();
    let pid_for_result = pid.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        let mut paths = paths;
        let cb = |ev: Value| progress(ev);
        // On-demand: если для этой функции (кастинг) или backend (GPU) не хватает моделей — тянем их СЕЙЧАС,
        // до анализа. Иначе кастинг «не видел» бы лиц, а GPU-стадии падали бы с ошибкой CUDA.
        ensure_job_components(&paths.repo_root, &paths.models_root, args.casting, &cb)?;
        // CUDA-библиотеки Whisper могут быть догружены строкой выше. Резолв до догрузки зафиксировал
        // бы CPU для всей текущей job, поэтому выбираем бинарь/модель/device заново прямо перед ASR.
        let sel = models::load_selection(&paths.models_root);
        paths.asr = models::resolve_asr_choice(&paths.repo_root, &paths.models_root, &sel);
        eprintln!("[models] analyze (after setup): ASR={}", paths.asr.describe());
        // Трекинг затрат OpenRouter в ДОЛЛАРАХ (перевод/vision через облако): total_usage до/после.
        let cost_before = openrouter_cli::total_usage_usd(&paths.models_root);
        let proj = analyze::run(&args, &paths, &cb)?;
        if let (Some(b), Some(a)) = (cost_before, openrouter_cli::total_usage_usd(&paths.models_root)) {
            let spent = (a - b).max(0.0);
            if spent > 0.0 {
                cb(json!({ "stage": "cost", "msg": format!("OpenRouter: потрачено ${spent:.4} за анализ (всего использовано ${a:.2})") }));
            }
        }
        save_project_atomic(&dir_for_save, &proj)?;
        Ok(json!({ "project_id": pid_for_result, "output": dir_for_save.join("project.json").to_string_lossy() }))
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

// ─── PATCH /projects/{pid} ──────────────────────────────────────────────────

async fn patch_project(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Json(edit): Json<Value>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let mut proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if let Err((code, msg)) = patch::apply(&mut proj, &edit) {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
        return (status, msg).into_response();
    }
    if let Err(e) = save_project_atomic(&dir, &proj) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    match proj.to_json() {
        Ok(s) => ([("content-type", "application/json")], s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST /projects/{pid}/render ────────────────────────────────────────────

/// Сбросить dirty у сегментов, чьи правки уже запечены в дубляж/рендер. Перечитываем project.json с
/// диска (а не пишем захваченный proj), чтобы не затереть правки, пришедшие во время job. Общий хвост
/// render_project и dub_audio_project (был байт-в-байт продублирован).
fn reset_baked_dirty(proj: &Project, proj_path: &Path, dir_for_job: &Path) {
    let baked: std::collections::HashMap<&str, &str> =
        proj.segments.iter().map(|s| (s.id.as_str(), s.tgt_text.as_str())).collect();
    if let Ok(t2) = std::fs::read_to_string(proj_path) {
        if let Ok(mut cur) = Project::from_json(&t2) {
            for s in &mut cur.segments {
                if baked.get(s.id.as_str()).copied() == Some(s.tgt_text.as_str()) {
                    s.dirty = false;
                }
            }
            let _ = save_project_atomic(dir_for_job, &cur);
        }
    }
}

async fn render_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let proj_path = dir.join("project.json");
    if !proj_path.is_file() {
        return (StatusCode::CONFLICT, "project not analyzed yet").into_response();
    }
    // Исходный путь видео из source.txt (как в analyze/app.py).
    let input = match std::fs::read_to_string(dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source uploaded").into_response(),
    };
    let output = dir.join("output.mp4");

    // Активный вариант модели резолвится ПРИ КАЖДОЙ джобе (см. models::resolve_*): скачал/выбрал
    // квант в настройках -> применяется без рестарта сервера.
    let sel = models::load_selection(&st.models_root);
    let (higgs_model_root, higgs_quant) = models::resolve_tts(&st.models_root, &sel);
    let sep_model = models::resolve_sep(&st.models_root, &sel);
    eprintln!("[models] render: TTS={} (q={}) · SEP={}", higgs_model_root.display(), higgs_quant, sep_model.display());
    let paths = render::RenderPaths {
        input,
        bench: models::bench_enabled(&st.models_root),
        work_dir: dir.clone(),
        output: output.clone(),
        bsroformer_cli: dub_sep::engine_cli(&st.repo_root, models::stage_backend(&st.models_root, "sep_backend")),
        bsroformer_model: sep_model,
        higgs_dll: st.higgs_dll.clone(),
        higgs_model_root,
        higgs_quant,
        fonts_dir: st.fonts_dir.clone(),
        higgs_backend: "cuda".to_string(),
        higgs_device: 0,
        higgs_threads: st.opts.num_threads,
        max_stretch: st.opts.max_stretch as f64,
        voices_dir: st.voices_dir.clone(),
        asr: models::resolve_asr_choice(&st.repo_root, &st.models_root, &sel),
        ref_secs: models::higgs_ref_secs(&st.models_root),
        models_root: st.models_root.clone(),
        tts_cache: st.tts_cache.clone(),
    };

    let dir_for_job = dir.clone();
    let out_for_result = output.clone();
    let repo_root_for_job = st.repo_root.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        let mut paths = paths;
        let cb = |ev: Value| progress(ev);
        // On-demand: GPU-стадии рендера (Higgs TTS / сепарация на CUDA) без CUDA-стека -> догрузить сейчас.
        ensure_job_components(&repo_root_for_job, &paths.models_root, false, &cb)?;
        // Как и в analyze: после on-demand загрузки CUDA заново выбираем Whisper, чтобы ASR-QC и
        // транскрипция референсов использовали CUDA уже в этой, а не следующей job.
        let sel = models::load_selection(&paths.models_root);
        paths.asr = models::resolve_asr_choice(&repo_root_for_job, &paths.models_root, &sel);
        eprintln!("[models] render (after setup): ASR={}", paths.asr.describe());
        // Загрузить свежий Project (правки могли прийти после enqueue).
        let text = std::fs::read_to_string(&proj_path).map_err(|e| e.to_string())?;
        let proj = Project::from_json(&text).map_err(|e| e.to_string())?;
        // regen_dub если есть dirty-сегменты (voice/text/rewrite правились).
        let regen = proj.segments.iter().any(|s| s.dirty);
        // Трекинг затрат OpenRouter в ДОЛЛАРАХ: total_usage до/после (None -> облако не использовалось).
        let cost_before = openrouter_cli::total_usage_usd(&paths.models_root);
        render::run(&proj, &paths, regen, &cb)?;
        if let (Some(b), Some(a)) = (cost_before, openrouter_cli::total_usage_usd(&paths.models_root)) {
            let spent = (a - b).max(0.0);
            if spent > 0.0 {
                cb(json!({ "stage": "cost", "msg": format!("OpenRouter: потрачено ${spent:.4} за прогон (всего использовано ${a:.2})") }));
            }
        }
        // Правки запечены в дубляж -> сбросить dirty (перечитать, чтобы не затереть правки во время рендера).
        if regen {
            reset_baked_dirty(&proj, &proj_path, &dir_for_job);
        }
        Ok(json!({ "output": out_for_result.to_string_lossy() }))
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

/// Клонировать каталог проекта для ре-перевода на другой язык: копируем ВСЁ, кроме пожадорных/финальных
/// выводов (они перегенерируются рендером). Сохраняем project.json (раскладка/стили/блюр/титры), исходник,
/// референсы клона голоса (ref_*.wav) и разделённые вокал/аудио-кэши — чтобы дубляж взял ТОТ ЖЕ голос,
/// а раскладка субтитров осталась как у пользователя. НЕ гоняем заново ASR/OCR/vision.
fn clone_project_for_relang(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let rd = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue; // подкаталоги (frames/) не нужны — vision/OCR не перезапускаем
        }
        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let skip = name.starts_with("seg_")
            || name.starts_with("output.")
            || name == "captioned.mp4"
            || name.starts_with("_preview")
            || name == "_original.png"
            || name == "new_audio.m4a"
            || name == "final_audio.m4a"
            || name == "dub_audio.m4a"
            || name == "dub_fit.wav";
        if !skip {
            std::fs::copy(&p, dst.join(&name)).map_err(|e| format!("copy {name}: {e}"))?;
        }
    }
    Ok(())
}

// ─── POST /projects/{pid}/export-lang?lang=Lx ───────────────────────────────
// Экспорт-уровень мультиязыка: КЛОНИРУЕТ уже отредактированный проект (сохраняя раскладку субтитров,
// стили, блюр, титры и клон голоса) в НОВЫЙ проект на языке Lx, переводит ТОЛЬКО текст (сегменты+титры)
// через Gemma (flat_run — без пере-раскладки vision), помечает dirty и рендерит. Возвращает {project_id, job_id}.
async fn export_lang(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let lang = q.get("lang").cloned().unwrap_or_default().trim().to_string();
    if lang.is_empty() {
        return (StatusCode::BAD_REQUEST, "no lang").into_response();
    }
    let src_dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !src_dir.join("project.json").is_file() {
        return (StatusCode::CONFLICT, "project not analyzed yet").into_response();
    }
    let mut new_pid = uuid::Uuid::new_v4().simple().to_string();
    new_pid.truncate(12);
    let dst_dir = st.workspace.join(&new_pid);
    if let Err(e) = clone_project_for_relang(&src_dir, &dst_dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    let input = match std::fs::read_to_string(dst_dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source").into_response(),
    };

    let llama_bin = st.llama_bin.clone();
    let mt_model = st.opts.mt_model_path.clone();
    let models_root_xl = st.models_root.clone();
    let sel = models::load_selection(&st.models_root);
    let (higgs_model_root, higgs_quant) = models::resolve_tts(&st.models_root, &sel);
    let sep_model = models::resolve_sep(&st.models_root, &sel);
    let output = dst_dir.join("output.mp4");
    let paths = render::RenderPaths {
        input,
        bench: models::bench_enabled(&st.models_root),
        work_dir: dst_dir.clone(),
        output: output.clone(),
        bsroformer_cli: dub_sep::engine_cli(&st.repo_root, models::stage_backend(&st.models_root, "sep_backend")),
        bsroformer_model: sep_model,
        higgs_dll: st.higgs_dll.clone(),
        higgs_model_root,
        higgs_quant,
        fonts_dir: st.fonts_dir.clone(),
        higgs_backend: "cuda".to_string(),
        higgs_device: 0,
        higgs_threads: st.opts.num_threads,
        max_stretch: st.opts.max_stretch as f64,
        voices_dir: st.voices_dir.clone(),
        asr: models::resolve_asr_choice(&st.repo_root, &st.models_root, &sel),
        ref_secs: models::higgs_ref_secs(&st.models_root),
        models_root: st.models_root.clone(),
        tts_cache: st.tts_cache.clone(),
    };

    let dst_for_job = dst_dir.clone();
    let lang_c = lang.clone();
    let new_pid_res = new_pid.clone();
    let out_res = output.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        use dub_translate::{flat_run, Seg};
        let pj = dst_for_job.join("project.json");
        let text = std::fs::read_to_string(&pj).map_err(|e| e.to_string())?;
        let mut p = Project::from_json(&text).map_err(|e| e.to_string())?;
        p.tgt_lang = lang_c.clone();
        let spoken = matches!(p.mode.as_str(), "dub" | "voiceover");
        progress(json!({ "type": "progress", "stage": "translate",
            "msg": format!("Перевод {} строк → {}", p.segments.len(), lang_c) }));
        // LLM-провайдер: облако OpenRouter (если включено) ИЛИ локальный llama-server (плоский MT).
        let prov = crate::llm_provider::open(
            &crate::llm_provider::LlmOpen {
                llama_bin: &llama_bin,
                mt_model: &mt_model,
                mmproj: std::path::Path::new(""),
                models_root: &models_root_xl,
            },
            crate::llm_provider::LlmMode::Text,
        )
        .map_err(|e| format!("перевод: LLM недоступен — {e}"))?;
        let client = prov.client();
        // Сегменты: src_text -> Lx (раскладка/стили/тайминг остаются от пользователя).
        let mut segs: Vec<Seg> = p
            .segments
            .iter()
            .map(|s| {
                let spk = crate::analyze::speaker_to_i64(s.speaker.as_deref());
                // Вручную добавленные фразы имеют пустой src_text — их перевод берём из текущего tgt_text
                // (он на СТАРОМ целевом языке), иначе flat_run их пропустит и они останутся на старом языке.
                let src = if s.src_text.trim().is_empty() { s.tgt_text.clone() } else { s.src_text.clone() };
                Seg::new(src, spk)
            })
            .collect();
        flat_run(client, &mut segs, "auto", &lang_c, spoken, &p.audio.translate_style).map_err(|e| format!("translate: {e}"))?;
        for (s, sg) in p.segments.iter_mut().zip(segs) {
            if !sg.tgt.trim().is_empty() {
                s.tgt_text = sg.tgt;
            }
            s.dirty = true; // форсим ре-TTS на новом языке
        }
        // Титры: text -> Lx (позиции/стиль остаются). При сбое перевода НЕ оставляем старый целевой язык:
        // очищаем tgt -> рендер покажет исходный text (лучше, чем титр на старом языке рядом с новыми сегментами).
        if !p.captions.titles.is_empty() {
            let mut tsegs: Vec<Seg> = p.captions.titles.iter().map(|ti| Seg::new(ti.text.clone(), 0)).collect();
            let ok = flat_run(client, &mut tsegs, "auto", &lang_c, false, &p.audio.translate_style).is_ok();
            for (ti, sg) in p.captions.titles.iter_mut().zip(tsegs) {
                ti.tgt = if ok && !sg.tgt.trim().is_empty() { sg.tgt } else { String::new() };
            }
        }
        drop(prov); // освободить VRAM перед TTS/рендером (облако — no-op)
        save_project_atomic(&dst_for_job, &p)?;
        let cb = |ev: Value| progress(ev);
        render::run(&p, &paths, true, &cb)?;
        Ok(json!({ "output": out_res.to_string_lossy(), "project_id": new_pid_res }))
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id, "project_id": new_pid })).into_response()
}

// ─── POST /projects/{pid}/retranslate?lang=Lx&mode=<dub|voiceover|nodub> ─────
/// Смена режима из «Транскрипта» БЕЗ повторного распознавания (реквест Стаса #122). Транскрипт уже дал
/// сегменты (src_text + спикеры + тайминги); переход в дубляж/субтитры/закадр требует лишь ПЕРЕВОДА этих
/// сегментов на tgt (озвучка/сборка — на рендере, как обычно). Раньше switchMode звал полный analyze ->
/// заново гонялись сепарация/диаризация/ASR, хотя текст только что распознан. Здесь — только flat_run-перевод
/// готовых сегментов IN-PLACE (тот же pid, без клона и без рендера) + смена p.mode. Переиспользует ту же
/// машинерию, что export_lang, но не пересобирает видео. Диаризация/ASR НЕ трогаются.
async fn retranslate_project(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let lang = q.get("lang").cloned().unwrap_or_default().trim().to_string();
    if lang.is_empty() {
        return (StatusCode::BAD_REQUEST, "no lang").into_response();
    }
    // Режим-мишень: dub|voiceover|nodub (субтитры). Прочее -> дубляж (дефолт), как маппинг во фронте.
    let mode = match q.get("mode").map(String::as_str) {
        Some("voiceover") => "voiceover",
        Some("nodub") => "nodub",
        _ => "dub",
    }
    .to_string();
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !dir.join("project.json").is_file() {
        return (StatusCode::CONFLICT, "project not analyzed yet").into_response();
    }

    let llama_bin = st.llama_bin.clone();
    let mt_model = st.opts.mt_model_path.clone();
    let models_root_xl = st.models_root.clone();
    let dir_for_job = dir.clone();
    let pid_res = pid.clone();
    let lang_c = lang.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        use dub_translate::{flat_run, Seg};
        let pj = dir_for_job.join("project.json");
        let text = std::fs::read_to_string(&pj).map_err(|e| e.to_string())?;
        let mut p = Project::from_json(&text).map_err(|e| e.to_string())?;
        p.tgt_lang = lang_c.clone();
        p.mode = mode.clone();
        // Закадр/субтитры не переписывают текст «смешно» — сбрасываем rewrite, чтобы derived-режим во фронте
        // не показал «funny» после перехода в dub/nodub/voiceover из транскрипта.
        p.audio.rewrite = None;
        let spoken = matches!(p.mode.as_str(), "dub" | "voiceover");
        progress(json!({ "type": "progress", "stage": "translate",
            "msg": format!("Перевод {} строк → {}", p.segments.len(), lang_c) }));
        let prov = match crate::llm_provider::open(
            &crate::llm_provider::LlmOpen {
                llama_bin: &llama_bin,
                mt_model: &mt_model,
                mmproj: std::path::Path::new(""),
                models_root: &models_root_xl,
            },
            crate::llm_provider::LlmMode::Text,
        ) {
            Ok(p) => p,
            Err(e) => {
                // Если LLM недоступен для перевода, мы ВСЁ РАВНО обновляем p.mode и сохраняем проект,
                // чтобы переход из «Транскрипта» в Дубляж/Закадр/Субтитры происходил успешно.
                p.mode = mode.clone();
                for s in &mut p.segments {
                    if s.tgt_text.trim().is_empty() {
                        s.tgt_text = s.src_text.clone();
                    }
                }
                progress(json!({ "type": "progress", "stage": "translate",
                    "msg": format!("⚠ LLM недоступен ({e}) — режим изменен на {}, текстом оставлен исходный", mode) }));
                save_project_atomic(&dir_for_job, &p)?;
                return Ok(json!({ "project_id": pid_res, "ok": true }));
            }
        };
        let client = prov.client();
        // src_text -> Lx (тайминги/спикеры/раскладка остаются от транскрипта). Вручную добавленные фразы
        // (пустой src_text) переводим из текущего tgt_text (как в export_lang).
        let mut segs: Vec<Seg> = p
            .segments
            .iter()
            .map(|s| {
                let spk = crate::analyze::speaker_to_i64(s.speaker.as_deref());
                let src = if s.src_text.trim().is_empty() { s.tgt_text.clone() } else { s.src_text.clone() };
                Seg::new(src, spk)
            })
            .collect();
        flat_run(client, &mut segs, "auto", &lang_c, spoken, &p.audio.translate_style)
            .map_err(|e| format!("translate: {e}"))?;
        for (s, sg) in p.segments.iter_mut().zip(segs) {
            if !sg.tgt.trim().is_empty() {
                s.tgt_text = sg.tgt;
            }
            s.dirty = true; // новый язык -> ре-TTS при рендере/озвучке
        }
        // Титры: text -> Lx (позиции/стиль остаются). Сбой перевода -> чистим tgt (рендер покажет исходный).
        if !p.captions.titles.is_empty() {
            let mut tsegs: Vec<Seg> = p.captions.titles.iter().map(|ti| Seg::new(ti.text.clone(), 0)).collect();
            let ok = flat_run(client, &mut tsegs, "auto", &lang_c, false, &p.audio.translate_style).is_ok();
            for (ti, sg) in p.captions.titles.iter_mut().zip(tsegs) {
                ti.tgt = if ok && !sg.tgt.trim().is_empty() { sg.tgt } else { String::new() };
            }
        }
        drop(prov);
        save_project_atomic(&dir_for_job, &p)?;
        Ok(json!({ "project_id": pid_res, "ok": true }))
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id, "project_id": pid })).into_response()
}

/// POST /projects/{pid}/dub-audio — сгенерить ТОЛЬКО озвучку (без сборки видео) -> dub_audio.m4a.
/// Фронт вызывает сразу после анализа, чтобы дуб можно было слушать в редакторе (плей играет /dub),
/// не дожидаясь и не собирая финальное видео. Тот же job-паттерн, что и render_project.
async fn dub_audio_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let proj_path = dir.join("project.json");
    if !proj_path.is_file() {
        return (StatusCode::CONFLICT, "project not analyzed yet").into_response();
    }
    let input = match std::fs::read_to_string(dir.join("source.txt")) {
        Ok(s) => PathBuf::from(s.trim()),
        Err(_) => return (StatusCode::CONFLICT, "no source uploaded").into_response(),
    };
    let sel = models::load_selection(&st.models_root);
    let (higgs_model_root, higgs_quant) = models::resolve_tts(&st.models_root, &sel);
    let paths = render::RenderPaths {
        input,
        bench: models::bench_enabled(&st.models_root),
        work_dir: dir.clone(),
        output: dir.join("output.mp4"),
        bsroformer_cli: dub_sep::engine_cli(&st.repo_root, models::stage_backend(&st.models_root, "sep_backend")),
        bsroformer_model: models::resolve_sep(&st.models_root, &sel),
        higgs_dll: st.higgs_dll.clone(),
        higgs_model_root,
        higgs_quant,
        fonts_dir: st.fonts_dir.clone(),
        higgs_backend: "cuda".to_string(),
        higgs_device: 0,
        higgs_threads: st.opts.num_threads,
        max_stretch: st.opts.max_stretch as f64,
        voices_dir: st.voices_dir.clone(),
        asr: models::resolve_asr_choice(&st.repo_root, &st.models_root, &sel),
        ref_secs: models::higgs_ref_secs(&st.models_root),
        models_root: st.models_root.clone(),
        tts_cache: st.tts_cache.clone(),
    };
    let dir_for_job = dir.clone();
    let job: jobs::JobFn = Box::new(move |progress: jobs::ProgressFn| {
        let cb = |ev: Value| progress(ev);
        let text = std::fs::read_to_string(&proj_path).map_err(|e| e.to_string())?;
        let proj = Project::from_json(&text).map_err(|e| e.to_string())?;
        let regen = proj.segments.iter().any(|s| s.dirty);
        let out = render::dub_audio(&proj, &paths, regen, &cb)?;
        // Правки запечены в озвучку (seg_XXX.wav) -> сбросить dirty, как делает render_project. Иначе
        // последующий Экспорт (render видит dirty) РЕ-РОЛЛИТ уже одобренный дубляж — регресс «скидывается».
        if regen {
            reset_baked_dirty(&proj, &proj_path, &dir_for_job);
        }
        Ok(json!({ "audio": out.to_string_lossy() }))
    });
    let job_id = st.jobs.enqueue(job).await;
    Json(json!({ "job_id": job_id })).into_response()
}

// ─── GET /projects/{pid}/output ; /original ; /dub (Range-раздача файла) ─────

/// Найти готовый выходной файл проекта: output.mp4 (обычный/mp4-мультитрек) -> output.mkv (mkv-мультитрек,
/// #113) -> output.wav (аудио-режим). Единый порядок фолбэков для раздачи/сохранения/открытия/превью.
fn find_output(dir: &std::path::Path) -> std::path::PathBuf {
    for name in ["output.mp4", "output.mkv", "output.wav"] {
        let p = dir.join(name);
        if p.is_file() {
            return p;
        }
    }
    dir.join("output.mp4")
}

/// Выход ДЛЯ СОХРАНЕНИЯ юзеру: mkv приоритетнее mp4 (юзер выбрал mkv ради мультитрека; mp4 — лишь
/// playable-компаньон для встроенного плеера, #116). Порядок mkv->mp4->wav.
fn find_output_save(dir: &std::path::Path) -> std::path::PathBuf {
    for name in ["output.mkv", "output.mp4", "output.wav"] {
        let p = dir.join(name);
        if p.is_file() {
            return p;
        }
    }
    dir.join("output.mp4")
}

async fn output(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    Query(q): Query<HashMap<String, String>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    // output.mp4/.mkv (видео; mkv — экспорт с оригинальной дорожкой) или output.wav (аудио-режим).
    let f = find_output(&dir);
    if !f.is_file() {
        return (StatusCode::NOT_FOUND, "not rendered").into_response();
    }
    let dl = q.get("dl").map(|v| v == "1").unwrap_or(false);
    let ext = f.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
    let filename = if dl { Some(format!("{pid}_dub.{ext}")) } else { None };
    serve_file_range(&f, req, filename).await
}

/// POST /pick-folder — нативный диалог выбора папки (rfd). Возвращает {dir} или {dir:null} при отмене.
/// Для batch-экспорта: юзер выбирает ОДНУ папку назначения, дальше save-output кладёт туда все файлы.
async fn pick_folder() -> Json<Value> {
    let dir = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new().set_title("Куда сохранить результаты").pick_folder()
    })
    .await
    .ok()
    .flatten();
    Json(json!({ "dir": dir.map(|d| d.to_string_lossy().into_owned()) }))
}

/// POST /projects/{pid}/save-output {dir, name} — скопировать готовый output проекта в папку `dir` под
/// именем `name` (оригинальное имя файла), сохранив расширение реального выхода. Для batch-вывода:
/// все переведённые файлы в ОДНУ папку с исходными именами.
async fn save_output(State(st): State<AppState>, AxPath(pid): AxPath<String>, Json(body): Json<Value>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let src = find_output_save(&dir); // сохраняем богатый файл: mkv приоритетнее playable-mp4 (#116)
    if !src.is_file() {
        return (StatusCode::NOT_FOUND, "not rendered").into_response();
    }
    let Some(dest_dir) = body.get("dir").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, "no dir").into_response();
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("output");
    // <исходный stem>.<расширение реального выхода> — имя как у оригинала, контейнер как у результата.
    let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let out_ext = src.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
    let base_path = std::path::Path::new(dest_dir);
    let base = if base_path.is_relative() {
        dir.join(base_path)
    } else {
        base_path.to_path_buf()
    };
    if !base.is_dir() {
        if let Err(e) = std::fs::create_dir_all(&base) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("создание каталога {}: {e}", base.display()),
            )
                .into_response();
        }
    }
    let mut dest = base.join(format!("{stem}.{out_ext}"));
    // Коллизия имён: два входа с одинаковым basename (напр. разные папки, оба intro.mp4) при batch
    // «Сохранить все в папку» затёрли бы друг друга → добавляем суффикс (2),(3)… (ревью-находка H).
    let mut n = 2u32;
    while dest.exists() {
        dest = base.join(format!("{stem} ({n}).{out_ext}"));
        n += 1;
    }
    match std::fs::copy(&src, &dest) {
        Ok(_) => (
            [("content-type", "application/json")],
            format!("{{\"ok\":true,\"path\":{:?}}}", dest.to_string_lossy()),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("копирование в {}: {e}", dest.display()),
        )
            .into_response(),
    }
}

/// POST /projects/{pid}/open — открыть готовый output.mp4 в СИСТЕМНОМ плеере (на машине пользователя).
/// Нужно, т.к. в нативном Tauri-webview <a target="_blank"> внешний плеер не открывает.
async fn open_output(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let f = find_output_save(&dir); // системный плеер (VLC и т.п.) тянет богатый mkv, если он есть
    if !f.is_file() {
        return (StatusCode::NOT_FOUND, "not rendered").into_response();
    }
    let path = f.to_string_lossy().to_string();
    let _ = tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            let _ = crate::media::cmd_silent("cmd").args(["/C", "start", "", &path]).spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        }
    })
    .await;
    Json(json!({ "ok": true })).into_response()
}

/// Открыть проводник/файловый менеджер с ВЫДЕЛЕННЫМ файлом (не плеер). Windows: explorer /select.
fn reveal_in_explorer(path: String) {
    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            let path_win = path.replace('/', "\\");
            // explorer /select,"<path>" — выделяет файл в открытом каталоге. Один аргумент.
            let _ = crate::media::cmd_silent("explorer").arg(format!("/select,{path_win}")).spawn();
        }
        #[cfg(not(windows))]
        {
            // прочие ОС: открыть родительский каталог (выделение файла непортабельно).
            let parent = std::path::Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    });
}

/// POST /projects/{pid}/reveal — показать файл (по имени в каталоге проекта) в проводнике с выделением.
/// Body {name}. Для кнопок сохранения/экспорта: пользователь видит, КУДА сохранилось.
async fn reveal_file(State(st): State<AppState>, AxPath(pid): AxPath<String>, Json(body): Json<Value>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("output.mp4");
    // Сначала пробуем прямой путь (поддерживает пробелы, кириллицу и подкаталоги типа Export/)
    let mut f = dir.join(name);
    
    // Проверка безопасности: путь должен быть внутри dir и быть файлом.
    // Если нет, откатываемся на строгую очистку от спецсимволов.
    if !f.is_file() || !f.starts_with(&dir) {
        let safe: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
        f = dir.join(&safe);
        if !f.is_file() && safe.starts_with("output.") {
            f = find_output_save(&dir);
        }
    }
    if !f.is_file() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    reveal_in_explorer(f.to_string_lossy().to_string());
    Json(json!({ "ok": true })).into_response()
}

/// POST /projects/{pid}/save-text — записать текстовый файл (SRT/TXT) в каталог проекта и показать его в
/// проводнике. В нативном Tauri-webview браузерный blob-download (<a download>) не работает — сохраняем
/// через бэкенд. Body {name, text}.
async fn save_text(State(st): State<AppState>, AxPath(pid): AxPath<String>, Json(body): Json<Value>) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("transcript.txt");
    let safe: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
    let safe = if safe.is_empty() { "transcript.txt".to_string() } else { safe };
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let f = dir.join(&safe);
    if let Err(e) = std::fs::write(&f, text) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("write failed: {e}")).into_response();
    }
    reveal_in_explorer(f.to_string_lossy().to_string());
    Json(json!({ "ok": true, "path": f.to_string_lossy() })).into_response()
}

async fn dub_video(
    State(st): State<AppState>,
    AxPath(pid): AxPath<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    let dir = match st.proj_dir(&pid) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    // Что играет <audio>: свежайший дубляж. И output.mp4 (после Экспорта), и dub_audio.m4a (после
    // «перегенерировать озвучку»/regen) несут актуальную дорожку — но regen обновляет ТОЛЬКО dub_audio.m4a,
    // не output.mp4. Прежний жёсткий приоритет output.mp4 играл в превью УСТАРЕВШИЙ дубляж после regen
    // (юзеры: «перегенерировать не работает, новое слышно только после Экспорта») -> берём НОВЕЙШИЙ по mtime.
    let output = find_output(&dir); // output.mp4/.mkv (видео) или output.wav (аудио-режим)
    let dub_audio = dir.join("dub_audio.m4a");
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut f = match (output.is_file(), dub_audio.is_file()) {
        (true, true) => {
            if mtime(&dub_audio) > mtime(&output) { dub_audio } else { output }
        }
        (true, false) => output,
        (false, true) => dub_audio,
        (false, false) => dir.join("analyzed.mp4"),
    };
    if !f.is_file() {
        // nodub / субтитры / транскрипт: озвучки нет — играем ОРИГИНАЛЬНУЮ дорожку видео (source.*),
        // чтобы плей в редакторе работал (переиспользуем аудиодорожку исходника, <audio> берёт её из mp4).
        // Путь — из source.txt (как в analyze/render); fallback — source.* в каталоге.
        if let Ok(p) = std::fs::read_to_string(dir.join("source.txt")) {
            let sp = std::path::PathBuf::from(p.trim());
            if sp.is_file() {
                f = sp;
            }
        }
        if !f.is_file() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.file_stem().and_then(|s| s.to_str()) == Some("source") && p.is_file() {
                        f = p;
                        break;
                    }
                }
            }
        }
    }
    if !f.is_file() {
        return (StatusCode::NOT_FOUND, "no dubbed audio yet").into_response();
    }
    serve_file_range(&f, req, None).await
}

/// Отдать файл с поддержкой Range (для <video> seek). Используем tower-http ServeFile — он
/// корректно обрабатывает Range/If-Range/Content-Range. dl -> Content-Disposition attachment.
async fn serve_file_range(
    path: &Path,
    req: axum::http::Request<axum::body::Body>,
    download_name: Option<String>,
) -> Response {
    use tower::ServiceExt;
    use tower_http::services::ServeFile;
    let svc = ServeFile::new(path);
    match svc.oneshot(req).await {
        Ok(mut resp) => {
            if let Some(name) = download_name {
                if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
                    "attachment; filename=\"{name}\""
                )) {
                    resp.headers_mut().insert(axum::http::header::CONTENT_DISPOSITION, v);
                }
            }
            resp.map(axum::body::Body::new)
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /jobs/{job_id}/events (SSE) ────────────────────────────────────────

async fn job_events(
    State(st): State<AppState>,
    AxPath(job_id): AxPath<String>,
) -> Response {
    let Some((rx, terminal)) = st.jobs.subscribe(&job_id).await else {
        return (StatusCode::NOT_FOUND, "job not found").into_response();
    };
    let jobs = st.jobs.clone();
    let stream = sse_stream(rx, terminal, jobs, job_id);
    Sse::new(stream).into_response()
}

/// Поток SSE-событий джобы. Завершается на событии done/error и реапит терминальную джобу.
fn sse_stream(
    mut rx: tokio::sync::broadcast::Receiver<Value>,
    terminal: Option<Value>,
    jobs: JobQueue,
    job_id: String,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        // Джоба уже завершена к моменту подписки — сразу отдать терминал и выйти.
        if let Some(ev) = terminal {
            yield sse_event(&ev);
            if jobs.is_terminal(&job_id).await {
                jobs.remove(&job_id).await;
            }
            return;
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let is_terminal = matches!(
                        ev.get("type").and_then(|t| t.as_str()),
                        Some("done") | Some("error")
                    );
                    yield sse_event(&ev);
                    if is_terminal {
                        if jobs.is_terminal(&job_id).await {
                            jobs.remove(&job_id).await;
                        }
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break, // канал закрыт
            }
        }
    }
}

fn sse_event(ev: &Value) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(ev).unwrap_or_default()))
}

// ─── POST /projects/{pid}/align — автоподгонка таймингов под оригинальные голоса ─────
pub async fn align_project(State(st): State<AppState>, AxPath(pid): AxPath<String>) -> Response {
    let Ok(d) = st.proj_dir(&pid) else {
        return (StatusCode::NOT_FOUND, "project not found").into_response();
    };
    let mut proj = match st.load_project(&pid) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if proj.segments.is_empty() {
        return Json(proj).into_response();
    }

    let audio_file = ["ref_vocals16.wav", "audio_hq.wav"]
        .iter()
        .map(|f| d.join(f))
        .find(|p| p.is_file())
        .unwrap_or_else(|| d.join("audio_hq.wav"));

    if !audio_file.is_file() {
        if let Ok(src_str) = tokio::fs::read_to_string(d.join("source.txt")).await {
            let src_p = Path::new(src_str.trim());
            if src_p.is_file() {
                let _ = media::to_16k_mono(src_p, &audio_file);
            }
        }
    }

    let res = tokio::task::spawn_blocking(move || {
        let mut count = 0usize;
        let mut wav_path = audio_file;
        let voc16 = d.join("vocals16_align.wav");
        if wav_path.is_file() && media::to_16k_mono(&wav_path, &voc16).is_ok() {
            wav_path = voc16;
        }

        if let Ok((samples, sr)) = wavio::read_mono_f32(&wav_path) {
            let cfg = dub_asr::WindowConfig::default();
            let (env, _) = dub_asr::speech_envelope(&samples, sr, cfg.frame_sec);
            let spans = dub_asr::detect_active_spans(&env, cfg.frame_sec, &cfg);
            if !spans.is_empty() {
                const MAX_DRIFT: f64 = 1.5;
                let mut last_end = 0.0f64;
                for seg in &mut proj.segments {
                    let s_center = (seg.start + seg.end) / 2.0;
                    // Фильтруем спаны строго в локальной окрестности ±1.5с от текущего субтитра
                    let local_spans: Vec<&(f64, f64)> = spans
                        .iter()
                        .filter(|span| (span.0 - seg.start).abs() <= MAX_DRIFT || (span.1 - seg.end).abs() <= MAX_DRIFT || (span.0 <= seg.end && span.1 >= seg.start))
                        .collect();

                    let best = local_spans.iter().max_by(|a, b| {
                        let overlap_a = (a.1.min(seg.end) - a.0.max(seg.start)).max(0.0);
                        let overlap_b = (b.1.min(seg.end) - b.0.max(seg.start)).max(0.0);
                        if (overlap_a - overlap_b).abs() > 0.01 {
                            overlap_a.partial_cmp(&overlap_b).unwrap()
                        } else {
                            let dist_a = ((a.0 + a.1) / 2.0 - s_center).abs();
                            let dist_b = ((b.0 + b.1) / 2.0 - s_center).abs();
                            dist_b.partial_cmp(&dist_a).unwrap()
                        }
                    });

                    if let Some(span) = best {
                        let new_start = (span.0 - 0.05).max(last_end).max(0.0);
                        let new_end = (span.1 + 0.05).max(new_start + 0.1);
                        if (new_start - seg.start).abs() <= MAX_DRIFT && (new_end - seg.end).abs() <= MAX_DRIFT {
                            if (seg.start - new_start).abs() > 0.03 || (seg.end - new_end).abs() > 0.03 {
                                seg.start = (new_start * 100.0).round() / 100.0;
                                seg.end = (new_end * 100.0).round() / 100.0;
                                seg.dirty = true;
                                count += 1;
                            }
                        }
                    }
                    last_end = seg.end;
                }
            }
        }
        let _ = save_project_atomic(&d, &proj);
        (count, proj)
    }).await;

    match res {
        Ok((_count, fresh_proj)) => Json(fresh_proj).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── SPA fallback ───────────────────────────────────────────────────────────

async fn spa_fallback(State(st): State<AppState>, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    spa::serve_spa(st.web_root.as_deref(), path).await
}
