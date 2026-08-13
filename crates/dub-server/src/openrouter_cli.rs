//! Запуск сайдкара openrouter-helper (Go SDK OpenRouter): один статический бинарь, как llama-server/
//! whisper. Протокол: аргумент = операция, JSON на stdin, ключ в env OPENROUTER_API_KEY, ответ на stdout.
//! Rust здесь только сериализует запрос и парсит ответ — сама интеграция с OpenRouter в Go SDK.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

/// Путь к бинарю сайдкара: env DUB_OPENROUTER_HELPER, иначе <repo>/tools/openrouter-helper/openrouter-helper(.exe).
pub fn helper_bin(repo_root: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("DUB_OPENROUTER_HELPER") {
        return PathBuf::from(p);
    }
    let exe = if cfg!(windows) { "openrouter-helper.exe" } else { "openrouter-helper" };
    repo_root.join("tools").join("openrouter-helper").join(exe)
}

/// repo_root из models_root (=<repo>/models): parent. Fallback — сам models_root.
pub fn repo_from_models(models_root: &Path) -> PathBuf {
    models_root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| models_root.to_path_buf())
}

/// Выполнить операцию helper: `op` + JSON-payload на stdin. Ключ передаём через env (не в argv/лог).
/// Возвращает сырые байты stdout (для tts это метаданные, аудио helper пишет в файл `out`).
pub fn run(repo_root: &Path, key: &str, op: &str, payload: &Value) -> Result<Vec<u8>, String> {
    let bin = helper_bin(repo_root);
    if !bin.is_file() {
        return Err(format!("openrouter-helper не найден: {}", bin.display()));
    }
    let mut cmd = crate::media::cmd_silent(&bin);
    cmd.arg(op).env("OPENROUTER_API_KEY", key);
    // Прокси для облака: если включён в active.json — прокидываем в env хелпера. Go net/http
    // (http.ProxyFromEnvironment у DefaultTransport, который использует Speakeasy-SDK) идёт через HTTPS_PROXY.
    if let Some(url) = crate::models::proxy_url(&repo_root.join("models")) {
        cmd.env("HTTPS_PROXY", &url).env("HTTP_PROXY", &url).env("ALL_PROXY", &url);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("запуск openrouter-helper: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("нет stdin у helper")?
        .write_all(payload.to_string().as_bytes())
        .map_err(|e| format!("stdin helper: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("ожидание helper: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "openrouter-helper {op}: {}",
            String::from_utf8_lossy(&out.stderr).trim().chars().take(400).collect::<String>()
        ));
    }
    Ok(out.stdout)
}

/// Как `run`, но парсит stdout в JSON.
pub fn run_json(repo_root: &Path, key: &str, op: &str, payload: &Value) -> Result<Value, String> {
    let bytes = run(repo_root, key, op, payload)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("ответ helper не JSON: {e}"))
}

/// Суммарно потрачено по ключу в ДОЛЛАРАХ (credits.data.total_usage; кредиты OpenRouter = USD 1:1).
/// None -> нет ключа/связи. Дельта до/после джобы = стоимость прогона.
pub fn total_usage_usd(models_root: &Path) -> Option<f64> {
    let key = crate::models::openrouter_key(models_root)?;
    let repo = repo_from_models(models_root);
    run_json(&repo, &key, "verify", &serde_json::json!({}))
        .ok()?
        .pointer("/credits/data/total_usage")
        .and_then(|x| x.as_f64())
}
