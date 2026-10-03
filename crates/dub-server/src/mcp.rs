//! The studio as an MCP server: Streamable HTTP, stateless JSON-RPC at `/mcp`.
//!
//! Every tool is a route of the studio's own API, called inside the process
//! through the same router the page talks to, so an agent does exactly what
//! the page does through the same code: make a project of a video, analyze it,
//! edit the transcript and the translation line by line, cast the voices,
//! render, export more languages, save the result to a folder. A file an agent
//! names by its path is sent to the route as the multipart upload the page
//! would send.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower::ServiceExt;

mod atomic;
mod window;

pub use window::{window_events, window_focus, window_result};
pub(crate) use window::{carry_job, save_with_revision, track, REV_HEADER};

/// The studio's API router, set once the service has built it.
static API: OnceLock<Router> = OnceLock::new();

/// Takes the API router before the Origin/Host guard is layered: the guard
/// wraps this router together with `/mcp` and `/mcp/status` from outside, so
/// the MCP endpoints are guarded and the tools' own calls do not go through it.
/// The tools' requests still carry `Host: 127.0.0.1`, so they pass the guard
/// should it be put inside this router.
pub fn install(api: Router) {
    let _ = API.set(api);
}

/// The studio's name, as clients show it.
const STUDIO: &str = "Dub Studio";
/// The name clients register the server under.
const SERVER_NAME: &str = "dub-studio";
/// The skill an agent reads, served as a resource and a prompt.
const SKILL: &str = include_str!("../../../docs/mcp-skill.md");
const SKILL_URI: &str = "studio://skill";
const LANGUAGES_URI: &str = "studio://languages";
const EDITS_URI: &str = "studio://patch-ops";
/// Answers longer than this are cut: a transcript of hundreds of lines is more
/// than an agent reads in one call.
const LIMIT: usize = 60_000;

/// What a tool sends to its route.
enum Payload {
    None,
    Json(Value),
    /// Multipart form fields and files, as the page uploads them.
    Form { fields: Vec<(String, String)>, files: Vec<(String, PathBuf, String)> },
    /// A command for the studio's window: what is on screen, the controls, the editor. Answered by
    /// the page itself.
    Window { command: &'static str, args: Value, seconds: u64 },
}

struct Call {
    method: Method,
    path: String,
    payload: Payload,
}

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: fn(&Value) -> Result<Call, String>,
}

fn get(path: String) -> Result<Call, String> {
    Ok(Call { method: Method::GET, path, payload: Payload::None })
}

fn post(path: String, body: Value) -> Result<Call, String> {
    Ok(Call { method: Method::POST, path, payload: Payload::Json(body) })
}

fn send(method: Method, path: String, body: Value) -> Result<Call, String> {
    Ok(Call { method, path, payload: Payload::Json(body) })
}

fn composite(kind: &'static str) -> Result<Call, String> {
    Ok(Call { method: Method::GET, path: format!("composite:{kind}"), payload: Payload::None })
}

/// A GET inside the process, as JSON; a route that refuses says why.
async fn fetch(path: &str) -> Result<Value, String> {
    let (status, text) = call_route(Call { method: Method::GET, path: path.into(), payload: Payload::None }).await?;
    if !status.is_success() {
        return Err(format!("GET {path}: {status} {text}"));
    }
    serde_json::from_str(&text).map_err(|_| format!("GET {path} did not answer JSON: {}", text.chars().take(200).collect::<String>()))
}

/// The app's version: tauri.conf.json is its single source, as for the window.
fn app_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let conf: Value = serde_json::from_str(include_str!("../../../desktop/src-tauri/tauri.conf.json")).expect("tauri.conf.json is JSON");
        conf["version"].as_str().expect("tauri.conf.json names the app's version").to_string()
    })
}

// ---------------------------------------------------------------- jobs

/// A job that ended, one way or the other; any other status is still at work.
const FINISHED: &[&str] = &["done", "error", "failed", "cancelled", "abandoned", "interrupted"];

fn finished(job: &Value) -> bool {
    job["status"].as_str().is_some_and(|status| FINISHED.contains(&status))
}

/// The rows of GET /jobs: its list, bare or under "jobs".
fn job_rows(listed: &Value) -> Result<Vec<Value>, String> {
    listed
        .as_array()
        .or_else(|| listed.get("jobs").and_then(Value::as_array))
        .cloned()
        .ok_or_else(|| format!("GET /jobs answered no list of jobs: {}", listed.to_string().chars().take(200).collect::<String>()))
}

/// A job as an agent follows it: what it is, where it got, what it made. A
/// result that is a whole project is left to project_get.
fn compact_job(job: &Value) -> Value {
    let result = &job["result"];
    let result = if result.get("segments").is_some() {
        json!("the project, updated: project_get reads it")
    } else {
        result.clone()
    };
    let mut row = json!({
        "id": job.get("id").or_else(|| job.get("job_id")).cloned().unwrap_or(Value::Null),
        "kind": job["kind"], "pid": job["pid"], "status": job["status"],
        "stage": job["stage"], "msg": job["msg"], "pct": job["pct"],
    });
    if !job["position"].is_null() {
        row["position"] = job["position"].clone();
    }
    if !result.is_null() {
        row["result"] = result;
    }
    if !job["error"].is_null() {
        row["error"] = job["error"].clone();
    }
    row
}

/// The required models still missing and whether the set is ready.
fn compact_setup(setup: &Value) -> Value {
    let missing: Vec<Value> = setup["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|component| component["installed"] == false && component["requirement"] == "required")
        .map(|component| json!({ "id": component["id"], "name": component["name"], "size": component["size"] }))
        .collect();
    json!({ "ready": setup["ready"], "driver_ok": setup["driverOk"], "download_pending_bytes": setup["downloadPending"], "missing_required": missing })
}

// ---------------------------------------------------------------- projects

/// A line as an agent reads it: without its word timings.
fn compact_segment(segment: &Value) -> Value {
    let mut row = json!({
        "id": segment["id"], "start": segment["start"], "end": segment["end"], "speaker": segment["speaker"],
        "src_text": segment["src_text"], "tgt_text": segment["tgt_text"], "dirty": segment["dirty"],
    });
    for flag in ["hidden", "keep_original"] {
        if segment[flag] == true {
            row[flag] = true.into();
        }
    }
    for said in ["tts_skip", "tts_text"] {
        if segment[said].is_string() {
            row[said] = segment[said].clone();
        }
    }
    if !segment["voice"].is_null() {
        row["voice"] = segment["voice"].clone();
    }
    for computed in ["fit", "takes", "shortened"] {
        if !segment[computed].is_null() {
            row[computed] = segment[computed].clone();
        }
    }
    row
}

/// Titles and blur boxes with the idx their tools take.
fn indexed(items: &Value) -> Value {
    Value::Array(
        items
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(idx, item)| {
                let mut item = item.clone();
                item["idx"] = idx.into();
                item
            })
            .collect(),
    )
}

fn count_dirty(project: &Value) -> usize {
    project["segments"].as_array().into_iter().flatten().filter(|segment| segment["dirty"] == true).count()
}

/// A project without what only the studio reads (word timings, the vision
/// context, stage checksums), its lines narrowed by time or id when asked.
fn compact_project(project: &Value, args: &Value) -> Value {
    let from = args.get("from").and_then(Value::as_f64);
    let to = args.get("to").and_then(Value::as_f64);
    let ids: Vec<&str> = args.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let lines: Vec<Value> = project["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|segment| {
            let start = segment["start"].as_f64().unwrap_or_default();
            let end = segment["end"].as_f64().unwrap_or_default();
            from.is_none_or(|from| end >= from) && to.is_none_or(|to| start <= to) && (ids.is_empty() || ids.contains(&segment["id"].as_str().unwrap_or_default()))
        })
        .map(compact_segment)
        .collect();
    let captions = &project["captions"];
    json!({
        "meta": { "video": project["meta"]["video"], "duration": project["meta"]["duration"], "width": project["meta"]["width"], "height": project["meta"]["height"], "fps": project["meta"]["fps"] },
        "mode": project["mode"], "tgt_lang": project["tgt_lang"], "subs": project["subs"], "audio": project["audio"],
        "blur_on": project["render"]["blur"], "casting_enabled": project["casting_enabled"],
        "captions": {
            "sub_style": captions["sub_style"], "sub_y": captions["sub_y"], "preset": captions["preset"], "overrides": captions["overrides"],
            "titles": indexed(&captions["titles"]), "blur_boxes": indexed(&captions["blur_boxes"]),
        },
        "segments_total": project["segments"].as_array().map_or(0, Vec::len),
        "dirty": count_dirty(project),
        "segments": lines,
    })
}

/// A project's words as a transcript: each line's id, time, speaker and text - the recognised
/// original (text src, the default) or the translation (tgt) - narrowed by from and to seconds,
/// as JSON lines or, with format text, one "[0:14.2 SPK 1] words" line each.
fn transcript(project: &Value, args: &Value) -> Value {
    let from = args.get("from").and_then(Value::as_f64);
    let to = args.get("to").and_then(Value::as_f64);
    let translation = args.get("text").and_then(Value::as_str) == Some("tgt");
    let clock = |seconds: f64| {
        let tenths = (seconds.max(0.0) * 10.0).round() as u64;
        format!("{}:{:02}.{}", tenths / 600, tenths % 600 / 10, tenths % 10)
    };
    // the translation as the subtitles burn it: a line's own subtitle text over its translation,
    // and no line that keeps the original speech
    let own: std::collections::HashMap<&str, &str> = project["captions"]["overrides"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| Some((o["seg_id"].as_str()?, o["text"].as_str()?)))
        .collect();
    let lines: Vec<(&Value, &str)> = project["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|segment| segment["hidden"] != true && !(translation && segment["keep_original"] == true))
        .map(|segment| {
            let text = match translation {
                true => segment["id"].as_str().and_then(|id| own.get(id).copied()).unwrap_or_else(|| segment["tgt_text"].as_str().unwrap_or_default()),
                false => segment["src_text"].as_str().unwrap_or_default(),
            };
            (segment, text.trim())
        })
        .filter(|(_, text)| !text.is_empty())
        .filter(|(segment, _)| {
            let start = segment["start"].as_f64().unwrap_or_default();
            let end = segment["end"].as_f64().unwrap_or_default();
            from.is_none_or(|from| end >= from) && to.is_none_or(|to| start <= to)
        })
        .collect();
    let speakers: std::collections::BTreeSet<&str> = lines.iter().filter_map(|(segment, _)| segment["speaker"].as_str()).collect();
    if args.get("format").and_then(Value::as_str) == Some("text") {
        let text: Vec<String> = lines
            .iter()
            .map(|(segment, text)| format!("[{} SPK {}] {text}", clock(segment["start"].as_f64().unwrap_or_default()), segment["speaker"].as_str().unwrap_or("-")))
            .collect();
        return json!({ "text": text.join("\n"), "lines": lines.len(), "speakers": speakers });
    }
    let rows: Vec<Value> = lines.iter().map(|(segment, text)| json!({ "id": segment["id"], "start": segment["start"], "end": segment["end"], "speaker": segment["speaker"], "text": text })).collect();
    json!({ "language": if translation { project["tgt_lang"].as_str().unwrap_or_default() } else { "original" }, "speakers": speakers, "lines": rows })
}

/// What an edit changed: the project's state in brief and the part the edit
/// touched, instead of the whole project every edit answers with.
fn compact_change(name: &str, args: &Value, project: &Value) -> Value {
    // an edit made by its op answers as the op's own tool does
    let name = match name {
        "project_patch" => args.get("op").and_then(Value::as_str).and_then(tool_of_op).unwrap_or(name),
        _ => name,
    };
    let mut summary = json!({
        "saved": true, "mode": project["mode"], "tgt_lang": project["tgt_lang"],
        "segments": project["segments"].as_array().map_or(0, Vec::len), "dirty": count_dirty(project),
    });
    let mut named: Vec<String> = args.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if let Some(id) = args.get("id").and_then(Value::as_str) {
        named.push(id.to_string());
    }
    let segments = project["segments"].as_array().cloned().unwrap_or_default();
    if name == "segment_split" {
        let after = args.get("id").and_then(Value::as_str).and_then(|id| segments.iter().position(|segment| segment["id"] == id)).and_then(|at| segments.get(at + 1));
        named.extend(after.and_then(|segment| segment["id"].as_str()).map(str::to_string));
    }
    let changed: Vec<Value> = segments.iter().filter(|segment| named.iter().any(|id| segment["id"] == id.as_str())).map(compact_segment).collect();
    if !changed.is_empty() {
        summary["changed"] = Value::Array(changed);
    }
    summary
}

/// Whether an answer is a whole project.
fn is_project(value: &Value) -> bool {
    value.get("segments").is_some_and(Value::is_array) && value.get("captions").is_some()
}

fn hide_proxy_password(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s + 3 => {
            let user = url[s + 3..at].split(':').next().unwrap_or_default();
            if user.is_empty() {
                format!("{}{}", &url[..s + 3], &url[at + 1..])
            } else {
                format!("{}{user}@{}", &url[..s + 3], &url[at + 1..])
            }
        }
        _ => url.to_string(),
    }
}

/// The OpenRouter key never leaves the studio, and a proxy's password is hidden.
fn redact(value: Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut clean = serde_json::Map::new();
            for (key, value) in fields {
                match key.as_str() {
                    "or_key" => {
                        let set = value.as_str().is_some_and(|key| !key.trim().is_empty());
                        clean.insert("or_key_set".into(), set.into());
                    }
                    "proxy_url" => {
                        let shown = value.as_str().map(|url| Value::String(hide_proxy_password(url))).unwrap_or(value);
                        clean.insert(key, shown);
                    }
                    _ => {
                        clean.insert(key, redact(value));
                    }
                }
            }
            Value::Object(clean)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(redact).collect()),
        other => other,
    }
}

/// What an agent is answered: the parts it acts on, unless it asked for
/// every field with response_format detailed.
fn shape(name: &str, args: &Value, value: Value) -> Value {
    let detailed = args.get("response_format").and_then(Value::as_str) == Some("detailed");
    match name {
        _ if detailed => value,
        "project_get" => compact_project(&value, args),
        "project_transcript" => transcript(&value, args),
        _ if is_project(&value) => compact_change(name, args, &value),
        "projects_list" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or_default().to_lowercase();
            let bound = |name: &str, end: bool| match args.get(name).and_then(Value::as_str) {
                Some(text) => moment(text, end).map(Some),
                None => Ok(None),
            };
            let (since, until) = match (bound("since", false), bound("until", true)) {
                (Ok(since), Ok(until)) => (since, until),
                (Err(problem), _) | (_, Err(problem)) => return json!({ "error": problem }),
            };
            let projects = value["projects"].as_array().cloned().unwrap_or_default().into_iter().filter(|project| {
                let edited = project["mtime"].as_i64().unwrap_or_default();
                (query.is_empty() || project["video"].as_str().unwrap_or_default().to_lowercase().contains(&query))
                    && since.is_none_or(|since| edited >= since)
                    && until.is_none_or(|until| edited <= until)
            });
            Value::Array(projects.collect())
        }
        _ => value,
    }
}

// ---------------------------------------------------------------- waiting

async fn status_summary() -> Value {
    let (jobs, setup) = tokio::join!(fetch("/jobs"), fetch("/setup/status"));
    let mut summary = json!({});
    match jobs.and_then(|jobs| job_rows(&jobs)) {
        Ok(rows) => {
            let (done, working): (Vec<Value>, Vec<Value>) = rows.into_iter().partition(finished);
            summary["jobs"] = Value::Array(working.iter().map(compact_job).collect());
            summary["finished_jobs"] = Value::Array(done.iter().take(5).map(compact_job).collect());
        }
        Err(problem) => summary["jobs_error"] = problem.into(),
    }
    match setup {
        Ok(setup) => {
            if let Some(download) = download_row(&setup) {
                if let Some(jobs) = summary["jobs"].as_array_mut() {
                    jobs.push(download);
                }
            }
            summary["models"] = compact_setup(&setup);
        }
        Err(problem) => summary["models_error"] = problem.into(),
    }
    summary
}

/// The models download runs beside the job queue: while it goes it is shown and waited for as work of
/// kind download.
fn download_row(setup: &Value) -> Option<Value> {
    let active = setup.get("active")?;
    if active["status"] != "downloading" {
        return None;
    }
    let (done, total) = (active["downloaded"].as_f64().unwrap_or(0.0), active["total"].as_f64().unwrap_or(0.0));
    Some(json!({
        "id": active["id"], "kind": "download", "status": "running", "stage": active["phase"],
        "pct": if total > 0.0 { Value::from((done / total * 100.0).round()) } else { Value::Null },
        "components": active["ids"], "waiting_seconds": active["waitingS"],
    }))
}

/// The kinds of work a status summary still has running or waiting.
fn busy(summary: &Value) -> Result<Vec<String>, String> {
    if let Some(problem) = summary.get("jobs_error").and_then(Value::as_str) {
        return Err(format!("The studio's jobs cannot be read: {problem}"));
    }
    Ok(summary["jobs"].as_array().into_iter().flatten().map(|job| job["kind"].as_str().unwrap_or("unknown").to_string()).collect())
}

/// How long one wait holds a call: clients give up on a tool call after about
/// a minute, so a wait answers before that and the agent calls it again.
const WAIT_DEFAULT: u64 = 30;
const WAIT_LONGEST: u64 = 55;

/// Waits for a job or for one kind of work or everything, a slice at a time.
async fn wait_for(args: &Value) -> Result<Value, String> {
    let seconds = args.get("seconds").and_then(Value::as_u64).unwrap_or(WAIT_DEFAULT).clamp(2, WAIT_LONGEST);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let job = args.get("job_id").and_then(Value::as_str).map(str::to_string);
    let until = args.get("until").and_then(Value::as_str).unwrap_or("idle").to_string();
    loop {
        let (done, now) = match &job {
            Some(job) => {
                let state = fetch(&format!("/jobs/{}", segment(job)))
                    .await
                    .map_err(|why| format!("No job {job} ({why}): job_id is what project_analyze, project_dub_audio, project_render, project_export_lang or a one-call tool returned. Wait for other work with until."))?;
                if state.get("status").and_then(Value::as_str).is_none() {
                    return Err(format!("The job {job} has no status: {state}"));
                }
                (finished(&state), compact_job(&state))
            }
            None => {
                let summary = status_summary().await;
                let working = busy(&summary)?;
                let done = if until == "idle" { working.is_empty() } else { !working.contains(&until) };
                (done, summary)
            }
        };
        if done {
            return Ok(json!({ "done": true, "state": now }));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({ "done": false, "note": "still running; call studio_wait again to keep waiting", "state": now }));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

const QUEUED_BEHIND_JOBS: &[&str] = &[];

async fn graphics_card_free() -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------- annotations

/// MCP tool annotations, from what each tool does.
fn annotations(name: &str) -> Value {
    const READS: &[&str] = &["_get", "_status", "_list", "_transcript"];
    const CHANGES: &[&str] = &[
        "create", "update", "split", "merge", "regen", "render", "dub_audio", "export_lang", "analyze",
        "open", "seek", "select", "play", "pause", "undo", "redo", "notify", "click", "type", "press_key", "scroll",
    ];
    const READ_NAMES: &[&str] = &["studio_wait", "project_transcript", "ui_screenshot", "ui_read_page"];
    const OVERWRITES: &[&str] = &[
        "project_analyze", "segment_update", "segments_regen_all", "segments_merge",
    ];
    let changes = CHANGES.iter().any(|verb| name.split('_').any(|word| word == *verb));
    let read_only = READ_NAMES.contains(&name) || !changes && READS.iter().any(|part| name.ends_with(part) || name.contains(&format!("{part}_")));
    let destructive = OVERWRITES.contains(&name);
    let title = name.replace('_', " ");
    let idempotent = read_only || name.ends_with("_file");
    json!({ "title": title, "readOnlyHint": read_only, "destructiveHint": destructive, "idempotentHint": idempotent, "openWorldHint": false })
}

// ---------------------------------------------------------------- origin and agent

struct Agent {
    /// When an agent last called the server, and what it called.
    last_call: Mutex<Option<(std::time::Instant, String)>>,
    calls: AtomicU64,
}

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| Agent { last_call: Mutex::new(None), calls: AtomicU64::new(0) })
}

/// An agent that called within this long still counts as connected.
const AGENT_PRESENT: Duration = Duration::from_secs(600);

fn seen(what: &str) {
    *agent().last_call.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((std::time::Instant::now(), what.to_string()));
    agent().calls.fetch_add(1, Ordering::Relaxed);
}

fn agent_present() -> bool {
    agent().last_call.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().is_some_and(|(at, _)| at.elapsed() < AGENT_PRESENT)
}

pub fn is_disabled() -> bool {
    let temp = std::env::var("TEMP").unwrap_or_else(|_| ".".into());
    let temp_flag = std::path::Path::new(&temp).join("dubstudio.mcp_disabled");
    if temp_flag.is_file() {
        if let Ok(s) = std::fs::read_to_string(&temp_flag) {
            if s.trim() == "1" { return true; }
            if s.trim() == "0" { return false; }
        }
    }
    for cand in &["workspace/.mcp_disabled", ".mcp_disabled"] {
        let p = std::path::Path::new(cand);
        if p.is_file() {
            if let Ok(s) = std::fs::read_to_string(p) {
                if s.trim() == "1" { return true; }
                if s.trim() == "0" { return false; }
            }
        }
    }
    false
}

/// Whether an agent is connected, for the settings page.
pub async fn status() -> Json<Value> {
    let last = agent().last_call.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    Json(json!({
        "server_name": SERVER_NAME,
        "enabled": !is_disabled(),
        "agent_connected": agent_present(),
        "agent_last_call": last.as_ref().map(|(_, what)| what.clone()),
        "agent_seconds_ago": last.as_ref().map(|(at, _)| at.elapsed().as_secs()),
        "agent_calls": agent().calls.load(Ordering::Relaxed),
        "window_open": window::windows_open() > 0,
    }))
}

// ---------------------------------------------------------------- arguments

/// A required text argument, or a message saying which is missing.
fn text(args: &Value, name: &str) -> Result<String, String> {
    args.get(name).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string).ok_or_else(|| format!("'{name}' is required"))
}

/// A path segment, escaped.
pub(crate) fn segment(value: &str) -> String {
    value.bytes().map(|byte| if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) { (byte as char).to_string() } else { format!("%{byte:02X}") }).collect()
}

/// The arguments without the ones that went into the path.
fn body_without(args: &Value, taken: &[&str]) -> Value {
    let mut body = args.as_object().cloned().unwrap_or_default();
    for name in taken {
        body.remove(*name);
    }
    Value::Object(body)
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn id_only(name: &str, what: &str) -> Value {
    object(json!({ name: { "type": "string", "description": what } }), &[name])
}

fn nothing() -> Value {
    json!({ "type": "object", "additionalProperties": false })
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into())
}

/// The project a tool acts on.
fn pid() -> Value {
    json!({ "type": "string", "description": "project id (projects_list, project_create)" })
}

fn project_only() -> Value {
    object(json!({ "pid": pid() }), &["pid"])
}

/// The route of a project, and what follows it.
fn project_path(args: &Value, rest: &str) -> Result<String, String> {
    Ok(format!("/projects/{}{rest}", segment(&text(args, "pid")?)))
}

/// A query string of the arguments given, in this order.
fn query(pairs: &[(&str, Option<String>)]) -> String {
    let given: Vec<String> = pairs.iter().filter_map(|(name, value)| value.as_ref().map(|value| format!("{name}={}", segment(value)))).collect();
    if given.is_empty() { String::new() } else { format!("?{}", given.join("&")) }
}

/// An argument as a query value: text as it is, a number, or a switch as 1 or 0.
fn given(args: &Value, name: &str) -> Option<String> {
    match args.get(name)? {
        Value::String(text) => Some(text.clone()),
        Value::Bool(on) => Some((if *on { "1" } else { "0" }).into()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// An edit of a project: PATCH /projects/{pid} with the op and its fields.
fn edit(args: &Value, op: &str) -> Result<Call, String> {
    let path = project_path(args, "")?;
    let mut body = body_without(args, &["pid", "response_format"]);
    body["op"] = op.into();
    send(Method::PATCH, path, body)
}

/// The detailed answer an edit can give instead of its summary.
fn detail() -> Value {
    json!({ "type": "string", "enum": ["concise", "detailed"], "description": "detailed: the whole project instead of what changed" })
}

/// Records of a glossary, as glossary_get answers them.
fn ids(what: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": what })
}

/// Each edit of a project and the tool that makes it; the resource
/// studio://patch-ops lists them with their fields.
const PATCH_OPS: &[(&str, &str)] = &[
    ("segment", "segment_update"),
    ("split_segment", "segment_split"),
    ("merge_segments", "segments_merge"),
    ("regen", "segment_regen"),
    ("regen_all", "segments_regen_all"),
];

/// Ops for one thing that a tool of many already makes, and the op it makes.
const PATCH_ALIASES: &[(&str, &str)] = &[
    ("regen_multi", "regen"),
    ("clear_regen", "regen_all"),
];

/// The tool that makes an op, or makes what an alias does.
fn tool_of_op(op: &str) -> Option<&'static str> {
    let op = PATCH_ALIASES.iter().find(|(alias, _)| *alias == op).map_or(op, |(_, same)| *same);
    PATCH_OPS.iter().find(|(known, _)| *known == op).map(|(_, tool)| *tool)
}

#[cfg(test)]
fn every_op() -> Vec<&'static str> {
    PATCH_OPS.iter().map(|(op, _)| *op).chain(PATCH_ALIASES.iter().map(|(op, _)| *op)).collect()
}

/// Every edit of a project: its op, its tool and the tool's fields.
fn edits() -> Value {
    let mut ops: Vec<Value> = PATCH_OPS
        .iter()
        .map(|(op, tool)| {
            let mut fields = tools().iter().find(|entry| entry.name == *tool).map(|entry| (entry.schema)()["properties"].clone()).unwrap_or(Value::Null);
            if let Some(fields) = fields.as_object_mut() {
                fields.remove("pid");
                fields.remove("response_format");
            }
            json!({ "op": op, "tool": tool, "fields": fields })
        })
        .collect();
    ops.extend(PATCH_ALIASES.iter().map(|(op, same)| json!({ "op": op, "same_as": same })));
    json!({ "route": "PATCH /projects/{pid}", "body": "{ op, ...fields }", "answer": "the whole project, saved", "ops": ops })
}

/// The languages a project is dubbed from and into.
fn languages() -> Value {
    Value::Array(dub_translate::WHISPER_LANGS.iter().map(|(code, name)| json!({ "code": code, "name": name })).collect())
}

/// A resource by its uri: its type and its text.
fn resource(uri: &str) -> Option<(&'static str, String)> {
    match uri {
        SKILL_URI => Some(("text/markdown", SKILL.to_string())),
        LANGUAGES_URI => Some(("application/json", serde_json::to_string_pretty(&languages()).unwrap_or_default())),
        EDITS_URI => Some(("application/json", serde_json::to_string_pretty(&edits()).unwrap_or_default())),
        _ => None,
    }
}

// ---------------------------------------------------------------- tools

fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let mut all = vec![
            Tool {
                name: "studio_status",
                description: "What the studio is doing now, in one short summary: the jobs running or waiting (analysis, voicing, render, another language, downloads) with their project, stage and percent, the last finished ones with their result or error, and whether the required models are there. Call it first, and use studio_wait to wait.",
                schema: nothing,
                call: |_| composite("status"),
            },
            Tool {
                name: "studio_wait",
                description: "Wait for work to finish instead of polling: a job (job_id, as project_analyze, project_dub_audio, project_render, project_export_lang or a one-call tool still at work returned it), or until one kind of work is over - analyze, dub_audio, render, export_lang, download - or everything (until: idle, the default). Returns when it is done or after seconds (30 by default, at most 55, under the minute clients allow a call) with how far it got; call it again to keep waiting.",
                schema: || object(json!({ "job_id": { "type": "string" }, "until": { "type": "string", "enum": ["idle", "analyze", "dub_audio", "render", "export_lang", "download"] }, "seconds": { "type": "integer" } }), &[]),
                call: |_| composite("wait"),
            },
            Tool {
                name: "projects_list",
                description: "The projects, the last edited first: pid, the video's name, target language, mode, size, length, number of lines, whether it is rendered (done), and source (agent: made of a file by a tool, window: by the studio's window). query matches the video's name; since and until bound when it was last edited.",
                schema: || object(json!({ "query": { "type": "string" }, "since": { "type": "string" }, "until": { "type": "string" } }), &[]),
                call: |_| get("/projects".into()),
            },
            Tool {
                name: "project_create",
                description: "Make a project of a video or audio file on this computer (path): the studio copies it into its workspace. subtitles_path adds an .srt, .ass or .ssa file: the analysis then takes its text and timing instead of recognising speech. Answers the project_id; project_analyze is next.",
                schema: || object(json!({ "path": { "type": "string", "description": "video or audio file" }, "subtitles_path": { "type": "string", "description": ".srt, .ass or .ssa file" } }), &["path"]),
                call: |args| {
                    let video = PathBuf::from(text(args, "path")?);
                    if !video.is_file() {
                        return Err(format!("{} is not a file on this computer.", video.display()));
                    }
                    let name = file_name(&video);
                    let mut files = vec![("file".to_string(), video, name)];
                    if let Some(subtitles) = args.get("subtitles_path").and_then(Value::as_str).filter(|path| !path.trim().is_empty()) {
                        let subtitles = PathBuf::from(subtitles.trim());
                        let kind = subtitles.extension().and_then(|value| value.to_str()).unwrap_or_default().to_lowercase();
                        if !["srt", "ass", "ssa"].contains(&kind.as_str()) {
                            return Err(format!("{} is not .srt, .ass or .ssa: the studio tells subtitles from the video by that extension.", subtitles.display()));
                        }
                        if !subtitles.is_file() {
                            return Err(format!("{} is not a file on this computer.", subtitles.display()));
                        }
                        let name = file_name(&subtitles);
                        files.push(("subs".to_string(), subtitles, name));
                    }
                    Ok(Call { method: Method::POST, path: "/projects".into(), payload: Payload::Form { fields: Vec::new(), files } })
                },
            },
            Tool {
                name: "project_get",
                description: "A project: mode, target language, subtitles, audio and voices, subtitle style, titles and blur boxes, how many lines there are and how many are changed (dirty, voiced again at the next project_dub_audio or project_render), and its lines - id, start, end, speaker, the recognised text (src_text), the translation (tgt_text), dirty, hidden, keep_original. from and to (seconds) or ids narrow the lines. response_format detailed returns the whole project as stored.",
                schema: || object(json!({ "pid": pid(), "response_format": { "type": "string", "enum": ["concise", "detailed"] }, "from": { "type": "number" }, "to": { "type": "number" }, "ids": ids("line ids") }), &["pid"]),
                call: |args| get(project_path(args, "")?),
            },
            Tool {
                name: "project_transcript",
                description: "A project's transcript without the window: each line's id, start, end, speaker and text - the recognised original (text src, the default) or the translation as the subtitles show it (text tgt: a line's own subtitle text over its translation, lines that keep the original speech left out) - hidden lines left out, from and to (seconds) narrowing it; format text gives one \"[0:14.2 SPK 1] words\" line each instead of JSON lines.",
                schema: || object(json!({ "pid": pid(), "text": { "type": "string", "enum": ["src", "tgt"] }, "format": { "type": "string", "enum": ["json", "text"] }, "from": { "type": "number" }, "to": { "type": "number" } }), &["pid"]),
                call: |args| get(project_path(args, "")?),
            },
            Tool {
                name: "project_analyze",
                description: "Analyze a project's video: separate the voices from the background, find who speaks, recognise the speech, translate it into tgt_lang, read the on-screen text (detect) and style the subtitles after the original. A job: studio_wait with its job_id, then project_get shows the lines. mode: auto (dub when there is speech), dub, voiceover (the translation over the quieted original), nodub (subtitles only), transcribe (the transcript in the original language). src_lang: auto or a code (studio://languages). subs: auto, none, transcribe, translate, bilingual (the translation with the original line beside it). burn: burn the subtitles into the video (default on). rewrite: an instruction for a funny or themed version of the dub. translate_style: the tone of the translation. casting: find the characters by voice and face (content_type real or anime, auto guesses). import_translated: the subtitles given to project_create are already in tgt_lang. Analyzing again replaces the project's lines.",
                schema: || {
                    object(
                        json!({
                            "pid": pid(),
                            "tgt_lang": { "type": "string", "description": "language code to dub into (studio://languages); not needed for mode transcribe" },
                            "mode": { "type": "string", "enum": ["auto", "dub", "voiceover", "nodub", "transcribe"] },
                            "src_lang": { "type": "string" },
                            "subs": { "type": "string", "enum": ["auto", "none", "transcribe", "translate", "bilingual"] },
                            "burn": { "type": "boolean" },
                            "detect": { "type": "boolean", "description": "read on-screen text to blur and translate it (default on)" },
                            "rewrite": { "type": "string" },
                            "translate_style": { "type": "string" },
                            "casting": { "type": "boolean" },
                            "content_type": { "type": "string", "enum": ["auto", "real", "anime"] },
                            "import_translated": { "type": "boolean" },
                            "align_subs": { "type": "boolean", "description": "subtitles given to project_create in the original language: align their timing to the recognised speech" },
                        }),
                        &["pid"],
                    )
                },
                call: |args| {
                    let path = project_path(args, "/analyze")?;
                    let tgt_lang = if given(args, "mode").as_deref() == Some("transcribe") { given(args, "tgt_lang") } else { Some(text(args, "tgt_lang")?) };
                    let tail = query(&[
                        ("tgt_lang", tgt_lang),
                        ("mode", given(args, "mode")),
                        ("src_lang", given(args, "src_lang")),
                        ("subs", given(args, "subs")),
                        ("rewrite", given(args, "rewrite")),
                        ("translate_style", given(args, "translate_style")),
                        ("burn", given(args, "burn")),
                        ("detect", given(args, "detect")),
                        ("casting", given(args, "casting")),
                        ("content_type", given(args, "content_type")),
                        ("import_translated", given(args, "import_translated")),
                        ("align_subs", given(args, "align_subs")),
                    ]);
                    post(format!("{path}{tail}"), json!({}))
                },
            },
            Tool {
                name: "project_dub_audio",
                description: "Voice the dub without making the video: the dirty lines (all of them the first time) are synthesized with each speaker's voice and mixed into dub_audio.m4a. A job: studio_wait with its job_id. Quicker than a render to check the voices.",
                schema: project_only,
                call: |args| post(project_path(args, "/dub-audio")?, json!({})),
            },
            Tool {
                name: "project_render",
                description: "Make the finished video: voice the dirty lines, mix the dub over the background, burn in the subtitles and titles, blur, and write output.mp4 (output.mkv with the original track kept, output.wav for audio). A job: studio_wait with its job_id.",
                schema: project_only,
                call: |args| post(project_path(args, "/render")?, json!({})),
            },
            Tool {
                name: "project_export_lang",
                description: "The same video in another language: a copy of the project keeps its layout, subtitle style, titles, blur and cloned voices, its lines and titles are translated into lang, and it is rendered. Answers the new project_id and the job_id; studio_wait with the job_id. One call per language, one after another.",
                schema: || object(json!({ "pid": pid(), "lang": { "type": "string" } }), &["pid", "lang"]),
                call: |args| {
                    let path = project_path(args, "/export-lang")?;
                    post(format!("{path}{}", query(&[("lang", Some(text(args, "lang")?))])), json!({}))
                },
            },
            Tool {
                name: "segment_update",
                description: "Edit one line (id from project_get): its translation (tgt_text), its recognised text (src_text), its start and end in seconds, its speaker (another speaker's id, or a new one for a new voice; \"\" takes it away), hidden (neither voiced nor subtitled), keep_original (the original voice plays there). The line becomes dirty: project_dub_audio or project_render voices it again.",
                schema: || {
                    object(
                        json!({
                            "pid": pid(), "id": { "type": "string" }, "tgt_text": { "type": "string" }, "src_text": { "type": "string" },
                            "start": { "type": "number" }, "end": { "type": "number" }, "speaker": { "type": "string" },
                            "hidden": { "type": "boolean" }, "keep_original": { "type": "boolean" }, "response_format": detail(),
                        }),
                        &["pid", "id"],
                    )
                },
                call: |args| {
                    text(args, "id")?;
                    edit(args, "segment")
                },
            },
            Tool {
                name: "segment_regen",
                description: "Voice one line again at the next project_dub_audio or project_render, and only that one: every other line is marked as voiced.",
                schema: || object(json!({ "pid": pid(), "id": { "type": "string" }, "response_format": detail() }), &["pid", "id"]),
                call: |args| {
                    text(args, "id")?;
                    edit(args, "regen")
                },
            },
            Tool {
                name: "segments_regen_all",
                description: "Voice every line again at the next project_dub_audio or project_render.",
                schema: || object(json!({ "pid": pid(), "response_format": detail() }), &["pid"]),
                call: |args| edit(args, "regen_all"),
            },
            Tool {
                name: "segment_split",
                description: "Cut a line in two at a moment (at, seconds, inside the line): the recognised words and the text are divided there, tgt_text and tgt_text_2 give the two halves' translations (the translation is divided in the same share when left out). The second half gets new_id or <id>_2. Both halves are dirty; the answer shows both.",
                schema: || object(json!({ "pid": pid(), "id": { "type": "string" }, "at": { "type": "number", "description": "seconds" }, "tgt_text": { "type": "string" }, "tgt_text_2": { "type": "string" }, "new_id": { "type": "string" }, "response_format": detail() }), &["pid", "id", "at"]),
                call: |args| {
                    text(args, "id")?;
                    edit(args, "split_segment")
                },
            },
            Tool {
                name: "segments_merge",
                description: "Join lines that follow one another in the list into one (ids): the first keeps its id, speaker and voice, it runs from the earliest start to the latest end, the texts follow each other. It is dirty.",
                schema: || object(json!({ "pid": pid(), "ids": ids("line ids, neighbours in the list"), "response_format": detail() }), &["pid", "ids"]),
                call: |args| edit(args, "merge_segments"),
            },
        ];
        all.extend(atomic::tools());
        all.extend(window::tools());
        all
    })
}

// ---------------------------------------------------------------- calling a route

/// The router the tools call; the tests' own stands in for it, whichever
/// router another test of this crate installs.
fn studio_api() -> Result<Router, String> {
    #[cfg(test)]
    if let Some(stub) = tests::STUB.get() {
        return Ok(stub.clone());
    }
    API.get().cloned().ok_or_else(|| "the studio is still starting".to_string())
}

/// A route's answer as it came.
struct Reply {
    status: StatusCode,
    mime: String,
    bytes: axum::body::Bytes,
}

/// Builds the route's request, calls it inside the process and returns what it
/// answered. A page instead of an answer means the route is not there.
async fn call_route_raw(call: Call) -> Result<Reply, String> {
    let api = studio_api()?;
    let asked = format!("{} {}", call.method, call.path);
    let builder = Request::builder().method(call.method).uri(&call.path).header(header::HOST, "127.0.0.1").header(window::AGENT_HEADER, "1");
    let request = match call.payload {
        Payload::None => builder.body(Body::empty()),
        Payload::Window { command, .. } => return Err(format!("{command} is a command of the studio's window, not a route")),
        Payload::Json(body) => builder.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
        Payload::Form { fields, files } => {
            let boundary = format!("studio-mcp-{}", uuid::Uuid::new_v4().simple());
            let mut parts: Vec<Part> = Vec::new();
            for (name, value) in fields {
                parts.push(Part::Text(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")));
            }
            for (name, path, file) in files {
                if let Err(error) = tokio::fs::metadata(&path).await {
                    return Err(format!("read {}: {error}", path.display()));
                }
                parts.push(Part::Text(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n", file.replace('"', "'"))));
                parts.push(Part::File(path));
                parts.push(Part::Text("\r\n".into()));
            }
            parts.push(Part::Text(format!("--{boundary}--\r\n")));
            builder.header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}")).body(Body::from_stream(streamed(parts)))
        }
    }
    .map_err(|error| error.to_string())?;
    let response = api.oneshot(request).await.map_err(|error| error.to_string())?;
    let status = response.status();
    let mime = response.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default().to_string();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024).await.map_err(|error| error.to_string())?;
    if mime.starts_with("text/html") {
        return Err(format!("The studio has no route {asked}: its page answered instead."));
    }
    Ok(Reply { status, mime, bytes })
}

/// A route's answer as text, cut to a size an agent reads.
fn reply_text(reply: &Reply) -> String {
    let mut text = match serde_json::from_slice::<Value>(&reply.bytes) {
        Ok(value) => serde_json::to_string(&value).unwrap_or_default(),
        Err(_) => String::from_utf8_lossy(&reply.bytes).into_owned(),
    };
    if text.trim().is_empty() {
        text = if reply.status.is_success() { "Done.".into() } else { reply.status.to_string() };
    }
    text
}

/// Calls a route inside the process and returns what it answered.
async fn call_route(call: Call) -> Result<(StatusCode, String), String> {
    let reply = call_route_raw(call).await?;
    Ok((reply.status, reply_text(&reply)))
}

/// A piece of a multipart body: text, or a file read as it is sent.
enum Part {
    Text(String),
    File(PathBuf),
}

/// The parts as a stream, a file a megabyte at a time: a long video is sent
/// without ever being held in memory whole.
fn streamed(parts: Vec<Part>) -> impl futures_util::Stream<Item = std::io::Result<axum::body::Bytes>> {
    futures_util::stream::unfold((parts.into_iter(), None::<tokio::fs::File>), |(mut parts, mut open)| async move {
        loop {
            if let Some(file) = open.as_mut() {
                let mut chunk = vec![0u8; 1 << 20];
                match tokio::io::AsyncReadExt::read(file, &mut chunk).await {
                    Ok(0) => open = None,
                    Ok(read) => {
                        chunk.truncate(read);
                        return Some((Ok(chunk.into()), (parts, open)));
                    }
                    Err(error) => return Some((Err(error), (parts, None))),
                }
                continue;
            }
            match parts.next()? {
                Part::Text(text) => return Some((Ok(text.into()), (parts, None))),
                Part::File(path) => match tokio::fs::File::open(&path).await {
                    Ok(file) => open = Some(file),
                    Err(error) => return Some((Err(error), (parts, None))),
                },
            }
        }
    })
}

/// Cuts an answer to what an agent reads in one go, and says how to get the rest.
fn cut(mut text: String) -> String {
    if text.len() > LIMIT {
        let mut end = LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n... (cut: ask for a part, e.g. project_get with from and to seconds or ids)");
    }
    text
}

// ---------------------------------------------------------------- the protocol

/// The protocol revisions the studio speaks: the stateless one, where every
/// request carries its version, and the handshake ones older clients open
/// with `initialize`.
const MODERN: &[&str] = &["2026-07-28"];
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// How long a client may keep the tool list, the prompts and the resources:
/// they change only with the studio, but an update should show within minutes.
const LIST_TTL_MS: u64 = 300_000;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

fn server_info() -> Value {
    json!({ "name": SERVER_NAME, "title": STUDIO, "version": app_version() })
}

fn supported_versions() -> Vec<&'static str> {
    MODERN.iter().chain(LEGACY).copied().collect()
}

/// A complete result, signed with the server's identity.
fn rpc(id: Value, mut result: Value) -> Response {
    if let Some(fields) = result.as_object_mut() {
        fields.entry("resultType").or_insert_with(|| "complete".into());
        let meta = fields.entry("_meta").or_insert_with(|| json!({}));
        meta["io.modelcontextprotocol/serverInfo"] = server_info();
    }
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i64, message: String) -> Response {
    rpc_failure(StatusCode::OK, id, code, message, None)
}

fn rpc_failure(status: StatusCode, id: Value, code: i64, message: String, data: Option<Value>) -> Response {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    (status, Json(json!({ "jsonrpc": "2.0", "id": id, "error": error }))).into_response()
}

/// A list or a read a client may cache: the same for everyone, fresh for five minutes.
fn cacheable(mut result: Value) -> Value {
    result["ttlMs"] = LIST_TTL_MS.into();
    result["cacheScope"] = "public".into();
    result
}

/// A header value, with the Base64 sentinel form decoded.
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    use base64::Engine;
    let raw = headers.get(name)?.to_str().ok()?;
    match raw.strip_prefix("=?base64?").and_then(|inner| inner.strip_suffix("?=")) {
        Some(encoded) => base64::engine::general_purpose::STANDARD.decode(encoded).ok().and_then(|bytes| String::from_utf8(bytes).ok()),
        None => Some(raw.to_string()),
    }
}

/// Why a stateless request is refused, if it is: a version the studio does
/// not speak, or headers that do not say what its body says.
fn refused(headers: &HeaderMap, method: &str, params: &Value, version: &str) -> Option<(i64, String, Option<Value>)> {
    if !MODERN.contains(&version) {
        return Some((UNSUPPORTED_VERSION, "Unsupported protocol version".into(), Some(json!({ "supported": supported_versions(), "requested": version }))));
    }
    let mismatch = |what: String| Some((HEADER_MISMATCH, format!("Header mismatch: {what}"), None));
    match header_value(headers, "mcp-protocol-version") {
        Some(value) if value == version => {}
        Some(value) => return mismatch(format!("MCP-Protocol-Version header value '{value}' does not match body value '{version}'")),
        None => return mismatch("the MCP-Protocol-Version header is missing".into()),
    }
    match header_value(headers, "mcp-method") {
        Some(value) if value == method => {}
        Some(value) => return mismatch(format!("Mcp-Method header value '{value}' does not match body value '{method}'")),
        None => return mismatch("the Mcp-Method header is missing".into()),
    }
    let named = match method {
        "tools/call" | "prompts/get" => params.get("name"),
        "resources/read" => params.get("uri"),
        _ => return None,
    }
    .and_then(Value::as_str)
    .unwrap_or_default();
    match header_value(headers, "mcp-name") {
        Some(value) if value == named => None,
        Some(value) => mismatch(format!("Mcp-Name header value '{value}' does not match body value '{named}'")),
        None => mismatch("the Mcp-Name header is missing".into()),
    }
}

/// A tool's answer: the text an agent reads, and the same data structured
/// when it is JSON and whole.
fn tool_result(id: Value, text: String, structured: Option<Value>, error: bool) -> Response {
    let mut result = json!({ "content": [{ "type": "text", "text": text }], "isError": error });
    if let Some(structured) = structured.filter(|value| value.is_object() || value.is_array()) {
        result["structuredContent"] = structured;
    }
    rpc(id, result)
}

fn tool_json(id: Value, value: Value) -> Response {
    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
    if text.len() > LIMIT {
        tool_result(id, cut(text), None, false)
    } else {
        tool_result(id, text, Some(value), false)
    }
}

/// A picture a route answered with, as an image the agent sees.
fn tool_image(id: Value, reply: &Reply, name: &str, args: &Value) -> Response {
    use base64::Engine;
    let mime = reply.mime.split(';').next().unwrap_or_default().trim().to_string();
    let what = match name {
        "project_frame" => format!(
            "The {} frame of project {} at {} s.",
            if args.get("source").and_then(Value::as_str) == Some("original") { "original" } else { "dubbed" },
            args.get("pid").and_then(Value::as_str).unwrap_or_default(),
            args.get("t").and_then(Value::as_f64).unwrap_or_default()
        ),
        _ => format!("The avatar of character {}.", args.get("character_id").and_then(Value::as_str).unwrap_or_default()),
    };
    let image = base64::engine::general_purpose::STANDARD.encode(&reply.bytes);
    rpc(id, json!({ "content": [{ "type": "image", "data": image, "mimeType": mime }, { "type": "text", "text": what }], "isError": false }))
}

/// The name a tool shows the user: its words, the first capitalised.
fn tool_title(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut letters = words.chars();
    letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default()
}

pub async fn handle(headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !crate::guard::local_origin(&headers) {
        return crate::guard::foreign_origin();
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_failure(StatusCode::BAD_REQUEST, Value::Null, -32700, "Parse error".into(), None);
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return rpc_failure(StatusCode::BAD_REQUEST, Value::Null, -32600, "Invalid request: one JSON-RPC request or notification per POST".into(), None);
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // a notification is only acknowledged
    if message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    if is_disabled() {
        return rpc_failure(StatusCode::OK, id, -32000, "MCP server is disabled by the user in DubStudio".into(), None);
    }
    // A request of the stateless revision says its version in _meta and is
    // checked against its headers; one without it is of the handshake era.
    if let Some(version) = params.get("_meta").and_then(|meta| meta.get(META_VERSION)).and_then(Value::as_str) {
        if let Some((code, text, data)) = refused(&headers, method, &params, version) {
            return rpc_failure(StatusCode::BAD_REQUEST, id, code, text, data);
        }
    }
    if method != "tools/call" {
        seen(method);
    }
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let protocol = LEGACY.iter().find(|version| **version == asked).copied().unwrap_or(LEGACY[0]);
            rpc(id, json!({
                "protocolVersion": protocol,
                "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            }))
        }
        "server/discover" => rpc(id, cacheable(json!({
            "supportedVersions": supported_versions(),
            "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
            "instructions": INSTRUCTIONS,
        }))),
        "ping" => rpc(id, json!({})),
        "resources/list" => rpc(id, cacheable(json!({ "resources": [
            { "uri": SKILL_URI, "name": "studio skill", "title": "How to drive the studio", "description": "How to drive Dub Studio: every tool by area, the ground rules, step-by-step recipes.", "mimeType": "text/markdown" },
            { "uri": LANGUAGES_URI, "name": "languages", "title": "Languages", "description": "The language codes a video is dubbed from and into: tgt_lang, src_lang, lang.", "mimeType": "application/json" },
            { "uri": EDITS_URI, "name": "project edits", "title": "Project edits", "description": "Every edit of a project (PATCH op), the tool that makes it and its fields.", "mimeType": "application/json" },
        ] }))),
        "resources/templates/list" => rpc(id, cacheable(json!({ "resourceTemplates": [] }))),
        "resources/read" => {
            let uri = params.get("uri").and_then(Value::as_str).unwrap_or_default();
            match resource(uri) {
                Some((mime, text)) => rpc(id, cacheable(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }))),
                None => rpc_error(id, -32002, format!("Resource not found: {uri}")),
            }
        }
        "prompts/list" => rpc(id, cacheable(json!({ "prompts": [
            { "name": "studio", "title": "Studio skill", "description": "Load the studio's skill: the tools, the ground rules and the recipes." },
            { "name": "dub_video", "title": "Dub a video", "description": "Dub a video file on this computer into a language, from the file to the finished video.", "arguments": [
                { "name": "path", "description": "the video file", "required": true },
                { "name": "lang", "description": "the language to dub into, a code such as ru, en, es", "required": true },
                { "name": "mode", "description": "dub (default), voiceover, nodub (subtitles only) or transcribe", "required": false },
            ] },
        ] }))),
        "prompts/get" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let argument = |key: &str| params.get("arguments").and_then(|arguments| arguments.get(key)).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let text = match name {
                "studio" => Some(SKILL.to_string()),
                "dub_video" => {
                    let mode = Some(argument("mode")).filter(|mode| !mode.is_empty()).unwrap_or_else(|| "dub".into());
                    Some(format!(
                        "Dub the video {path} into {lang} (mode {mode}) with Dub Studio. Read the skill first (prompt studio, resource {SKILL_URI}).\n\n1. studio_status: the models must be ready and no job running.\n2. project_create with path {path}; keep its project_id.\n3. project_analyze with that pid, tgt_lang {lang} and mode {mode}; studio_wait with its job_id until done.\n4. project_get: read the translation line by line and fix what reads wrong with segment_update.\n5. project_render; studio_wait with its job_id.\n6. project_frame at a moment with speech to see the subtitles as they are burned in.\n7. project_save_output into the folder the user named (ask when they did not), then tell the user the path.",
                        path = argument("path"),
                        lang = argument("lang"),
                    ))
                }
                _ => None,
            };
            match text {
                Some(text) => rpc(id, json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] })),
                None => rpc_error(id, -32602, format!("Unknown prompt: {name}")),
            }
        }
        "tools/list" => {
            let list: Vec<Value> = tools().iter().map(|tool| json!({ "name": tool.name, "title": tool_title(tool.name), "description": tool.description, "inputSchema": (tool.schema)(), "annotations": annotations(tool.name) })).collect();
            rpc(id, cacheable(json!({ "tools": list })))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            seen(name);
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let answer = |text: String, error: bool| tool_result(id.clone(), text, None, error);
            let Some(tool) = tools().iter().find(|tool| tool.name == name) else {
                return rpc_error(id, -32602, format!("Unknown tool: {name}; tools/list names them all."));
            };
            // a tool may look at a file on disk before its call: off the runtime
            let prepare = tool.call;
            let prepared = {
                let args = args.clone();
                tokio::task::spawn_blocking(move || prepare(&args)).await.unwrap_or_else(|error| Err(format!("{name} failed: {error}")))
            };
            match prepared {
                Err(problem) => answer(problem, true),
                Ok(Call { payload: Payload::Window { command, args, seconds }, .. }) => match window::ask_window(command, args, seconds).await {
                    Ok(result) => window::window_reply(id, result),
                    Err(problem) => answer(problem, true),
                },
                Ok(call) if call.path == "composite:status" => tool_json(id, status_summary().await),
                Ok(call) if call.path == "composite:wait" => match wait_for(&args).await {
                    Ok(state) => tool_json(id, state),
                    Err(problem) => answer(problem, true),
                },
                Ok(call) if call.path.starts_with(atomic::PREFIX) => match atomic::run(name, &args).await {
                    Ok(result) => tool_json(id, result),
                    Err(problem) => answer(problem, true),
                },
                Ok(call) => {
                    if QUEUED_BEHIND_JOBS.contains(&name) {
                        if let Err(problem) = graphics_card_free().await {
                            return answer(problem, true);
                        }
                    }
                    match call_route_raw(call).await {
                        Ok(reply) if reply.status.is_success() && reply.mime.starts_with("image/") => tool_image(id, &reply, name, &args),
                        Ok(reply) if reply.status.is_success() => {
                            let text = reply_text(&reply);
                            match serde_json::from_str::<Value>(&text) {
                                Ok(value) => tool_json(id, shape(name, &args, redact(value))),
                                Err(_) => answer(text, false),
                            }
                        }
                        Ok(reply) => answer(reply_text(&reply), true),
                        Err(problem) => answer(problem, true),
                    }
                }
            }
        }
        _ => rpc_failure(StatusCode::NOT_FOUND, id, -32601, format!("Method not found: {method}"), None),
    }
}

/// A moment an agent names, as unix seconds in this computer's time zone:
/// today, yesterday, a date (its start, or its end for an upper bound) or a date-time.
fn moment(text: &str, end: bool) -> Result<i64, String> {
    let text = text.trim();
    if let Ok(ts) = text.parse::<i64>() {
        return Ok(ts);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs() as i64;
    let day_secs = 86_400i64;
    let today_start = now - (now % day_secs);
    match text.to_lowercase().as_str() {
        "today" => Ok(if end { today_start + day_secs - 1 } else { today_start }),
        "yesterday" => {
            let y_start = today_start - day_secs;
            Ok(if end { y_start + day_secs - 1 } else { y_start })
        }
        _ => {
            let parts: Vec<&str> = text.split(|c| c == 'T' || c == ' ').collect();
            let date_parts: Vec<i64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
            if date_parts.len() == 3 {
                let (y, m, d) = (date_parts[0], date_parts[1], date_parts[2]);
                if !(1970..=2100).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                    return Err(format!("'{text}' is not a moment: today, yesterday, 2026-09-26 or 2026-09-26T18:00."));
                }
                let mut days = (y - 1970) * 365 + (y - 1969) / 4 - (y - 1901) / 100 + (y - 1601) / 400;
                let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
                let month_days = [31, if is_leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
                for mon in 0..(m - 1) as usize {
                    days += month_days[mon];
                }
                days += d - 1;
                let mut sec = days * day_secs;
                if parts.len() > 1 && !parts[1].is_empty() {
                    let time_parts: Vec<i64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();
                    if !time_parts.is_empty() {
                        sec += time_parts[0] * 3600;
                    }
                    if time_parts.len() > 1 {
                        sec += time_parts[1] * 60;
                    }
                    if time_parts.len() > 2 {
                        sec += time_parts[2];
                    }
                } else if end {
                    sec += day_secs - 1;
                }
                Ok(sec)
            } else {
                Err(format!("'{text}' is not a moment: today, yesterday, 2026-09-26 or 2026-09-26T18:00."))
            }
        }
    }
}

/// What an agent is told when it connects.
const INSTRUCTIONS: &str = "You drive Dub Studio on this computer: it dubs, voices over, subtitles and transcribes videos. Every tool runs the same code as a button of the studio, through the routes its window calls. Start with studio_status. For one result from a file, without working in the studio, one call does it: transcribe_file (the transcript), translate_file (translated subtitles), dub_file (the dubbed video), separate_file (voice and background apart), detect_text_file (the text in the picture); export_subtitles writes a project's subtitles. Long work - project_analyze, project_dub_audio, project_render, project_export_lang, project_retranslate, project_remix, downloads - is a job: start it, then studio_wait with its job_id instead of polling. The graphics card runs one job at a time and a preview frame (project_frame) waits behind it, so look at frames while the studio is idle. Edits (segment_update, caption_style_set and the rest) are instant and saved; a line whose words, timing, speaker or voice changed is dirty, and project_dub_audio and project_render voice only the dirty lines again. Look ids up instead of guessing them: projects_list, project_get, voices_list, casting_get, casting_library_list, models_status. Files on this computer are passed by path. To work in front of the user, open the project in the studio's window with editor_open and use the editor_* tools (the user watches the lines, the timeline, the frame and the export change) and ui_* for anything else on screen; they need the window open, the rest works without it. The whole guide is the resource studio://skill (prompt 'studio').";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempfile;

    /// The router the tools call in these tests.
    pub(super) static STUB: OnceLock<Router> = OnceLock::new();

    fn a_project() -> Value {
        json!({
            "meta": { "video": "C:/v/clip.mp4", "duration": 10.0, "width": 1920, "height": 1080, "fps": 25.0, "src_codec": "h264" },
            "mode": "dub", "tgt_lang": "ru",
            "audio": { "voice": { "mode": "clone", "name": null }, "gain_db": 0.0 },
            "subs": { "mode": "translate", "burn": true },
            "render": { "blur": true },
            "captions": { "sub_style": { "color": "#FFFFFF" }, "sub_y": 900, "overrides": [], "titles": [{ "text": "Title", "tgt": "Титр" }], "blur_boxes": [{ "x": 1, "y": 2, "w": 3, "h": 4 }], "preset": {} },
            "raw_ctx": { "scene_context": "long" },
            "stage_ckpts": { "asr": "abc" },
            "segments": [
                { "id": "s1", "start": 0.5, "end": 2.0, "speaker": "0", "src_text": "Hello", "tgt_text": "Привет", "dirty": true, "words": [{ "w": "Hello", "s": 0.5 }], "ckpt": "k1" },
                { "id": "s2", "start": 4.0, "end": 6.0, "speaker": "1", "src_text": "Bye", "tgt_text": "Пока", "dirty": false, "hidden": true },
            ],
        })
    }

    /// A studio in miniature: a render running, an analysis done, a picture,
    /// a project, the settings with a key, and a page for what is not there;
    /// the projects and jobs of the one-call tools' tests are atomic::stub's.
    pub(super) fn stub() {
        use axum::extract::{Multipart, Path as Segment, Query};
        use axum::routing::{get, post};
        let jobs = || {
            json!({ "jobs": [
                { "id": "j1", "kind": "render", "pid": "p1", "status": "running", "stage": "tts", "msg": "voicing", "pct": 40.0 },
                { "id": "j2", "kind": "analyze", "pid": "p1", "status": "done", "stage": "done", "msg": "", "pct": 100.0, "result": { "project_id": "p1" } },
            ] })
        };
        let router = Router::new()
            .route("/jobs", get(move |Query(asked): Query<std::collections::HashMap<String, String>>| async move {
                Json(asked.get("pid").and_then(|pid| atomic::stub::jobs(pid)).unwrap_or_else(jobs))
            }))
            .route("/jobs/{id}", get(move |Segment(id): Segment<String>| async move {
                if let Some(job) = atomic::stub::job(&id) {
                    return Json(job).into_response();
                }
                match jobs()["jobs"].as_array().unwrap().iter().find(|job| job["id"] == id.as_str()) {
                    Some(job) => Json(job.clone()).into_response(),
                    None => (StatusCode::NOT_FOUND, "job not found").into_response(),
                }
            }))
            .route("/setup/status", get(|| async { Json(json!({ "ready": false, "driverOk": true, "downloadPending": 5, "components": [
                { "id": "higgs", "name": "Higgs", "requirement": "required", "installed": false, "size": 5, "bytesOnDisk": 0, "vram": 1, "missing": ["a"] },
                { "id": "ocr", "name": "OCR", "requirement": "recommended", "installed": true, "size": 1, "bytesOnDisk": 1, "vram": 0, "missing": [] },
            ] })) }))
            .route(
                "/projects/{pid}",
                get(|Segment(pid): Segment<String>| async move { atomic::stub::project(&pid).unwrap_or_else(|| Json(a_project()).into_response()) })
                    .patch(|Segment(pid): Segment<String>, Json(edit): Json<Value>| async move { atomic::stub::patch(&pid, &edit).unwrap_or_else(|| Json(a_project()).into_response()) }),
            )
            .route("/host", get(|headers: HeaderMap| async move { Json(json!({ "host": headers.get(header::HOST).and_then(|value| value.to_str().ok()) })) }))
            .route("/projects", post(|mut form: Multipart| async move {
                let mut parts = Vec::new();
                while let Some(field) = form.next_field().await.unwrap() {
                    let name = field.name().unwrap_or_default().to_string();
                    let file = field.file_name().unwrap_or_default().to_string();
                    let size = field.bytes().await.unwrap().len();
                    parts.push(json!({ "name": name, "file": file, "size": size }));
                }
                Json(json!({ "project_id": "p2", "parts": parts }))
            }))
            .merge(atomic::stub::routes())
            .fallback(|| async { axum::response::Html("<!doctype html><title>Dub Studio</title>") })
            .layer(axum::extract::DefaultBodyLimit::disable());
        let _ = STUB.set(router);
    }

    pub(super) async fn call_tool(name: &str, arguments: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        let response = handle(HeaderMap::new(), axum::body::Bytes::from(body.to_string())).await;
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 24).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    pub(super) fn answer_text(reply: &Value) -> String {
        reply["result"]["content"].as_array().unwrap().iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n")
    }

    // ---------------------------------------------------------------- the router and the edits, read from their source

    /// The routes of build_router: (METHOD, path pattern).
    fn router_routes() -> Vec<(String, String)> {
        let source = include_str!("lib.rs").replace("\r\n", "\n");
        let body = source.split("pub fn build_router(").nth(1).expect("build_router").split("\n}\n").next().unwrap();
        let code: String = body.lines().map(|line| line.split("//").next().unwrap_or_default()).collect::<Vec<_>>().join("\n");
        let mut routes = Vec::new();
        for chunk in code.split(".route(").skip(1) {
            let path = chunk.trim_start().strip_prefix('"').and_then(|rest| rest.split('"').next()).expect("a route's path").to_string();
            let handlers = chunk.split(".fallback(").next().unwrap().split(".layer(").next().unwrap().split(".with_state(").next().unwrap();
            for method in ["get", "post", "patch", "put", "delete"] {
                let call = format!("{method}(");
                let called = handlers.match_indices(&call).any(|(at, _)| !handlers[..at].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_'));
                if called {
                    routes.push((method.to_uppercase(), path.clone()));
                }
            }
        }
        routes
    }

    /// The ops patch::apply knows.
    fn patch_ops() -> Vec<String> {
        let source = include_str!("patch.rs").replace("\r\n", "\n");
        let body = source.split("pub fn apply(").nth(1).expect("patch::apply").split("other =>").next().unwrap();
        body.lines().filter_map(|line| line.trim().strip_prefix('"').and_then(|rest| rest.split_once("\" =>")).map(|(op, _)| op.to_string())).collect()
    }

    fn route_of(pattern: &str, path: &str) -> bool {
        let path = path.split('?').next().unwrap_or_default();
        let (wanted, given): (Vec<&str>, Vec<&str>) = (pattern.split('/').collect(), path.split('/').collect());
        wanted.len() == given.len() && wanted.iter().zip(&given).all(|(part, value)| part.starts_with('{') && !value.is_empty() || part == value)
    }

    /// A pattern with its holes filled, as a tool would call it.
    fn filled(pattern: &str) -> String {
        pattern.split('/').map(|part| if part.starts_with('{') { "x1" } else { part }).collect::<Vec<_>>().join("/")
    }

    /// Routes an agent is not given, and why.
    const NOT_TOOLS: &[(&str, &str, &str)] = &[
        ("GET", "/engine/capabilities", "studio internal: capabilities for web frontend UI"),
        ("PATCH", "/engine/opts", "studio internal: settings update for web frontend UI"),
        ("POST", "/engine/select", "studio internal: model selection for web frontend UI"),
        ("GET", "/engine/openrouter/models", "studio internal: OpenRouter model query for web frontend UI"),
        ("GET", "/engine/openrouter/voices", "studio internal: OpenRouter voices query for web frontend UI"),
        ("POST", "/engine/openrouter/verify", "studio internal: OpenRouter verification for web frontend UI"),
                ("POST", "/engine/proxy/test", "studio internal: proxy test for web frontend UI"),
        ("GET", "/engine/openrouter/catalog", "studio internal: OpenRouter model catalog for web frontend UI"),
        ("POST", "/engine/openrouter/catalog/refresh", "studio internal: OpenRouter catalog refresh for web frontend UI"),
        ("GET", "/engine/openrouter/settings", "studio internal: OpenRouter settings for web frontend UI"),
        ("PUT", "/engine/openrouter/settings", "studio internal: OpenRouter settings update for web frontend UI"),
        ("DELETE", "/engine/openrouter/settings", "studio internal: OpenRouter settings delete for web frontend UI"),
        ("GET", "/engine/proxy/settings", "studio internal: proxy settings for web frontend UI"),
        ("PUT", "/engine/proxy/settings", "studio internal: proxy settings update for web frontend UI"),
        ("GET", "/engine/server/models", "studio internal: local OpenAI-compatible server models query for web frontend UI"),
        ("GET", "/engine/server/key", "studio internal: local server key query for web frontend UI"),
        ("PUT", "/engine/server/key", "studio internal: local server key update for web frontend UI"),
        ("DELETE", "/engine/server/key", "studio internal: local server key delete for web frontend UI"),
        ("GET", "/engine/presets", "studio internal: subtitle presets list for web frontend UI"),
        ("POST", "/engine/preset", "studio internal: subtitle preset selection for web frontend UI"),
        ("POST", "/setup/download", "studio internal: component downloader for web frontend UI"),
        ("POST", "/setup/cancel", "studio internal: cancel download for web frontend UI"),
        ("POST", "/setup/open-models", "studio internal: open models directory in OS explorer"),
        ("POST", "/setup/browse", "studio internal: browse directory dialog for user"),
        ("POST", "/pick-folder", "studio internal: folder picker dialog for user"),
        ("POST", "/pick-file-save", "studio internal: file save picker dialog for user"),
        ("POST", "/setup/import", "studio internal: component import dialog for user"),
        ("GET", "/hw/snapshot", "studio internal: hardware snapshot for web frontend UI"),
        ("GET", "/record/devices", "studio internal: audio recording devices list for web frontend UI"),
        ("GET", "/record/level", "studio internal: microphone recording level for web frontend UI"),
        ("POST", "/record/start", "studio internal: microphone recording start for web frontend UI"),
        ("POST", "/record/stop", "studio internal: microphone recording stop for web frontend UI"),
        ("POST", "/voices/download-pack", "studio internal: voice pack downloader for web frontend UI"),
        ("GET", "/voices/catalog", "studio internal: online voice catalog for web frontend UI"),
        ("GET", "/voices/subfolders", "studio internal: voice subfolders for web frontend UI"),
        ("GET", "/voices/cast", "studio internal: cast voice assignment for web frontend UI"),
        ("POST", "/voices/open-cast", "studio internal: open cast folder in Explorer"),
        ("GET", "/voices/sample", "studio internal: voice sample audio stream for player"),
        ("POST", "/voices/get", "studio internal: fetch voice from catalog for web frontend UI"),
        ("POST", "/voices/rename", "studio internal: rename voice in library"),
        ("POST", "/voices/delete", "studio internal: delete voice from library"),
        ("POST", "/projects/{pid}/speaker-voice", "studio internal: create voice from speaker"),
        ("POST", "/projects/{pid}/voice-slots", "studio internal: voice slots assign for web frontend UI"),
        ("POST", "/projects/{pid}/autocast", "studio internal: autocast voices for web frontend UI"),
        ("GET", "/projects/{pid}/casting", "studio internal: get casting state for web frontend UI"),
        ("POST", "/projects/{pid}/casting", "studio internal: update casting state for web frontend UI"),
        ("GET", "/projects/{pid}/casting/avatar", "studio internal: casting avatar image stream for web frontend UI"),
        ("GET", "/projects/{pid}/casting/voice", "studio internal: casting voice sample audio stream for web frontend UI"),
        ("GET", "/casting/library", "studio internal: list casting library for web frontend UI"),
        ("POST", "/projects/{pid}/casting/library", "studio internal: save character to library"),
        ("DELETE", "/casting/library/{slug}", "studio internal: delete character from library"),
        ("GET", "/casting/library/{slug}/avatar", "studio internal: character avatar image stream"),
        ("GET", "/fonts", "studio internal: fonts list for web frontend UI"),
        ("GET", "/presets", "studio internal: caption presets list for web frontend UI"),
        ("PUT", "/projects/{pid}", "studio internal: replace whole project JSON"),
        ("DELETE", "/projects/{pid}", "studio internal: delete project"),
        ("POST", "/projects/{pid}/align", "studio internal: forced alignment stage"),
        ("POST", "/projects/{pid}/regroup", "studio internal: regroup segments stage"),
        ("POST", "/projects/{pid}/remix", "studio internal: remix dub stage"),
        ("POST", "/projects/{pid}/retranslate", "studio internal: retranslate segments stage"),
        ("GET", "/projects/{pid}/waveform", "studio internal: waveform data for web frontend UI"),
        ("GET", "/projects/{pid}/preview", "studio internal: preview frame image stream"),
        ("GET", "/projects/{pid}/output", "studio internal: output video stream for player"),
        ("POST", "/projects/{pid}/open", "studio internal: open output video in OS player"),
        ("POST", "/projects/{pid}/open-folder", "studio internal: open project folder in Explorer"),
        ("POST", "/projects/{pid}/trash", "studio internal: move project to trash"),
        ("POST", "/projects/{pid}/reveal", "studio internal: reveal file in Explorer"),
        ("POST", "/projects/{pid}/save-text", "studio internal: save custom text file"),
        ("POST", "/projects/{pid}/synth-segments", "studio internal: segment TTS synthesis pipeline step"),
        ("POST", "/projects/{pid}/mix-audio", "studio internal: audio mixing pipeline step"),
        ("GET", "/projects/{pid}/segments/{id}/audio", "studio internal: segment audio stream for player"),
        ("GET", "/projects/{pid}/original", "studio internal: original video frame image stream"),
        ("GET", "/projects/{pid}/source-video", "studio internal: source video stream for player"),
        ("GET", "/projects/{pid}/dub", "studio internal: dubbed video stream for player"),
        ("GET", "/projects/{pid}/audio-dub-clean", "studio internal: clean dubbed audio stream for player"),
        ("GET", "/projects/{pid}/audio-vocals", "studio internal: separated vocals stream for player"),
        ("GET", "/projects/{pid}/audio-bgm", "studio internal: background music stream for player"),
        ("POST", "/jobs/{job_id}/cancel", "studio internal: cancel job endpoint"),
        ("GET", "/jobs/{job_id}/events", "studio internal: job progress SSE stream for web frontend UI"),
        ("POST", "/window/title", "studio internal: window title control"),
        ("POST", "/mcp", "MCP server JSON-RPC endpoint"),
        ("GET", "/mcp/status", "MCP server status endpoint"),
        ("GET", "/mcp/window", "MCP window command event stream"),
        ("POST", "/mcp/window/result", "MCP window command response endpoint"),
        ("POST", "/mcp/window/focus", "MCP window focus event endpoint"),
    ];

    /// Routes the tools call that come with the parallel work on jobs; once one
    /// is in the router it leaves this list.
    const PENDING: &[(&str, &str)] = &[];

    /// What the composite tools read.
    const COMPOSITE_ROUTES: &[(&str, &[(&str, &str)])] = &[
        ("composite:status", &[("GET", "/jobs"), ("GET", "/setup/status")]),
        ("composite:wait", &[("GET", "/jobs/x1"), ("GET", "/jobs"), ("GET", "/setup/status")]),
    ];

    /// Arguments for every property of a tool, and one set more for each other
    /// choice of an enum.
    fn samples(tool: &Tool, video: &Path, subtitles: &Path) -> Vec<Value> {
        let schema = (tool.schema)();
        let properties = schema["properties"].as_object().cloned().unwrap_or_default();
        let sample = |name: &str, property: &Value| -> Value {
            if let Some(first) = property["enum"].as_array().and_then(|choices| choices.first()) {
                return first.clone();
            }
            let kind = match &property["type"] {
                Value::Array(kinds) => kinds[0].as_str().unwrap_or("string").to_string(),
                other => other.as_str().unwrap_or("string").to_string(),
            };
            match (kind.as_str(), name) {
                ("integer", _) => json!(1),
                ("number", _) => json!(1.5),
                ("boolean", _) => json!(true),
                ("array", _) if property["items"]["type"] == "integer" => json!([1]),
                ("array", _) => json!(["x1"]),
                ("object", _) => json!({}),
                (_, "path") => json!(video.to_string_lossy()),
                (_, "subtitles_path") => json!(subtitles.to_string_lossy()),
                _ => json!("x1"),
            }
        };
        let base: serde_json::Map<String, Value> = properties.iter().map(|(name, property)| (name.clone(), sample(name, property))).collect();
        let mut all = vec![Value::Object(base.clone())];
        for (name, property) in &properties {
            for choice in property["enum"].as_array().into_iter().flatten().skip(1) {
                let mut other = base.clone();
                other.insert(name.clone(), choice.clone());
                all.push(Value::Object(other));
            }
        }
        all
    }

    /// The routes a tool reaches: (METHOD, path).
    fn reached_by(tool: &Tool, video: &Path, subtitles: &Path) -> Vec<(String, String)> {
        let mut reached = Vec::new();
        for args in samples(tool, video, subtitles) {
            let call = (tool.call)(&args).unwrap_or_else(|problem| panic!("{} refused {args}: {problem}", tool.name));
            if matches!(call.payload, Payload::Window { .. }) {
                continue;
            }
            match COMPOSITE_ROUTES.iter().chain(atomic::ROUTES).find(|(path, _)| *path == call.path) {
                Some((_, reads)) => reached.extend(reads.iter().map(|(method, path)| (method.to_string(), path.to_string()))),
                None => {
                    assert!(!call.path.starts_with("composite:"), "{} is a composite this test does not know", tool.name);
                    reached.push((call.method.to_string(), call.path.clone()));
                }
            }
        }
        reached
    }

    #[test]
    fn the_transcript_is_what_is_heard_and_the_translation_what_is_burned() {
        let project = json!({
            "tgt_lang": "ru",
            "captions": { "overrides": [{ "seg_id": "d", "text": "До встречи" }] },
            "segments": [
                { "id": "a", "start": 59.96, "end": 61.0, "speaker": "0", "src_text": "Hello", "tgt_text": "Привет" },
                { "id": "b", "start": 61.0, "end": 62.0, "speaker": "0", "src_text": "Thanks for watching", "tgt_text": "Спасибо", "hidden": true },
                { "id": "c", "start": 62.0, "end": 63.0, "speaker": "1", "src_text": "Bonjour", "tgt_text": "Бонжур", "keep_original": true },
                { "id": "d", "start": 63.0, "end": 64.0, "speaker": "1", "src_text": "Bye", "tgt_text": "Пока" },
            ]
        });
        let heard = transcript(&project, &json!({ "format": "text" }));
        assert_eq!(heard["text"], "[1:00.0 SPK 0] Hello\n[1:02.0 SPK 1] Bonjour\n[1:03.0 SPK 1] Bye");
        let burned = transcript(&project, &json!({ "text": "tgt" }));
        let texts: Vec<&str> = burned["lines"].as_array().unwrap().iter().map(|line| line["text"].as_str().unwrap()).collect();
        assert_eq!((texts, burned["language"].as_str()), (vec!["Привет", "До встречи"], Some("ru")));
    }

    #[test]
    fn every_route_and_every_edit_is_a_tool() {
        let folder = tempfile::tempdir().unwrap();
        let (video, subtitles) = (folder.path().join("clip.mp4"), folder.path().join("clip.srt"));
        std::fs::write(&video, b"x").unwrap();
        std::fs::write(&subtitles, b"x").unwrap();
        let routes = router_routes();
        assert!(routes.len() > 50, "build_router read: {routes:?}");
        let mut covered = vec![false; routes.len()];
        for tool in tools() {
            for (method, path) in reached_by(tool, &video, &subtitles) {
                let found: Vec<usize> = routes.iter().enumerate().filter(|(_, (m, p))| *m == method && route_of(p, &path)).map(|(at, _)| at).collect();
                let pending = PENDING.iter().any(|(m, p)| *m == method && route_of(p, &path));
                assert!(!found.is_empty() || pending, "{} calls {method} {path}, a route the studio does not have", tool.name);
                for at in found {
                    covered[at] = true;
                }
            }
        }
        let by_tool = covered.clone();
        let mut stale = Vec::new();
        for (method, path) in NOT_TOOLS.iter().map(|(method, path, _)| (method, path)) {
            let at = routes.iter().position(|(m, p)| m == method && p == path).unwrap_or_else(|| panic!("{method} {path} of NOT_TOOLS is not a route of build_router"));
            if by_tool[at] {
                stale.push(format!("{method} {path}"));
            }
            covered[at] = true;
        }
        assert!(stale.is_empty(), "these routes of NOT_TOOLS are reached by a tool: drop them from NOT_TOOLS: {stale:?}");
        let missing: Vec<String> = routes.iter().zip(&covered).filter(|(_, done)| !**done).map(|((method, path), _)| format!("{method} {path}")).collect();
        assert!(missing.is_empty(), "these routes have no tool: give each one, or put it into NOT_TOOLS with why: {missing:?}");
        let landed: Vec<String> = PENDING.iter().filter(|(method, pattern)| routes.iter().any(|(m, p)| m == method && route_of(p, &filled(pattern)))).map(|(method, pattern)| format!("{method} {pattern}")).collect();
        assert!(landed.is_empty(), "these routes are in the router now: drop them from PENDING: {landed:?}");

        let ops = patch_ops();
        for op in every_op() {
            assert!(ops.iter().any(|known| known == op), "{op} is not an op of patch::apply");
        }
        for (op, name) in PATCH_OPS {
            let tool = tools().iter().find(|tool| tool.name == *name).unwrap_or_else(|| panic!("{name} is not a tool"));
            let call = (tool.call)(&samples(tool, &video, &subtitles)[0]).unwrap();
            assert_eq!((call.method.clone(), call.path.as_str()), (Method::PATCH, "/projects/x1"), "{name}");
            match call.payload {
                Payload::Json(body) => assert_eq!(body["op"], *op, "{name} makes {op}"),
                _ => panic!("{name} sends a JSON body"),
            }
        }
        for (alias, same) in PATCH_ALIASES {
            assert!(PATCH_OPS.iter().any(|(op, _)| op == same), "{alias} names {same}, which no tool makes");
        }
    }

    #[test]
    fn every_tool_is_in_the_skill() {
        let missing: Vec<&str> = tools().iter().map(|tool| tool.name).filter(|name| !SKILL.contains(&format!("`{name}`"))).collect();
        assert!(missing.is_empty(), "docs/mcp-skill.md does not name {missing:?}");
    }

    #[test]
    fn every_tool_has_a_unique_name_and_an_object_schema() {
        let mut names: Vec<&str> = tools().iter().map(|tool| tool.name).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "two tools share a name");
        for tool in tools() {
            let schema = (tool.schema)();
            assert_eq!(schema["type"], "object", "{} has no object schema", tool.name);
            assert!(!tool.description.is_empty(), "{} has no description", tool.name);
        }
    }

    #[test]
    fn a_tool_turns_its_arguments_into_its_route() {
        let find = |name: &str| tools().iter().find(|tool| tool.name == name).expect("the tool");
        let call = (find("segment_update").call)(&json!({ "pid": "p 1", "id": "s1", "tgt_text": "Привет", "response_format": "detailed" })).unwrap();
        assert_eq!(call.method, Method::PATCH);
        assert_eq!(call.path, "/projects/p%201");
        match call.payload {
            Payload::Json(body) => assert_eq!(body, json!({ "op": "segment", "id": "s1", "tgt_text": "Привет" })),
            _ => panic!("a JSON body"),
        }
        let call = (find("project_analyze").call)(&json!({ "pid": "p1", "tgt_lang": "ru", "mode": "voiceover", "burn": false, "casting": true, "rewrite": "make it rhyme & shine" })).unwrap();
        assert_eq!(call.path, "/projects/p1/analyze?tgt_lang=ru&mode=voiceover&rewrite=make%20it%20rhyme%20%26%20shine&burn=0&casting=1");
        let call = (find("project_analyze").call)(&json!({ "pid": "p1", "mode": "transcribe" })).unwrap();
        assert_eq!(call.path, "/projects/p1/analyze?mode=transcribe");
        assert!((find("project_analyze").call)(&json!({ "pid": "p1", "mode": "dub" })).is_err(), "a dub needs tgt_lang");
        assert!((find("project_get").call)(&json!({})).is_err(), "a missing id is refused");
        assert!((find("project_create").call)(&json!({ "path": "Z:/nowhere/clip.mp4" })).is_err(), "a file that is not there is refused before the upload");
        let call = (find("project_dub_audio").call)(&json!({ "pid": "p1" })).unwrap();
        assert_eq!((call.method, call.path.as_str()), (Method::POST, "/projects/p1/dub-audio"));
        let call = (find("project_render").call)(&json!({ "pid": "p1" })).unwrap();
        assert_eq!((call.method, call.path.as_str()), (Method::POST, "/projects/p1/render"));
    }

    #[test]
    fn analyze_supports_bilingual_subtitles() {
        let find = |name: &str| tools().iter().find(|tool| tool.name == name).expect("the tool");
        let schema = (find("project_analyze").schema)();
        assert!(schema["properties"]["subs"]["enum"].as_array().unwrap().contains(&json!("bilingual")));
    }

    #[test]
    fn a_tool_that_changes_something_is_not_read_only() {
        for name in ["project_create", "segment_update", "project_dub_audio", "dub_file"] {
            assert_eq!(annotations(name)["readOnlyHint"], false, "{name}");
        }
        for name in ["studio_status", "studio_wait", "project_get", "project_transcript", "projects_list", "ui_screenshot", "ui_read_page"] {
            assert_eq!(annotations(name)["readOnlyHint"], true, "{name}");
        }
    }

    #[test]
    fn a_tool_that_writes_over_what_was_stored_is_destructive() {
        for name in ["project_analyze", "segment_update", "segments_regen_all", "segments_merge"] {
            assert_eq!(annotations(name)["destructiveHint"], true, "{name}");
        }
        for name in ["project_create", "project_render", "project_export_lang", "segment_regen", "segment_split"] {
            assert_eq!(annotations(name)["destructiveHint"], false, "{name}");
        }
    }

    #[test]
    fn the_key_never_leaves_the_studio() {
        let clean = redact(json!({ "selection": { "or_key": "sk-or-secret", "proxy_url": "socks5://user:pass@host:1080", "bench": "1" }, "list": [{ "or_key": "" }] }));
        assert_eq!(clean, json!({ "selection": { "or_key_set": true, "proxy_url": "socks5://user@host:1080", "bench": "1" }, "list": [{ "or_key_set": false }] }));
    }

    #[test]
    fn an_agent_is_answered_what_it_acts_on() {
        let short = compact_project(&a_project(), &json!({}));
        assert_eq!(short["segments_total"], 2);
        assert_eq!(short["dirty"], 1);
        assert_eq!(short["segments"][1]["hidden"], true);
        let text = short.to_string();
        assert!(!text.contains("scene_context") && !text.contains("\"words\"") && !text.contains("\"ckpt\""), "no vision context, word timings or checksums");
        let window = compact_project(&a_project(), &json!({ "from": 3.0, "to": 10.0 }));
        assert_eq!(window["segments"].as_array().unwrap().len(), 1);
        assert_eq!(window["segments"][0]["id"], "s2");

        let change = compact_change("segment_update", &json!({ "pid": "p1", "id": "s1" }), &a_project());
        assert_eq!(change["changed"][0]["tgt_text"], "Привет");
        assert_eq!(shape("segment_update", &json!({ "response_format": "detailed" }), a_project()), a_project(), "detailed is the whole project");

        let job = compact_job(&json!({ "id": "j", "kind": "analyze", "pid": "p", "status": "done", "result": a_project() }));
        assert_eq!(job["result"], "the project, updated: project_get reads it");
        assert!(job.get("error").is_none());
    }

    #[test]
    fn idle_waits_for_every_kind_of_work() {
        assert!(busy(&json!({ "jobs": [] })).unwrap().is_empty());
        assert_eq!(busy(&json!({ "jobs": [{ "kind": "render", "status": "running" }, { "status": "queued" }] })).unwrap(), ["render", "unknown"]);
        assert!(busy(&json!({ "jobs_error": "GET /jobs: 404" })).is_err(), "jobs that cannot be read are not idle");
        assert!(finished(&json!({ "status": "cancelled" })) && !finished(&json!({ "status": "running" })) && !finished(&json!({ "status": "cancelling" })));
    }

    #[test]
    fn the_server_names_the_apps_version() {
        let version = app_version();
        assert_eq!(version.split('.').count(), 3, "{version}");
        assert!(version.split('.').all(|part| part.parse::<u32>().is_ok()), "{version}");
    }

    #[tokio::test]
    async fn a_multipart_body_streams_its_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("clip.mp4");
        std::fs::write(&path, vec![7u8; (1 << 20) + 5]).unwrap();
        let parts = vec![Part::Text("head".into()), Part::File(path), Part::Text("tail".into())];
        let chunks: Vec<axum::body::Bytes> = futures_util::StreamExt::collect::<Vec<_>>(streamed(parts)).await.into_iter().map(Result::unwrap).collect();
        let whole: Vec<u8> = chunks.concat();
        assert_eq!(whole.len(), 4 + (1 << 20) + 5 + 4);
        assert!(whole.starts_with(b"head") && whole.ends_with(b"tail"));
    }

    #[tokio::test]
    async fn a_video_by_its_path_arrives_as_the_page_uploads_it() {
        stub();
        let folder = tempfile::tempdir().unwrap();
        let (video, subtitles) = (folder.path().join("My clip.mp4"), folder.path().join("My clip.srt"));
        std::fs::write(&video, vec![1u8; 3 * (1 << 20) + 7]).unwrap();
        let cues = b"1\n00:00:00,000 --> 00:00:01,000\nHi\n";
        std::fs::write(&subtitles, cues).unwrap();
        let reply = call_tool("project_create", json!({ "path": video.to_string_lossy(), "subtitles_path": subtitles.to_string_lossy() })).await;
        let answer = &reply["result"]["structuredContent"];
        assert_eq!(answer["project_id"], "p2", "{reply}");
        assert_eq!(answer["parts"], json!([{ "name": "file", "file": "My clip.mp4", "size": 3 * (1 << 20) + 7 }, { "name": "subs", "file": "My clip.srt", "size": cues.len() }]));
    }

    #[tokio::test]
    async fn a_job_is_waited_for() {
        stub();
        let done = wait_for(&json!({ "job_id": "j2" })).await.unwrap();
        assert_eq!((done["done"].clone(), done["state"]["result"]["project_id"].clone()), (json!(true), json!("p1")));
        let other_work = wait_for(&json!({ "until": "analyze" })).await.unwrap();
        assert_eq!(other_work["done"], true, "only a render runs");
        let running = wait_for(&json!({ "job_id": "j1", "seconds": 2 })).await.unwrap();
        assert_eq!((running["done"].clone(), running["state"]["stage"].clone()), (json!(false), json!("tts")));
        assert!(wait_for(&json!({ "job_id": "nope" })).await.unwrap_err().starts_with("No job nope"));

        let status = call_tool("studio_status", json!({})).await;
        let summary = &status["result"]["structuredContent"];
        assert_eq!(summary["jobs"][0]["kind"], "render");
        assert_eq!(summary["finished_jobs"][0]["id"], "j2");
        assert_eq!(summary["models"]["missing_required"][0]["id"], "higgs");
    }

    #[tokio::test]
    async fn a_tool_calls_its_route_as_this_computer() {
        stub();
        let reply = call_route_raw(Call { method: Method::GET, path: "/host".into(), payload: Payload::None }).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&reply.bytes).unwrap()["host"], "127.0.0.1");
    }

    #[tokio::test]
    async fn a_picture_comes_back_as_an_image() {
        let reply = Reply {
            status: StatusCode::OK,
            mime: "image/jpeg".into(),
            bytes: axum::body::Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xD9]),
        };
        let response = tool_image(json!(7), &reply, "ui_screenshot", &json!({}));
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let answer: Value = serde_json::from_slice(&bytes).unwrap();
        let content = &answer["result"]["content"];
        assert_eq!((content[0]["type"].clone(), content[0]["mimeType"].clone(), content[0]["data"].clone()), (json!("image"), json!("image/jpeg"), json!("/9j/2Q==")));
    }

    #[tokio::test]
    async fn answers_are_short_and_keep_the_key_inside() {
        stub();
        let edit = call_tool("segment_update", json!({ "pid": "p1", "id": "s1", "tgt_text": "Привет" })).await;
        assert_eq!(edit["result"]["structuredContent"]["changed"][0]["id"], "s1", "{edit}");
        let missing = call_tool("nonexistent_tool", json!({})).await;
        assert_eq!(missing["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn a_stateless_request_is_checked_against_its_headers() {
        let call = |headers: &[(&str, &str)], body: Value| {
            let mut map = HeaderMap::new();
            for (name, value) in headers {
                map.insert(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
            }
            async move {
                let response = handle(map, axum::body::Bytes::from(body.to_string())).await;
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
            }
        };
        let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {} });
        let modern = [("mcp-protocol-version", "2026-07-28"), ("mcp-method", "server/discover")];
        let (status, found) = call(&modern, json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": meta } })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(found["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(found["result"]["resultType"], "complete");
        assert_eq!(found["result"]["cacheScope"], "public");
        assert!(found["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"].is_string());

        let (status, found) = call(&[("mcp-protocol-version", "2026-07-28"), ("mcp-method", "tools/call"), ("mcp-name", "project_analyze")], json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "studio_status", "_meta": meta } })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a name that differs from the body is refused");
        assert_eq!(found["error"]["code"], HEADER_MISMATCH);

        let (status, found) = call(&[("mcp-protocol-version", "2026-07-28")], json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": { "_meta": meta } })).await;
        assert_eq!((status, found["error"]["code"].clone()), (StatusCode::BAD_REQUEST, json!(HEADER_MISMATCH)), "a missing Mcp-Method is refused");

        let old = json!({ "io.modelcontextprotocol/protocolVersion": "1900-01-01" });
        let (status, found) = call(&[("mcp-protocol-version", "1900-01-01"), ("mcp-method", "tools/list")], json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": { "_meta": old } })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(found["error"]["code"], UNSUPPORTED_VERSION);
        assert_eq!(found["error"]["data"]["requested"], "1900-01-01");

        let encoded = format!("=?base64?{}?=", { use base64::Engine; base64::engine::general_purpose::STANDARD.encode("studio://skill") });
        let (status, found) = call(&[("mcp-protocol-version", "2026-07-28"), ("mcp-method", "resources/read"), ("mcp-name", encoded.as_str())], json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": { "uri": "studio://skill", "_meta": meta } })).await;
        assert_eq!(status, StatusCode::OK, "a Base64 name is decoded before it is compared");
        assert!(found["result"]["contents"][0]["text"].as_str().unwrap().contains("MCP"));

        let (status, found) = call(&[("mcp-protocol-version", "2026-07-28"), ("mcp-method", "nope/nope")], json!({ "jsonrpc": "2.0", "id": 6, "method": "nope/nope", "params": { "_meta": meta } })).await;
        assert_eq!((status, found["error"]["code"].clone()), (StatusCode::NOT_FOUND, json!(-32601)));

        let (status, _) = call(&[], json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let mut foreign = HeaderMap::new();
        foreign.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        let response = handle(foreign, axum::body::Bytes::from(json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }).to_string())).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "a web page is refused");
    }

    #[tokio::test]
    async fn the_server_introduces_itself_and_lists_its_tools() {
        let reply = |body: Value| async move {
            let response = handle(HeaderMap::new(), axum::body::Bytes::from(body.to_string())).await;
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        };
        let hello = reply(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).await;
        assert_eq!(hello["result"]["capabilities"], json!({ "tools": {}, "resources": {}, "prompts": {} }));
        assert_eq!(hello["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(hello["result"]["serverInfo"], json!({ "name": "dub-studio", "title": "Dub Studio", "version": app_version() }));
        let languages = reply(json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": { "uri": "studio://languages" } })).await;
        assert!(languages["result"]["contents"][0]["text"].as_str().unwrap().contains("\"ru\""));
        let edits = reply(json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": { "uri": "studio://patch-ops" } })).await;
        let edits: Value = serde_json::from_str(edits["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(edits["ops"].as_array().unwrap().len(), every_op().len());
        assert!(edits["ops"][0]["fields"]["tgt_text"].is_object(), "an op lists its tool's fields");
        let skill = reply(json!({ "jsonrpc": "2.0", "id": 6, "method": "prompts/get", "params": { "name": "studio" } })).await;
        assert!(skill["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("MCP"));
        let dub = reply(json!({ "jsonrpc": "2.0", "id": 7, "method": "prompts/get", "params": { "name": "dub_video", "arguments": { "path": "C:/v/a.mp4", "lang": "es" } } })).await;
        let recipe = dub["result"]["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(recipe.contains("C:/v/a.mp4") && recipe.contains("tgt_lang es") && recipe.contains("mode dub"), "{recipe}");
        let list = reply(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), tools().len());
        assert_eq!(list["result"]["tools"][0]["title"], "Studio status");
        let unknown = reply(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "nope" } })).await;
        assert_eq!(unknown["error"]["code"], -32602);
    }
}
