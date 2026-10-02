//! The studio's window as an agent's second way in, next to the tools that call routes: the agent
//! works in the editor the user watches. The page subscribes to a stream of commands (GET
//! /mcp/window), executes each and posts its answer back by the command's id (POST
//! /mcp/window/result); the window the person turned to last is the one asked (POST
//! /mcp/window/focus). The same stream tells every window what changed behind it - a project
//! saved, the projects, the settings, the voices, a job started - so the page reads it again.
//!
//! Every save of a project raises its revision. The page learns the revision of what it shows
//! from the `x-project-rev` header and sends, with the whole-project PUT of its undo, the revision
//! the state it writes back was taken at: the PUT is refused when anyone but its own window saved
//! the project after that revision, so an undo never erases what an agent or another window saved
//! meanwhile - even when the window's own later edit already carried that save to it.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::convert::Infallible;

use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use tokio::sync::broadcast::error::RecvError;

use crate::jobs::JobFn;

use super::*;

/// The header the page marks every request of its window with.
const WINDOW_HEADER: &str = "x-dub-window";
/// The header the MCP tools' own calls carry.
pub(super) const AGENT_HEADER: &str = "x-dub-agent";
/// The revision of the project an answer shows, and the one the state a whole-project PUT writes
/// was taken at.
pub(crate) const REV_HEADER: &str = "x-project-rev";
/// The error code of a PUT whose state someone else's later save would be lost under.
pub(crate) const PROJECT_CHANGED: &str = "project_changed";

const NO_WINDOW: &str = "The studio's window is not open, so there is nothing on screen to work in. Open Dub Studio (or its address in a browser) and call the tool again; every tool that is not ui_* or editor_* works without the window.";

/// Kinds of work a POST starts as a job, by the last part of its route.
const JOB_ROUTES: &[&str] = &["analyze", "render", "dub-audio", "export-lang", "retranslate", "remix", "align"];

struct Bridge {
    commands: tokio::sync::broadcast::Sender<String>,
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Result<Value, String>>>>,
    /// The open windows, the one the person turned to last at the end: a
    /// command goes to that one only, so a second window never runs it again.
    windows: Mutex<Vec<u64>>,
    sequence: AtomicU64,
}

fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(|| Bridge {
        commands: tokio::sync::broadcast::channel(256).0,
        pending: Mutex::new(HashMap::new()),
        windows: Mutex::new(Vec::new()),
        sequence: AtomicU64::new(0),
    })
}

fn open_windows() -> std::sync::MutexGuard<'static, Vec<u64>> {
    bridge().windows.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn pending() -> std::sync::MutexGuard<'static, HashMap<String, tokio::sync::oneshot::Sender<Result<Value, String>>>> {
    bridge().pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How many windows listen now, for the settings page.
pub(super) fn windows_open() -> usize {
    open_windows().len()
}

/// Keeps the window on the list while its stream is open.
struct Listening(u64);

impl Drop for Listening {
    fn drop(&mut self) {
        open_windows().retain(|window| *window != self.0);
    }
}

/// The stream of commands and notices the studio's page follows. Its first message names the
/// window, so the page knows the commands addressed to it. A window that fell behind the stream
/// is told to read everything again instead of silently missing a notice.
pub async fn window_events() -> Response {
    let receiver = bridge().commands.subscribe();
    let window = bridge().sequence.fetch_add(1, Ordering::Relaxed);
    open_windows().push(window);
    let hello = futures_util::stream::once(async move { Ok::<Event, Infallible>(Event::default().data(json!({ "window": window }).to_string())) });
    let commands = futures_util::stream::unfold((receiver, Listening(window)), |(mut receiver, listening)| async move {
        match receiver.recv().await {
            Ok(command) => Some((Ok(Event::default().data(command)), (receiver, listening))),
            Err(RecvError::Lagged(missed)) => Some((Ok(Event::default().data(json!({ "changed": "everything", "missed": missed }).to_string())), (receiver, listening))),
            Err(RecvError::Closed) => None,
        }
    });
    Sse::new(futures_util::StreamExt::chain(hello, commands)).keep_alive(KeepAlive::default()).into_response()
}

#[derive(serde::Deserialize)]
pub struct WindowAnswer {
    id: String,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

/// The page's answer to one command.
pub async fn window_result(Json(answer): Json<WindowAnswer>) -> StatusCode {
    let waiting = pending().remove(&answer.id);
    match waiting {
        Some(sender) => {
            let _ = sender.send(match answer.error {
                Some(error) => Err(error),
                None => Ok(answer.result),
            });
            StatusCode::NO_CONTENT
        }
        None => StatusCode::NOT_FOUND,
    }
}

#[derive(serde::Deserialize)]
pub struct WindowFocus {
    window: u64,
}

/// The page the person turned to: an agent's command goes there, not to the window that
/// happened to open last.
pub async fn window_focus(Json(focus): Json<WindowFocus>) -> StatusCode {
    if focus_window(focus.window) { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
}

fn focus_window(window: u64) -> bool {
    let mut windows = open_windows();
    let Some(place) = windows.iter().position(|open| *open == window) else { return false };
    windows.remove(place);
    windows.push(window);
    true
}

/// An event for every open window.
pub(crate) fn tell_windows(event: Value) {
    let _ = bridge().commands.send(event.to_string());
}

/// Asks the window the person turned to last to run a command, and waits for its answer.
pub(super) async fn ask_window(command: &str, args: Value, seconds: u64) -> Result<Value, String> {
    let Some(window) = open_windows().last().copied() else {
        return Err(NO_WINDOW.into());
    };
    let id = format!("w{}", bridge().sequence.fetch_add(1, Ordering::Relaxed));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    pending().insert(id.clone(), sender);
    if bridge().commands.send(json!({ "id": id, "window": window, "command": command, "args": args }).to_string()).is_err() {
        pending().remove(&id);
        return Err(NO_WINDOW.into());
    }
    match tokio::time::timeout(Duration::from_secs(seconds), receiver).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => Err(format!("The window dropped '{command}' without an answer.")),
        Err(_) => {
            pending().remove(&id);
            Err(format!("The window did not answer '{command}' within {seconds} s: it may be reloading or busy. ui_read_page shows what it is doing; call the tool again."))
        }
    }
}

/// A tool that the window answers.
pub(super) fn window(command: &'static str, args: &Value, seconds: u64) -> Result<Call, String> {
    Ok(Call { method: Method::GET, path: String::new(), payload: Payload::Window { command, args: args.clone(), seconds } })
}

/// The window's answer as a tool's: a picture it took as an image the agent sees, text as text,
/// and anything else as structured content.
pub(super) fn window_reply(id: Value, result: Value) -> Response {
    let mut content = Vec::new();
    if let Some(image) = result.get("image").and_then(Value::as_str) {
        let mime = result.get("mime").and_then(Value::as_str).unwrap_or("image/png");
        content.push(json!({ "type": "image", "data": image, "mimeType": mime }));
    }
    let text = match result.get("text").and_then(Value::as_str) {
        Some(text) => text.to_string(),
        None if result.is_null() || result.get("image").is_some() => "Done.".into(),
        None => serde_json::to_string_pretty(&result).unwrap_or_default(),
    };
    content.push(json!({ "type": "text", "text": cut(text) }));
    let mut answer = json!({ "content": content, "isError": false });
    if result.get("image").is_none() && result.get("text").is_none() && (result.is_object() || result.is_array()) {
        answer["structuredContent"] = result;
    }
    rpc(id, answer)
}

// ---------------------------------------------------------------- revisions and who changed what

/// Who made the request being served, the revision the state its whole-project PUT writes was
/// taken at, and what its save of the project did (0: no save), shared with the blocking work it
/// hands on. A job runs in a scope of its own: its author's, marked `job`.
#[derive(Clone)]
#[allow(dead_code)]
struct Scope {
    actor: String,
    expected: Option<u64>,
    job: bool,
    saved: Arc<AtomicU64>,
    conflict: Arc<AtomicBool>,
}

impl Scope {
    fn new(actor: String, expected: Option<u64>) -> Self {
        Scope { actor, expected, job: false, saved: Arc::new(AtomicU64::new(0)), conflict: Arc::new(AtomicBool::new(false)) }
    }

    fn of_job(actor: String) -> Self {
        Scope { job: true, ..Scope::new(actor, None) }
    }
}

tokio::task_local! {
    static REQUEST: Scope;
}

/// A project's revision and who saved it: enough to tell whether everything saved after a given
/// revision is one author's.
#[derive(Default)]
struct Saves {
    rev: u64,
    /// The author of the latest save.
    last: String,
    /// The latest revision saved by anyone but `last`; 0 when nobody else saved.
    other: u64,
}

impl Saves {
    fn record(&mut self, actor: &str) -> u64 {
        if self.last != actor {
            self.other = self.rev;
            self.last = actor.to_string();
        }
        self.rev += 1;
        self.rev
    }

    /// Whether `actor` alone saved the project after revision `base`: writing back a state taken at
    /// `base` then loses nobody else's save. A revision the studio has not reached (one from before
    /// it restarted) is nobody's.
    fn only_by_since(&self, actor: &str, base: u64) -> bool {
        let others = if self.last == actor { self.other } else { self.rev };
        base <= self.rev && others <= base
    }
}

fn revisions() -> std::sync::MutexGuard<'static, HashMap<String, Saves>> {
    static REVISIONS: OnceLock<Mutex<HashMap<String, Saves>>> = OnceLock::new();
    REVISIONS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn revision(pid: &str) -> u64 {
    revisions().get(pid).map(|saves| saves.rev).unwrap_or(0)
}

/// Work a request hands to a blocking thread, run in the request's scope there: a save it makes is
/// the request's author's, not a job's.
#[allow(dead_code)]
pub(crate) fn carry<T>(work: impl FnOnce() -> T + Send + 'static) -> impl FnOnce() -> T + Send + 'static {
    let carried = REQUEST.try_with(Scope::clone).ok();
    move || match carried {
        Some(scope) => REQUEST.sync_scope(scope, work),
        None => work(),
    }
}

/// A job, taken when a request queues it and run as that request author's job: its saves are
/// theirs, marked `job`, whoever queued the project's next job before it ran. JobQueue::enqueue
/// wraps every job in it.
pub(crate) fn carry_job(job: JobFn) -> JobFn {
    let actor = REQUEST.try_with(|scope| scope.actor.clone()).unwrap_or_else(|_| "studio".into());
    Box::new(move |progress| REQUEST.sync_scope(Scope::of_job(actor), move || job(progress)))
}

/// Writes a project through `write` as its next revision and tells the windows who saved it: the
/// window or agent whose request or job is being served (`studio` outside both; a job's save marked
/// `job`). A whole-project PUT whose state was taken before someone else's save is refused before
/// anything is written.
pub(crate) fn save_with_revision(dir: &Path, write: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let pid = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let (actor, expected, job) = REQUEST.try_with(|scope| (scope.actor.clone(), scope.expected, scope.job)).unwrap_or_else(|_| ("studio".into(), None, false));
    let rev = {
        let mut revisions = revisions();
        let saves = revisions.entry(pid.clone()).or_default();
        if expected.is_some_and(|base| !saves.only_by_since(&actor, base)) {
            let _ = REQUEST.try_with(|scope| scope.conflict.store(true, Ordering::Relaxed));
            return Err(format!("{PROJECT_CHANGED}: project {pid} is at revision {}", saves.rev));
        }
        write()?;
        saves.record(&actor)
    };
    let _ = REQUEST.try_with(|scope| scope.saved.store(rev, Ordering::Relaxed));
    tell_windows(json!({ "changed": "project", "pid": pid, "rev": rev, "by": actor, "job": job }));
    Ok(())
}

/// The request's author: a window of the page by its mark, an agent through the MCP tools, or
/// another program calling the API.
fn actor_of(headers: &HeaderMap) -> String {
    let mark = headers.get(WINDOW_HEADER).and_then(|value| value.to_str().ok()).filter(|mark| !mark.is_empty() && mark.len() <= 64 && mark.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
    match mark {
        Some(mark) => format!("window:{mark}"),
        None if headers.contains_key(AGENT_HEADER) => "agent".into(),
        None => "api".into(),
    }
}

/// The project a route is about: `/projects/{pid}` and everything under it.
fn project_of(path: &str) -> Option<String> {
    let mut parts = path.trim_matches('/').split('/');
    match (parts.next(), parts.next()) {
        (Some("projects"), Some(pid)) if !pid.is_empty() && pid.chars().all(|c| c.is_ascii_alphanumeric()) => Some(pid.to_string()),
        _ => None,
    }
}

/// Middleware on the studio's API: serves each request in the scope of its author, answers the
/// whole project with its revision, refuses a whole-project PUT that would lose someone else's
/// save, and tells the windows what a change made that is not a project's save: the projects, the
/// settings, the voices, the casting, a job started.
pub(crate) async fn track(request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let actor = actor_of(request.headers());
    let pid = project_of(&path);
    let depth = path.trim_matches('/').split('/').count();
    // the answers that are the project itself: read, edited, replaced
    let whole_project = pid.is_some() && depth == 2;
    let expected = match (&method, whole_project) {
        (&Method::PUT, true) => request.headers().get(REV_HEADER).and_then(|value| value.to_str().ok()).and_then(|value| value.trim().parse::<u64>().ok()),
        _ => None,
    };
    let before = pid.as_deref().map(revision);
    let scope = Scope::new(actor.clone(), expected);
    let (saved, conflict) = (scope.saved.clone(), scope.conflict.clone());
    let mut response = REQUEST.scope(scope, next.run(request)).await;
    if conflict.load(Ordering::Relaxed) {
        let detail = format!("the project was saved by someone else after the state this change was made on (it is at revision {}): read it again", pid.as_deref().map(revision).unwrap_or_default());
        return (StatusCode::CONFLICT, Json(json!({ "error": PROJECT_CHANGED, "detail": detail }))).into_response();
    }
    if whole_project && response.status().is_success() {
        let shown = if method == Method::GET { before } else { Some(saved.load(Ordering::Relaxed)).filter(|rev| *rev > 0) };
        if let Some(rev) = shown {
            response.headers_mut().insert(REV_HEADER, HeaderValue::from(rev));
        }
    }
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS || !response.status().is_success() {
        return response;
    }
    announce_route(&method, &path, pid.as_deref(), &actor, response).await
}

async fn announce_route(method: &Method, path: &str, pid: Option<&str>, actor: &str, response: Response) -> Response {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let what = match (method.as_str(), parts.as_slice()) {
        ("POST", ["projects"]) | ("DELETE", ["projects", _]) => Some("projects"),
        ("POST", ["projects", _, "casting"]) => Some("casting"),
        ("POST", ["projects", _, "casting", "library"]) | ("DELETE", ["casting", "library", _]) => Some("casting_library"),
        ("POST", ["voices", "get" | "rename" | "delete"]) | ("POST", ["projects", _, "speaker-voice"]) | ("POST", ["record", "stop"]) => Some("voices"),
        (_, ["engine", "select" | "preset" | "opts"])
        | (_, ["engine", "openrouter" | "proxy", "settings"])
        | (_, ["engine", "server", "key"])
        | ("POST", ["engine", "openrouter", "catalog", "refresh"])
        | (_, ["settings", "launch"])
        | ("POST", ["setup", "import" | "browse" | "remove"]) => Some("settings"),
        _ => None,
    };
    if let Some(what) = what {
        tell_windows(json!({ "changed": what, "pid": pid, "by": actor }));
    }
    let kind = match (method.as_str(), parts.as_slice()) {
        ("POST", ["projects", _, route]) if JOB_ROUTES.contains(route) => route.replace('-', "_"),
        ("POST", ["voices", "download-pack"]) => "voices_pack".into(),
        _ => return response,
    };
    let (head, body) = response.into_parts();
    // the answer is already whole in memory (a JSON reply); a ready result of detect-text can be large
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!("[ERROR] {method} {path}: its answer could not be read to tell the windows of its job: {error}");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("the answer of {method} {path} could not be read: {error}")).into_response();
        }
    };
    let answer = serde_json::from_slice::<Value>(&bytes).ok();
    // an answer without a job_id started none: the result was ready (separate, detect-text)
    if let Some(job_id) = answer.as_ref().and_then(|answer| answer.get("job_id")).and_then(Value::as_str) {
        let made = answer.as_ref().and_then(|answer| answer.get("project_id")).cloned().unwrap_or(Value::Null);
        tell_windows(json!({ "changed": "jobs", "job_id": job_id, "kind": kind, "pid": pid, "project_id": made, "by": actor }));
        if made.is_string() && made.as_str() != pid {
            tell_windows(json!({ "changed": "projects", "pid": made, "by": actor }));
        }
    }
    Response::from_parts(head, Body::from(bytes))
}

// ---------------------------------------------------------------- the tools

/// The tools that work in the studio's window, in front of the user.
pub(super) fn tools() -> Vec<Tool> {
    vec![
        // ---------------------------------------------------------------- the window: what the user sees
        Tool {
            name: "ui_screenshot",
            description: "A picture of the studio's window as the user sees it now. Use it to check what a change looks like, and before clicking anything. The window must be visible (not minimised); ui_read_page works either way.",
            schema: || object(json!({ "max_width": { "type": "integer", "description": "pixels, 1600 by default" } }), &[]),
            call: |args| window("screenshot", args, 30),
        },
        Tool {
            name: "ui_read_page",
            description: "Every visible control of the window, one per line: its ref (e12), kind, label, value and on/off. A row of a list reads as one line with its controls - `segment s12 0:14.2→0:17.0 SPK 1: ...: e40 open, e41 Play this line, e45 Translation=\"...\"` - and an open dialog (a confirmation, the settings) is listed first. A key or password field reads only as filled or empty. Refs are what ui_click, ui_type and ui_select take; read the page again after it changes.",
            schema: nothing,
            call: |args| window("read_page", args, 15),
        },
        Tool {
            name: "ui_click",
            description: "Click a control of the window like the user would: by ref from ui_read_page, or by its visible label in text. The control is highlighted for the user. Deleting a project or a saved cast asks in a dialog of the page: click its confirm button only when the user asked for the deletion.",
            schema: || object(json!({ "ref": { "type": "string" }, "text": { "type": "string" } }), &[]),
            call: |args| window("click", args, 15),
        },
        Tool {
            name: "ui_type",
            description: "Type into a field of the window (replaces its text) by ref or by its label in text; submit presses Enter after. A line's translation field saves when it loses focus, which this does.",
            schema: || object(json!({ "ref": { "type": "string" }, "text": { "type": "string", "description": "the field's label, when there is no ref" }, "value": { "type": "string" }, "submit": { "type": "boolean" } }), &["value"]),
            call: |args| window("type", args, 15),
        },
        Tool {
            name: "ui_press_key",
            description: "Press a key in the window, modifiers joined by +: Space or K plays and pauses the dub, ArrowLeft/ArrowRight seek 5 s (Shift+ 1 s, Ctrl+ 10 s), Home and End, ArrowUp/ArrowDown the volume, F the preview full screen, ? the shortcuts, Ctrl+K the command palette, Ctrl+Z and Ctrl+Shift+Z undo and redo, Escape closes a dialog, Enter, Tab.",
            schema: || id_only("key", "the key"),
            call: |args| window("press_key", args, 15),
        },
        Tool {
            name: "ui_scroll",
            description: "Scroll the page (direction down or up, amount in pixels), or bring a control into view by ref or text.",
            schema: || object(json!({ "direction": { "type": "string", "enum": ["down", "up"] }, "amount": { "type": "integer" }, "ref": { "type": "string" }, "text": { "type": "string" } }), &[]),
            call: |args| window("scroll", args, 15),
        },
        Tool {
            name: "ui_notify",
            description: "Show the user a short message in the studio's window for a few seconds and put it into its activity log: what you did, what you need from them. tone: info (default), success or error.",
            schema: || object(json!({ "text": { "type": "string" }, "tone": { "type": "string", "enum": ["info", "success", "error"] } }), &["text"]),
            call: |args| window("notify", args, 15),
        },
        // ---------------------------------------------------------------- the editor, in front of the user
        Tool {
            name: "editor_open",
            description: "Open a project in the studio's window - its editor, or the transcript view for a transcript - for the user to watch what you do next. The editor_* tools work on the project open there.",
            schema: project_only,
            call: |args| window("editor_open", args, 20),
        },
        Tool {
            name: "editor_seek",
            description: "Move the editor's playhead to seconds, or to a line's start (segment_id), and wait until its frame is on screen (up to 25 s while the graphics card is busy; frame says whether it arrived). The list scrolls to the line there.",
            schema: || object(json!({ "seconds": { "type": "number" }, "segment_id": { "type": "string" } }), &[]),
            call: |args| window("editor_seek", args, 30),
        },
        Tool {
            name: "editor_select",
            description: "Show the user a thing in the editor: a line (segment_id; segment_ids ticks several for the list's bulk actions), a blur box (blur_idx) or a title (title_idx). The lane switches to it, the playhead moves to its start, the list scrolls to it and it is highlighted.",
            schema: || object(json!({ "segment_id": { "type": "string" }, "segment_ids": ids("line ids to tick"), "blur_idx": { "type": "integer" }, "title_idx": { "type": "integer" } }), &[]),
            call: |args| window("editor_select", args, 15),
        },
        Tool {
            name: "editor_play",
            description: "Play the dub in the editor for the user to hear: one line (segment_id) from its start to its end, or from a moment (from, seconds; the playhead when left out) on. Lines play their last voiced take: project_dub_audio voices the dirty ones first.",
            schema: || object(json!({ "segment_id": { "type": "string" }, "from": { "type": "number" } }), &[]),
            call: |args| window("editor_play", args, 15),
        },
        Tool {
            name: "editor_pause",
            description: "Stop the editor's playback.",
            schema: nothing,
            call: |args| window("editor_pause", args, 15),
        },
        Tool {
            name: "editor_undo",
            description: "Undo the last edit in the window, as Ctrl+Z does.",
            schema: nothing,
            call: |args| window("editor_undo", args, 15),
        },
        Tool {
            name: "editor_redo",
            description: "Redo the edit undone last in the window.",
            schema: nothing,
            call: |args| window("editor_redo", args, 15),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempfile;

    /// The bridge is one for the whole process: tests that open windows or wait for answers take turns.
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The next message of the stream as JSON, within a second.
    async fn next(receiver: &mut tokio::sync::broadcast::Receiver<String>) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await.expect("a message within a second").expect("the stream is open");
        serde_json::from_str(&message).unwrap()
    }

    /// The next message about project `pid`, skipping the other tests' notices.
    async fn next_about(receiver: &mut tokio::sync::broadcast::Receiver<String>, pid: &str) -> Value {
        loop {
            let message = next(receiver).await;
            if message["pid"] == pid || message["project_id"] == pid {
                return message;
            }
        }
    }

    #[tokio::test]
    async fn a_command_reaches_the_window_and_its_answer_comes_back_by_id() {
        let _turn = TURN.lock().await;
        let mut page = bridge().commands.subscribe();
        let window = u64::MAX - 100;
        open_windows().push(window);
        let asked = tokio::spawn(ask_window("editor_state", json!({ "x": 1 }), 5));
        let command = loop {
            let message = next(&mut page).await;
            if message["command"] == "editor_state" {
                break message;
            }
        };
        assert_eq!((command["window"].as_u64(), command["args"]["x"].as_i64()), (Some(window), Some(1)));
        let id = command["id"].as_str().unwrap().to_string();
        let stranger = window_result(Json(WindowAnswer { id: "w-none".into(), result: json!(null), error: None })).await;
        assert_eq!(stranger, StatusCode::NOT_FOUND, "an answer to nothing asked is refused");
        let taken = window_result(Json(WindowAnswer { id: id.clone(), result: json!({ "pid": "p1" }), error: None })).await;
        assert_eq!(taken, StatusCode::NO_CONTENT);
        assert_eq!(asked.await.unwrap().unwrap(), json!({ "pid": "p1" }));
        assert!(!pending().contains_key(&id));

        let failing = tokio::spawn(ask_window("editor_undo", json!({}), 5));
        let command = loop {
            let message = next(&mut page).await;
            if message["command"] == "editor_undo" {
                break message;
            }
        };
        window_result(Json(WindowAnswer { id: command["id"].as_str().unwrap().into(), result: Value::Null, error: Some("nothing to undo".into()) })).await;
        assert_eq!(failing.await.unwrap().unwrap_err(), "nothing to undo", "the page's error is the tool's");
        open_windows().retain(|open| *open != window);
    }

    #[tokio::test]
    async fn a_window_that_does_not_answer_times_out_and_no_window_is_said_at_once() {
        let _turn = TURN.lock().await;
        let saved: Vec<u64> = std::mem::take(&mut *open_windows());
        let started = std::time::Instant::now();
        let none = ask_window("editor_state", json!({}), 30).await.unwrap_err();
        assert!(none.contains("window is not open") && started.elapsed() < Duration::from_secs(1), "{none}");

        let _page = bridge().commands.subscribe();
        let window = u64::MAX - 101;
        open_windows().push(window);
        let silent = ask_window("editor_frame", json!({}), 1).await.unwrap_err();
        assert!(silent.contains("did not answer 'editor_frame' within 1 s"), "{silent}");
        assert!(pending().is_empty(), "a command given up on is not left waiting");
        open_windows().retain(|open| *open != window);
        open_windows().extend(saved);
    }

    #[tokio::test]
    async fn a_window_tool_without_a_window_says_so_through_the_protocol() {
        let _turn = TURN.lock().await;
        let saved: Vec<u64> = std::mem::take(&mut *open_windows());
        let body = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "ui_click", "arguments": { "text": "Export" } } });
        let response = handle(HeaderMap::new(), axum::body::Bytes::from(body.to_string())).await;
        let reply: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        open_windows().extend(saved);
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("window is not open"), "{reply}");
    }

    #[tokio::test]
    async fn the_stream_names_the_window_first_and_forgets_it_when_it_closes() {
        let _turn = TURN.lock().await;
        let response = window_events().await;
        assert!(response.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/event-stream"));
        let mut stream = response.into_body().into_data_stream();
        let first = futures_util::StreamExt::next(&mut stream).await.unwrap().unwrap();
        let hello: Value = serde_json::from_str(String::from_utf8_lossy(&first).trim().trim_start_matches("data:").trim()).unwrap();
        let window = hello["window"].as_u64().expect("the first message names the window");
        assert_eq!(open_windows().last(), Some(&window));
        tell_windows(json!({ "changed": "voices", "by": "agent" }));
        let second = futures_util::StreamExt::next(&mut stream).await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&second).contains("\"changed\":\"voices\""));
        drop(stream);
        assert!(!open_windows().contains(&window), "a closed stream is no longer a window");
    }

    #[test]
    fn a_command_goes_to_the_window_the_person_turned_to() {
        let _turn = TURN.blocking_lock();
        let (older, newer) = (u64::MAX - 20, u64::MAX - 21);
        open_windows().extend([older, newer]);
        let place = |window: u64| open_windows().iter().position(|open| *open == window).unwrap();
        assert!(place(newer) > place(older), "the window opened last is asked first");
        assert!(focus_window(older));
        assert!(place(older) > place(newer), "the one the person turned to is asked now");
        assert!(!focus_window(u64::MAX - 22), "a window that is not open is not taken");
        open_windows().retain(|open| *open != older && *open != newer);
    }

    #[test]
    fn every_window_tool_asks_the_window_and_no_other_tool_does() {
        const BUILT_IN: &[&str] = &["screenshot", "read_page", "click", "type", "select", "press_key", "scroll", "console"];
        const APP: &[&str] = &["navigate", "open_settings", "open_help", "notify"];
        for tool in super::super::tools() {
            let windowed = tool.name.starts_with("ui_") || tool.name.starts_with("editor_");
            let schema = (tool.schema)();
            let args = json!({});
            match (tool.call)(&args) {
                Ok(Call { payload: Payload::Window { command, seconds, .. }, .. }) => {
                    assert!(windowed, "{} asks the window but is not named ui_ or editor_", tool.name);
                    assert!(seconds >= 15, "{} gives the window {seconds} s", tool.name);
                    if tool.name.starts_with("ui_") {
                        assert!(BUILT_IN.contains(&command) || APP.contains(&command), "{} asks for {command}, which the page does not know", tool.name);
                    } else {
                        assert_eq!(command, tool.name, "an editor tool asks for its own name");
                    }
                    assert!(schema["properties"].get("pid").is_none() || tool.name == "editor_open" || tool.name == "ui_navigate", "{} works on the open project, not a pid", tool.name);
                }
                _ => assert!(!windowed, "{} is named for the window but does not ask it", tool.name),
            }
        }
    }

    #[test]
    fn window_tools_that_only_look_are_read_only() {
        for name in ["ui_screenshot", "ui_read_page", "project_transcript"] {
            assert_eq!(annotations(name)["readOnlyHint"], true, "{name}");
        }
        for name in ["ui_click", "ui_type", "editor_open", "editor_undo", "segment_split"] {
            assert_eq!(annotations(name)["readOnlyHint"], false, "{name}");
        }
        for name in ["segments_merge"] {
            assert_eq!(annotations(name)["destructiveHint"], true, "{name}");
        }
    }

    /// A studio in miniature behind the middleware: a project saved by PATCH and PUT, and a job.
    fn tracked(workspace: PathBuf) -> Router {
        use axum::extract::Path as Segment;
        use axum::routing::{get, post};
        let save = move |pid: String| {
            let dir = workspace.join(&pid);
            std::fs::create_dir_all(&dir).unwrap();
            match save_with_revision(&dir, || std::fs::write(dir.join("project.json"), b"{}").map_err(|error| error.to_string())) {
                Ok(()) => Json(json!({ "segments": [] })).into_response(),
                Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
            }
        };
        let (put_save, patch_save) = (save.clone(), save);
        Router::new()
            .route(
                "/projects/{pid}",
                get(|| async { Json(json!({ "segments": [] })) })
                    .patch(move |Segment(pid): Segment<String>| std::future::ready(patch_save(pid)))
                    .put(move |Segment(pid): Segment<String>| std::future::ready(put_save(pid))),
            )
            .route("/projects/{pid}/render", post(|| async { Json(json!({ "job_id": "j9" })) }))
            .route("/projects/{pid}/files", get(|| async { Json(json!({ "dir": "x" })) }))
            .route("/voices/rename", post(|| async { Json(json!({ "voices": [] })) }))
            .layer(axum::middleware::from_fn(track))
    }

    async fn send(router: &Router, method: Method, path: &str, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        router.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn a_save_raises_the_revision_and_tells_the_windows_who_made_it() {
        let folder = tempfile::tempdir().unwrap();
        let router = tracked(folder.path().to_path_buf());
        let mut page = bridge().commands.subscribe();
        let pid = "revtest1";

        let read = send(&router, Method::GET, "/projects/revtest1", &[]).await;
        assert_eq!(read.headers()[REV_HEADER], "0", "a project never saved is at revision 0");
        let files = send(&router, Method::GET, "/projects/revtest1/files", &[]).await;
        assert!(files.headers().get(REV_HEADER).is_none(), "only an answer that is the project names its revision");
        let edited = send(&router, Method::PATCH, "/projects/revtest1", &[(WINDOW_HEADER, "tab-1")]).await;
        assert_eq!(edited.headers()[REV_HEADER], "1");
        assert_eq!(next_about(&mut page, pid).await, json!({ "changed": "project", "pid": pid, "rev": 1, "by": "window:tab-1", "job": false }));

        let by_agent = send(&router, Method::PATCH, "/projects/revtest1", &[(AGENT_HEADER, "1")]).await;
        assert_eq!(by_agent.headers()[REV_HEADER], "2");
        assert_eq!(next_about(&mut page, pid).await["by"], "agent");

        let stale = send(&router, Method::PUT, "/projects/revtest1", &[(WINDOW_HEADER, "tab-1"), (REV_HEADER, "1")]).await;
        assert_eq!(stale.status(), StatusCode::CONFLICT, "an undo made before the agent's edit would erase it");
        let refused: Value = serde_json::from_slice(&axum::body::to_bytes(stale.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(refused["error"], PROJECT_CHANGED);
        assert_eq!(revision(pid), 2, "nothing was written");

        let fresh = send(&router, Method::PUT, "/projects/revtest1", &[(WINDOW_HEADER, "tab-1"), (REV_HEADER, "2")]).await;
        assert_eq!((fresh.status(), fresh.headers()[REV_HEADER].to_str().unwrap()), (StatusCode::OK, "3"));
        assert_eq!(next_about(&mut page, pid).await["rev"], 3);
        let unmarked = send(&router, Method::PUT, "/projects/revtest1", &[]).await;
        assert_eq!(unmarked.status(), StatusCode::OK, "a PUT without a revision is taken as it is");
        assert_eq!(next_about(&mut page, pid).await["by"], "api");
    }

    #[tokio::test]
    async fn an_undo_under_the_window_s_own_later_edit_still_keeps_the_agent_s_save() {
        let folder = tempfile::tempdir().unwrap();
        let router = tracked(folder.path().to_path_buf());
        let status = |response: Response| (response.status(), response.headers().get(REV_HEADER).map(|rev| rev.to_str().unwrap().to_string()));
        let tab = [(WINDOW_HEADER, "tab-1")];
        let with = |rev: &'static str| [(WINDOW_HEADER, "tab-1"), (REV_HEADER, rev)];

        assert_eq!(status(send(&router, Method::PATCH, "/projects/revtest2", &tab).await), (StatusCode::OK, Some("1".into())));
        assert_eq!(status(send(&router, Method::PUT, "/projects/revtest2", &with("0")).await), (StatusCode::OK, Some("2".into())), "the window undoes its own edit");

        send(&router, Method::PATCH, "/projects/revtest2", &[(AGENT_HEADER, "1")]).await;
        // the window's next edit is saved on top of the agent's, and its answer brings the window revision 4
        assert_eq!(status(send(&router, Method::PATCH, "/projects/revtest2", &tab).await), (StatusCode::OK, Some("4".into())));
        assert_eq!(send(&router, Method::PUT, "/projects/revtest2", &with("2")).await.status(), StatusCode::CONFLICT, "the snapshot taken before that edit lacks the agent's save");
        assert_eq!(revision("revtest2"), 4, "nothing was written");
        assert_eq!(status(send(&router, Method::PUT, "/projects/revtest2", &with("3")).await), (StatusCode::OK, Some("5".into())), "a snapshot that has the agent's save undoes the window's edit");

        send(&router, Method::PATCH, "/projects/revtest2", &[(WINDOW_HEADER, "tab-2")]).await;
        assert_eq!(send(&router, Method::PUT, "/projects/revtest2", &with("5")).await.status(), StatusCode::CONFLICT, "another window saved after it");
        assert_eq!(send(&router, Method::PUT, "/projects/revtest2", &with("99")).await.status(), StatusCode::CONFLICT, "a revision the studio never reached");
        let other = send(&router, Method::PUT, "/projects/revtest2", &[(WINDOW_HEADER, "tab-2"), (REV_HEADER, "5")]).await;
        assert_eq!(status(other), (StatusCode::OK, Some("7".into())), "the other window undoes its own edit");
    }

    #[test]
    fn saves_tell_whether_one_author_alone_saved_after_a_revision() {
        let mut saves = Saves::default();
        assert!(saves.only_by_since("window:a", 0), "a project nobody saved");
        saves.record("window:a");
        saves.record("window:a");
        assert!(saves.only_by_since("window:a", 0) && !saves.only_by_since("agent", 0) && saves.only_by_since("agent", 2));
        saves.record("agent");
        saves.record("window:a");
        assert_eq!(saves.rev, 4);
        assert!(!saves.only_by_since("window:a", 2) && saves.only_by_since("window:a", 3));
        assert!(!saves.only_by_since("agent", 3) && saves.only_by_since("agent", 4));
        assert!(!saves.only_by_since("window:a", 5));
    }

    #[tokio::test]
    async fn a_job_and_a_list_change_reach_the_windows() {
        let folder = tempfile::tempdir().unwrap();
        let router = tracked(folder.path().to_path_buf());
        let mut page = bridge().commands.subscribe();
        let started = send(&router, Method::POST, "/projects/jobtest1/render", &[(AGENT_HEADER, "1")]).await;
        let answer: Value = serde_json::from_slice(&axum::body::to_bytes(started.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(answer["job_id"], "j9", "the answer reaches the caller whole");
        let notice = next_about(&mut page, "jobtest1").await;
        assert_eq!((notice["changed"].as_str(), notice["kind"].as_str(), notice["job_id"].as_str(), notice["by"].as_str()), (Some("jobs"), Some("render"), Some("j9"), Some("agent")));
        send(&router, Method::POST, "/voices/rename", &[(WINDOW_HEADER, "tab-2")]).await;
        let voices = loop {
            let message = next(&mut page).await;
            if message["changed"] == "voices" && message["by"] == "window:tab-2" {
                break message;
            }
        };
        assert!(voices["pid"].is_null());

        let nobody = folder.path().join("jobtest2");
        std::fs::create_dir_all(&nobody).unwrap();
        save_with_revision(&nobody, || Ok(())).unwrap();
        let studio = next_about(&mut page, "jobtest2").await;
        assert_eq!((studio["by"].as_str(), studio["job"].as_bool()), (Some("studio"), Some(false)), "a save outside any request and any job is the studio's");
    }

    #[tokio::test]
    async fn a_job_s_save_is_its_author_s_whoever_queued_the_next_job_of_the_project() {
        use axum::extract::Path as Segment;
        use axum::routing::post;
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_path_buf();
        let queue = crate::jobs::JobQueue::new();
        // the first job waits until the test lets it save: the window queues its own job meanwhile
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(Some(gate)));
        let router = Router::new()
            .route(
                "/projects/{pid}/render",
                post(move |Segment(pid): Segment<String>| {
                    let (queue, dir, gate) = (queue.clone(), workspace.join(pid), gate.clone());
                    async move {
                        std::fs::create_dir_all(&dir).unwrap();
                        let wait = gate.lock().unwrap().take();
                        let job: JobFn = Box::new(move |_progress| {
                            if let Some(gate) = wait {
                                gate.recv().unwrap();
                            }
                            save_with_revision(&dir, || Ok(())).map(|()| Value::Null)
                        });
                        Json(json!({ "job_id": queue.enqueue(job).await }))
                    }
                }),
            )
            .layer(axum::middleware::from_fn(track));
        let mut page = bridge().commands.subscribe();
        send(&router, Method::POST, "/projects/jobtest3/render", &[(AGENT_HEADER, "1")]).await;
        send(&router, Method::POST, "/projects/jobtest3/render", &[(WINDOW_HEADER, "tab-3")]).await;
        release.send(()).unwrap();
        let mut saves = Vec::new();
        while saves.len() < 2 {
            let message = next_about(&mut page, "jobtest3").await;
            if message["changed"] == "project" {
                saves.push((message["by"].as_str().unwrap().to_string(), message["job"].as_bool(), message["rev"].as_u64()));
            }
        }
        assert_eq!(saves[0], ("agent".to_string(), Some(true), Some(1)), "the agent's job saves as the agent's, though the window queued a job before it ran");
        assert_eq!(saves[1], ("window:tab-3".to_string(), Some(true), Some(2)));
    }

    #[tokio::test]
    async fn a_request_s_blocking_work_saves_as_its_author() {
        let folder = tempfile::tempdir().unwrap();
        let dir = folder.path().join("carrytest1");
        std::fs::create_dir_all(&dir).unwrap();
        let mut page = bridge().commands.subscribe();
        let scope = Scope::new("window:tab-9".into(), None);
        REQUEST
            .scope(scope, async move {
                let work = carry(move || save_with_revision(&dir, || Ok(())));
                tokio::task::spawn_blocking(work).await.unwrap().unwrap();
            })
            .await;
        let saved = next_about(&mut page, "carrytest1").await;
        assert_eq!((saved["by"].as_str(), saved["job"].as_bool()), (Some("window:tab-9"), Some(false)));
    }

    #[test]
    fn a_request_is_told_apart_by_its_mark() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in pairs {
                map.insert(*name, value.parse().unwrap());
            }
            map
        };
        assert_eq!(actor_of(&headers(&[(WINDOW_HEADER, "5f0c-11ab")])), "window:5f0c-11ab");
        assert_eq!(actor_of(&headers(&[(AGENT_HEADER, "1")])), "agent");
        assert_eq!(actor_of(&headers(&[])), "api");
        assert_eq!(actor_of(&headers(&[(WINDOW_HEADER, "a b\"")])), "api", "a mark that is not a token is no window");
        assert_eq!(project_of("/projects/abc123/render").as_deref(), Some("abc123"));
        assert_eq!(project_of("/projects"), None);
        assert_eq!(project_of("/projects/../x"), None);
    }
}
