//! DubStudio as an MCP server: Streamable HTTP & JSON-RPC 2.0 at `/mcp`.
//!
//! Tools execute natively against DubStudio's axum router (the same routes the GUI talks to),
//! so an agent works directly with project state: inspecting segments, translating text,
//! splitting and merging phrases, adjusting timings, tuning TTS and re-synthesizing audio.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower::ServiceExt;

/// Studio's API router, installed once the server builds it.
static API: OnceLock<Router> = OnceLock::new();

pub fn install(api: Router) {
    let _ = API.set(api);
}

const SERVER_NAME: &str = "dubstudio";
const SERVER_VERSION: &str = "3.2.0";
const INSTRUCTIONS: &str = "Drive DubStudio on this computer through its MCP server: inspect dubbing projects, read and translate segments with context, split and merge phrases, fit speech pacing (CPS), fine-tune TTS temperatures and seeds, and trigger selective re-synthesis.";

struct Agent {
    last_call: Mutex<Option<(std::time::Instant, String)>>,
    calls: AtomicU64,
}

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| Agent {
        last_call: Mutex::new(None),
        calls: AtomicU64::new(0),
    })
}

const AGENT_PRESENT: Duration = Duration::from_secs(600);

fn seen(what: &str) {
    *agent().last_call.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((std::time::Instant::now(), what.to_string()));
    agent().calls.fetch_add(1, Ordering::Relaxed);
}

fn agent_present() -> bool {
    agent()
        .last_call
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < AGENT_PRESENT)
}

/// GET /mcp/status — whether an agent is connected, for the settings page.
pub async fn status() -> Json<Value> {
    let last = agent()
        .last_call
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    Json(json!({
        "server_name": SERVER_NAME,
        "agent_connected": agent_present(),
        "agent_last_call": last.as_ref().map(|(_, what)| what.clone()),
        "agent_seconds_ago": last.as_ref().map(|(at, _)| at.elapsed().as_secs()),
        "agent_calls": agent().calls.load(Ordering::Relaxed),
    }))
}

enum Payload {
    None,
    Json(Value),
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
    call: fn(&Value) -> Call,
}

#[allow(dead_code)]
fn get(path: String) -> Call {
    Call {
        method: Method::GET,
        path,
        payload: Payload::None,
    }
}

fn post(path: String, body: Value) -> Call {
    Call {
        method: Method::POST,
        path,
        payload: Payload::Json(body),
    }
}

fn patch(path: String, body: Value) -> Call {
    Call {
        method: Method::PATCH,
        path,
        payload: Payload::Json(body),
    }
}

fn composite(kind: &'static str) -> Call {
    Call {
        method: Method::GET,
        path: format!("composite:{kind}"),
        payload: Payload::None,
    }
}

async fn call_route(call: Call) -> Result<(StatusCode, String), String> {
    let api = API.get().ok_or("DubStudio API router is not ready yet")?.clone();
    let builder = Request::builder().method(call.method).uri(&call.path);
    let request = match call.payload {
        Payload::None => builder.body(Body::empty()),
        Payload::Json(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
    }
    .map_err(|e| e.to_string())?;

    let response = api.oneshot(request).await.map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .map_err(|e| e.to_string())?;
    let text = match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_default(),
        Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
    };
    Ok((status, text))
}

async fn fetch_json(path: &str) -> Result<Value, String> {
    let (status, text) = call_route(Call {
        method: Method::GET,
        path: path.into(),
        payload: Payload::None,
    })
    .await?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("parse JSON from {path}: {e}"))
}

async fn resolve_project_id(given: Option<&str>) -> Result<String, String> {
    if let Some(id) = given.filter(|s| !s.trim().is_empty()) {
        return Ok(id.trim().to_string());
    }
    let list = fetch_json("/projects").await?;
    let projects = list
        .get("projects")
        .and_then(Value::as_array)
        .or_else(|| list.as_array())
        .ok_or("unexpected /projects response")?;
    if projects.is_empty() {
        return Err("No projects found in DubStudio workspace. Open or upload a video first.".into());
    }
    // Return latest project (sorted by mtime descending in list_projects)
    let latest = &projects[0];
    let pid = latest
        .get("pid")
        .or_else(|| latest.get("id"))
        .or_else(|| latest.get("project_id"))
        .and_then(Value::as_str)
        .ok_or("project has no id")?;
    Ok(pid.to_string())
}

// ─── Tools Definition ────────────────────────────────────────────────────────

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        vec![
            Tool {
                name: "dub_status",
                description: "Get current DubStudio status: active project, capabilities, installed models, and GPU resources.",
                schema: || json!({ "type": "object", "properties": {} }),
                call: |_| composite("status"),
            },
            Tool {
                name: "dub_get_project",
                description: "Get metadata for a project (or the active project if project_id is omitted): duration, video file, languages, and settings.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional; defaults to latest open project)" }
                }), &[]),
                call: |_| composite("get_project"),
            },
            Tool {
                name: "dub_get_segments",
                description: "List subtitle/dubbing segments with computed speech rate (CPS - characters per second) and timing pacing status (ok, too_fast, too_slow).",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "offset": { "type": "integer", "description": "Starting index (default 0)" },
                    "limit": { "type": "integer", "description": "Max segments to return (default 30)" },
                    "only_dirty": { "type": "boolean", "description": "Only return modified segments that need re-synthesis" }
                }), &[]),
                call: |_| composite("get_segments"),
            },
            Tool {
                name: "dub_get_segment_context",
                description: "Get a specific segment with surrounding dialogue context (preceding and following phrases) for accurate conversational translation.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_id": { "type": "string", "description": "Target segment ID" },
                    "context_size": { "type": "integer", "description": "Number of neighbor phrases to include (default 2)" }
                }), &["segment_id"]),
                call: |_| composite("get_segment_context"),
            },
            Tool {
                name: "dub_update_segment",
                description: "Update translated text (tgt_text), source text, speaker or voice for a segment. Automatically marks segment as dirty for TTS and returns updated CPS pacing.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_id": { "type": "string", "description": "Target segment ID" },
                    "tgt_text": { "type": "string", "description": "New translated text" },
                    "src_text": { "type": "string", "description": "New source text (optional)" },
                    "speaker": { "type": "string", "description": "Speaker label (optional)" },
                    "voice": { "type": "string", "description": "Assigned voice name (optional)" }
                }), &["segment_id"]),
                call: |_| composite("update_segment"),
            },
            Tool {
                name: "dub_split_segment",
                description: "Split a segment into two parts at split_at (seconds timestamp). Distributes text between part 1 and part 2, marks both for re-synthesis.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_id": { "type": "string", "description": "Target segment ID to cut" },
                    "split_at": { "type": "number", "description": "Exact timestamp in seconds where to split (between start and end)" },
                    "tgt_text_1": { "type": "string", "description": "Translated text for the 1st half" },
                    "tgt_text_2": { "type": "string", "description": "Translated text for the 2nd half" },
                    "src_text_1": { "type": "string", "description": "Source text for 1st half (optional)" },
                    "src_text_2": { "type": "string", "description": "Source text for 2nd half (optional)" }
                }), &["segment_id", "split_at", "tgt_text_1", "tgt_text_2"]),
                call: |_| composite("split_segment"),
            },
            Tool {
                name: "dub_merge_segments",
                description: "Merge two or more consecutive segments into a single segment. Combines start and end time windows and concatenates text.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_ids": { "type": "array", "items": { "type": "string" }, "description": "List of 2+ segment IDs in sequence to merge" },
                    "combined_tgt_text": { "type": "string", "description": "Explicit combined translated text (optional)" }
                }), &["segment_ids"]),
                call: |_| composite("merge_segments"),
            },
            Tool {
                name: "dub_adjust_timing",
                description: "Micro-adjust segment boundaries (start / end in seconds) or shift the entire segment window by shift_seconds.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_id": { "type": "string", "description": "Target segment ID" },
                    "start": { "type": "number", "description": "New start timestamp (optional)" },
                    "end": { "type": "number", "description": "New end timestamp (optional)" },
                    "shift_seconds": { "type": "number", "description": "Delta to shift both start and end by (optional)" }
                }), &["segment_id"]),
                call: |_| composite("adjust_timing"),
            },
            Tool {
                name: "dub_tune_tts",
                description: "Fine-tune TTS generation parameters for specific phrase(s): Higgs temperature and seed, or VoxCPM2 prompt, CFG scale and diffusion steps.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "segment_id": { "type": "string", "description": "Target segment ID (optional; if omitted sets project-wide audio defaults)" },
                    "higgs_temp": { "type": "number", "description": "Higgs sampling temperature (e.g. 0.65 - 1.00)" },
                    "higgs_seed": { "type": "integer", "description": "Fixed Higgs RNG seed" },
                    "vox_prompt": { "type": "string", "description": "Style/emotion prompt for VoxCPM2 (e.g. 'whispering, slow pace')" },
                    "vox_cfg": { "type": "number", "description": "CFG guidance scale for VoxCPM2 (1.0 - 3.0)" },
                    "vox_steps": { "type": "integer", "description": "Diffusion steps for VoxCPM2 (10 - 50)" },
                    "vox_seed": { "type": "integer", "description": "Fixed VoxCPM2 seed" }
                }), &[]),
                call: |_| composite("tune_tts"),
            },
            Tool {
                name: "dub_synth_segments",
                description: "Resynthesize speech only for modified (dirty) segments. Returns background job ID.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" }
                }), &[]),
                call: |_| composite("synth_segments"),
            },
            Tool {
                name: "dub_mix_audio",
                description: "Mix down all synthesized phrases and original audio into the master dub_audio track.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" }
                }), &[]),
                call: |_| composite("mix_audio"),
            },
            Tool {
                name: "dub_retranslate",
                description: "Trigger DubStudio's internal MT engine (Gemma / OpenRouter) to re-translate existing segments with an optional style prompt.",
                schema: || object(json!({
                    "project_id": { "type": "string", "description": "Project ID (optional)" },
                    "style": { "type": "string", "description": "Style instruction (e.g. 'formal', 'conversational', 'humorous', 'condensed')" }
                }), &[]),
                call: |_| composite("retranslate"),
            }
        ]
    })
}

// ─── Composite Tool Handlers ────────────────────────────────────────────────

fn compute_cps(text: &str, start: f64, end: f64) -> (f64, &'static str) {
    let dur = (end - start).max(0.1);
    let chars = text.chars().count();
    let cps = (chars as f64 / dur * 10.0).round() / 10.0;
    let pacing = if cps > 16.5 {
        "too_fast"
    } else if cps < 8.0 && chars > 0 {
        "too_slow"
    } else {
        "ok"
    };
    (cps, pacing)
}

async fn handle_composite(kind: &str, args: &Value) -> Result<Value, String> {
    match kind {
        "status" => {
            let caps = fetch_json("/engine/capabilities").await.unwrap_or(json!({}));
            let projects_val = fetch_json("/projects").await.unwrap_or(json!({}));
            let projects_count = projects_val
                .get("projects")
                .and_then(Value::as_array)
                .or_else(|| projects_val.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            let hw = fetch_json("/hw/snapshot").await.unwrap_or(json!({}));
            let active_id = resolve_project_id(None).await.ok();
            Ok(json!({
                "status": "ready",
                "active_project_id": active_id,
                "projects_count": projects_count,
                "capabilities": caps,
                "hardware": hw
            }))
        }
        "get_project" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let proj = fetch_json(&format!("/projects/{pid}")).await?;
            let seg_count = proj["segments"].as_array().map(Vec::len).unwrap_or(0);
            let dirty_count = proj["segments"].as_array().into_iter().flatten()
                .filter(|s| s["dirty"].as_bool().unwrap_or(false))
                .count();
            Ok(json!({
                "project_id": pid,
                "meta": proj["meta"],
                "src_lang": proj["src_lang"],
                "tgt_lang": proj["tgt_lang"],
                "mode": proj["mode"],
                "audio": proj["audio"],
                "total_segments": seg_count,
                "dirty_segments": dirty_count
            }))
        }
        "get_segments" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let proj = fetch_json(&format!("/projects/{pid}")).await?;
            let segments = proj["segments"].as_array().cloned().unwrap_or_default();
            let only_dirty = args.get("only_dirty").and_then(Value::as_bool).unwrap_or(false);
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;

            let filtered: Vec<Value> = segments.into_iter()
                .filter(|s| !only_dirty || s["dirty"].as_bool().unwrap_or(false))
                .skip(offset)
                .take(limit)
                .map(|mut s| {
                    let start = s["start"].as_f64().unwrap_or(0.0);
                    let end = s["end"].as_f64().unwrap_or(0.0);
                    let tgt = s["tgt_text"].as_str().unwrap_or("");
                    let (cps, pacing) = compute_cps(tgt, start, end);
                    s["cps"] = json!(cps);
                    s["pacing"] = json!(pacing);
                    s["duration"] = json!(((end - start) * 100.0).round() / 100.0);
                    s
                })
                .collect();

            Ok(json!({
                "project_id": pid,
                "offset": offset,
                "count": filtered.len(),
                "segments": filtered
            }))
        }
        "get_segment_context" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let target_id = args.get("segment_id").and_then(Value::as_str).ok_or("missing segment_id")?;
            let csize = args.get("context_size").and_then(Value::as_u64).unwrap_or(2) as usize;
            let proj = fetch_json(&format!("/projects/{pid}")).await?;
            let segments = proj["segments"].as_array().cloned().unwrap_or_default();
            let idx = segments.iter().position(|s| s["id"] == target_id)
                .ok_or_else(|| format!("segment {target_id} not found"))?;

            let start_idx = idx.saturating_sub(csize);
            let end_idx = (idx + csize + 1).min(segments.len());
            let context = &segments[start_idx..end_idx];

            Ok(json!({
                "project_id": pid,
                "target_segment": segments[idx],
                "context_dialogue": context
            }))
        }
        "update_segment" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let mut edit = json!({ "op": "segment", "id": args["segment_id"] });
            if let Some(t) = args.get("tgt_text") { edit["tgt_text"] = t.clone(); }
            if let Some(t) = args.get("src_text") { edit["src_text"] = t.clone(); }
            if let Some(t) = args.get("speaker") { edit["speaker"] = t.clone(); }
            if let Some(t) = args.get("voice") { edit["voice"] = t.clone(); }

            let (status, text) = call_route(patch(format!("/projects/{pid}"), edit)).await?;
            if !status.is_success() {
                return Err(format!("update segment failed: {text}"));
            }
            Ok(json!({ "ok": true, "project_id": pid, "segment_id": args["segment_id"] }))
        }
        "split_segment" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let mut edit = json!({
                "op": "split_segment",
                "id": args["segment_id"],
                "split_at": args["split_at"],
                "tgt_text_1": args["tgt_text_1"],
                "tgt_text_2": args["tgt_text_2"]
            });
            if let Some(t) = args.get("src_text_1") { edit["src_text_1"] = t.clone(); }
            if let Some(t) = args.get("src_text_2") { edit["src_text_2"] = t.clone(); }

            let (status, text) = call_route(patch(format!("/projects/{pid}"), edit)).await?;
            if !status.is_success() {
                return Err(format!("split segment failed: {text}"));
            }
            Ok(json!({ "ok": true, "project_id": pid, "split_at": args["split_at"] }))
        }
        "merge_segments" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let mut edit = json!({
                "op": "merge_segments",
                "ids": args["segment_ids"]
            });
            if let Some(t) = args.get("combined_tgt_text") { edit["combined_tgt_text"] = t.clone(); }

            let (status, text) = call_route(patch(format!("/projects/{pid}"), edit)).await?;
            if !status.is_success() {
                return Err(format!("merge segments failed: {text}"));
            }
            Ok(json!({ "ok": true, "project_id": pid, "merged_ids": args["segment_ids"] }))
        }
        "adjust_timing" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let sid = args.get("segment_id").and_then(Value::as_str).ok_or("missing segment_id")?;
            let mut edit = json!({ "op": "segment", "id": sid });
            if let Some(shift) = args.get("shift_seconds").and_then(Value::as_f64) {
                let proj = fetch_json(&format!("/projects/{pid}")).await?;
                let seg = proj["segments"].as_array().and_then(|a| a.iter().find(|s| s["id"] == sid))
                    .ok_or_else(|| format!("segment {sid} not found"))?;
                let start = (seg["start"].as_f64().unwrap_or(0.0) + shift).max(0.0);
                let end = (seg["end"].as_f64().unwrap_or(0.0) + shift).max(start + 0.1);
                edit["start"] = json!(start);
                edit["end"] = json!(end);
            } else {
                if let Some(st) = args.get("start") { edit["start"] = st.clone(); }
                if let Some(en) = args.get("end") { edit["end"] = en.clone(); }
            }

            let (status, text) = call_route(patch(format!("/projects/{pid}"), edit)).await?;
            if !status.is_success() {
                return Err(format!("adjust timing failed: {text}"));
            }
            Ok(json!({ "ok": true, "project_id": pid, "segment_id": sid }))
        }
        "tune_tts" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let mut edit = json!({ "op": "tts_settings" });
            if let Some(v) = args.get("higgs_temp") { edit["higgs_temp"] = v.clone(); }
            if let Some(v) = args.get("higgs_seed") { edit["higgs_seed"] = v.clone(); }
            if let Some(v) = args.get("vox_prompt") { edit["vox_prompt"] = v.clone(); }
            if let Some(v) = args.get("vox_cfg") { edit["vox_cfg"] = v.clone(); }
            if let Some(v) = args.get("vox_steps") { edit["vox_steps"] = v.clone(); }
            if let Some(v) = args.get("vox_seed") { edit["vox_seed"] = v.clone(); }

            let (status, text) = call_route(patch(format!("/projects/{pid}"), edit)).await?;
            if !status.is_success() {
                return Err(format!("tune TTS failed: {text}"));
            }
            // If segment_id is given, mark that segment as dirty
            if let Some(sid) = args.get("segment_id").and_then(Value::as_str) {
                let _ = call_route(patch(format!("/projects/{pid}"), json!({ "op": "regen", "id": sid }))).await;
            }
            Ok(json!({ "ok": true, "project_id": pid }))
        }
        "synth_segments" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let (status, text) = call_route(post(format!("/projects/{pid}/synth-segments"), json!({}))).await?;
            if !status.is_success() {
                return Err(format!("synth-segments failed: {text}"));
            }
            let res: Value = serde_json::from_str(&text).unwrap_or(json!({ "raw": text }));
            Ok(res)
        }
        "mix_audio" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let (status, text) = call_route(post(format!("/projects/{pid}/mix-audio"), json!({}))).await?;
            if !status.is_success() {
                return Err(format!("mix-audio failed: {text}"));
            }
            let res: Value = serde_json::from_str(&text).unwrap_or(json!({ "raw": text }));
            Ok(res)
        }
        "retranslate" => {
            let pid = resolve_project_id(args.get("project_id").and_then(Value::as_str)).await?;
            let style = args.get("style").and_then(Value::as_str).unwrap_or_default();
            if !style.is_empty() {
                let _ = call_route(patch(format!("/projects/{pid}"), json!({ "op": "translate_style", "style": style }))).await;
            }
            let (status, text) = call_route(post(format!("/projects/{pid}/retranslate"), json!({}))).await?;
            if !status.is_success() {
                return Err(format!("retranslate failed: {text}"));
            }
            let res: Value = serde_json::from_str(&text).unwrap_or(json!({ "raw": text }));
            Ok(res)
        }
        other => Err(format!("unknown composite operation {other}")),
    }
}

// ─── JSON-RPC 2.0 / Streamable HTTP Handlers ────────────────────────────────

fn rpc(id: Value, result: Value) -> Response {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
    .into_response()
}

fn rpc_error(id: Value, code: i32, message: String) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })),
    )
        .into_response()
}

fn tool_json(id: Value, value: Value) -> Response {
    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
    rpc(
        id,
        json!({
            "content": [{ "type": "text", "text": text }],
            "structuredContent": value,
            "isError": false
        }),
    )
}

fn tool_error(id: Value, message: String) -> Response {
    rpc(
        id,
        json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }),
    )
}

/// GET /mcp — Discovery for Streamable HTTP clients.
pub async fn discover() -> Response {
    Json(json!({
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "capabilities": { "tools": {} },
        "instructions": INSTRUCTIONS
    }))
    .into_response()
}

/// POST /mcp — Handles MCP JSON-RPC 2.0 messages.
pub async fn handle(_headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(Value::Null, -32700, "Parse error: expected JSON-RPC 2.0 object".into());
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return rpc_error(Value::Null, -32600, "Invalid Request: missing method".into());
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // Notifications acknowledge with 202 Accepted
    if message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

    if method != "tools/call" {
        seen(method);
    }

    match method {
        "server/discover" => rpc(
            id,
            json!({
                "supportedVersions": ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                "instructions": INSTRUCTIONS
            }),
        ),
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2024-11-05");
            rpc(
                id,
                json!({
                    "protocolVersion": asked,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                    "instructions": INSTRUCTIONS
                }),
            )
        }
        "ping" => rpc(id, json!({})),
        "tools/list" => {
            let list: Vec<Value> = tools()
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": (t.schema)()
                    })
                })
                .collect();
            rpc(id, json!({ "tools": list }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            seen(name);
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

            let Some(tool) = tools().iter().find(|t| t.name == name) else {
                return rpc_error(id, -32602, format!("Unknown tool: {name}; see tools/list."));
            };

            let call = (tool.call)(&args);

            if let Some(composite_kind) = call.path.strip_prefix("composite:") {
                match handle_composite(composite_kind, &args).await {
                    Ok(val) => tool_json(id, val),
                    Err(err) => tool_error(id, err),
                }
            } else {
                match call_route(call).await {
                    Ok((status, text)) if status.is_success() => {
                        let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
                        tool_json(id, parsed)
                    }
                    Ok((status, text)) => tool_error(id, format!("HTTP {status}: {text}")),
                    Err(err) => tool_error(id, err),
                }
            }
        }
        _ => rpc_error(id, -32601, format!("Method not found: {method}")),
    }
}
