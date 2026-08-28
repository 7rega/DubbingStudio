//! Рантайм и HTTP-клиент CosyVoice 3 через CrispASR (persistent localhost сервер).
//!
//! Управляет жизненным циклом процесса crispasr.exe на свободном порту 127.0.0.1:<порт>,
//! дренирует stdout/stderr в кольцевой буфер (LogTail) для предотвращения дедлоков пайпов,
//! опрашивает готовность сервера (/health / /v1/models) и выполняет синтез речи
//! с on-the-fly zero-shot клонированием голоса через OpenAI-совместимый эндпоинт POST /v1/audio/speech.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

type LogTail = Arc<Mutex<VecDeque<String>>>;

fn drain_to_tail<R: std::io::Read + Send + 'static>(
    reader: R,
    tail: LogTail,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let r = BufReader::new(reader);
        for line in r.lines().map_while(Result::ok) {
            if let Ok(mut t) = tail.lock() {
                if t.len() >= 50 {
                    t.pop_front();
                }
                t.push_back(line);
            }
        }
    })
}

fn tail_text(tail: &LogTail) -> String {
    tail.lock()
        .map(|t| t.iter().map(String::as_str).collect::<Vec<_>>().join(" | "))
        .unwrap_or_default()
}

/// Найти свободный TCP-порт на 127.0.0.1
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("ошибка bind свободного порта для CosyVoice: {e}"))?;
    let p = l
        .local_addr()
        .map_err(|e| format!("ошибка local_addr для CosyVoice: {e}"))?
        .port();
    Ok(p)
}

/// Конфигурация для запуска CrispASR CosyVoice 3 рантайма
#[derive(Clone, Debug)]
pub struct CosyVoiceConfig {
    pub bin: PathBuf,
    pub llm_model: PathBuf,
    pub flow_model: Option<PathBuf>,
    pub hift_model: Option<PathBuf>,
    pub s3tok_model: Option<PathBuf>,
    pub campplus_model: Option<PathBuf>,
    pub voices_model: Option<PathBuf>,
    pub voice_dir: Option<PathBuf>,
    pub backend: String,
    pub device: String,
    pub temperature: f32,
    pub speed: f32,
    pub ready_timeout_secs: u64,
}

impl Default for CosyVoiceConfig {
    fn default() -> Self {
        Self {
            bin: PathBuf::from("tools/crispasr/crispasr.exe"),
            llm_model: PathBuf::from("models/cosyvoice3/cosyvoice3-llm-rl-q4_k.gguf"),
            flow_model: Some(PathBuf::from("models/cosyvoice3/cosyvoice3-flow-q8_0.gguf")),
            hift_model: Some(PathBuf::from("models/cosyvoice3/cosyvoice3-hift-f16.gguf")),
            s3tok_model: Some(PathBuf::from("models/cosyvoice3/cosyvoice3-s3tok-f16.gguf")),
            campplus_model: Some(PathBuf::from("models/cosyvoice3/cosyvoice3-campplus-f16.gguf")),
            voices_model: Some(PathBuf::from("models/cosyvoice3/cosyvoice3-voices.gguf")),
            voice_dir: None,
            backend: "cosyvoice3-tts-rl".to_string(),
            device: "cuda".to_string(),
            temperature: 0.7,
            speed: 1.0,
            ready_timeout_secs: 180,
        }
    }
}

/// Живой процесс CrispASR CosyVoice 3 сервера
pub struct CosyVoiceRuntime {
    child: Child,
    port: u16,
    base_url: String,
    backend: String,
    log_tail: LogTail,
    drain_handles: Vec<std::thread::JoinHandle<()>>,
    client: reqwest::blocking::Client,
}

impl CosyVoiceRuntime {
    /// Запустить сервер CrispASR и дождаться готовности HTTP API
    pub fn start(cfg: &CosyVoiceConfig) -> Result<Self, String> {
        if !cfg.bin.is_file() {
            return Err(format!(
                "CosyVoice 3: бинарник CrispASR не найден по пути: {}",
                cfg.bin.display()
            ));
        }
        if !cfg.llm_model.is_file() {
            return Err(format!(
                "CosyVoice 3: основная LLM-модель не найдена: {}",
                cfg.llm_model.display()
            ));
        }

        let port = free_port()?;
        let mut cmd = Command::new(&cfg.bin);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        // Параметры сервера CrispASR
        cmd.arg("--server")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("-m")
            .arg(&cfg.llm_model);

        if !cfg.backend.is_empty() {
            cmd.arg("--backend").arg(&cfg.backend);
        }

        // Каталог с моделями-компаньонами (flow, hift, s3tok, campplus, voices)
        if let Some(parent) = cfg.llm_model.parent() {
            cmd.arg("--cache-dir").arg(parent);
        }

        // Каталог с референсными голосами для Zero-Shot клонирования
        if let Some(ref vd) = cfg.voice_dir {
            cmd.arg("--voice-dir").arg(vd);
        }

        // Выбор устройства (device)
        if cfg.device == "cpu" {
            cmd.arg("--no-gpu");
        }

        // Устанавливаем рабочий каталог рядом с бинарником или моделями для подгрузки зависимых DLL
        if let Some(parent) = cfg.bin.parent() {
            cmd.current_dir(parent);
        }

        // Дренируем оба потока вывода
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        tracing::info!(
            "[cosyvoice3] starting runtime: bin={}, backend={}, port={}, llm={}",
            cfg.bin.display(),
            cfg.backend,
            port,
            cfg.llm_model.display()
        );

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("ошибка запуска процесса CrispASR ({:?}): {e}", cfg.bin))?;

        let base_url = format!("http://127.0.0.1:{port}");
        let log_tail: LogTail = Arc::new(Mutex::new(VecDeque::new()));
        let mut drain_handles = Vec::new();

        if let Some(out) = child.stdout.take() {
            drain_handles.push(drain_to_tail(out, log_tail.clone()));
        }
        if let Some(err) = child.stderr.take() {
            drain_handles.push(drain_to_tail(err, log_tail.clone()));
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| format!("ошибка создания HTTP клиента для CosyVoice: {e}"))?;

        let mut runtime = Self {
            child,
            port,
            base_url,
            backend: cfg.backend.clone(),
            log_tail,
            drain_handles,
            client,
        };

        runtime.wait_ready(cfg.ready_timeout_secs)?;
        tracing::info!("[cosyvoice3] server ready on {}", runtime.base_url);
        Ok(runtime)
    }

    /// Опрос готовности HTTP-сервера с таймаутом и fail-fast проверкой завершения дочернего процесса
    fn wait_ready(&mut self, timeout_secs: u64) -> Result<(), String> {
        let health_url = format!("{}/health", self.base_url);
        let models_url = format!("{}/v1/models", self.base_url);
        let root_url = self.base_url.clone();
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);

        while Instant::now() < deadline {
            // Проверяем, не упал ли процесс раньше времени
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!(
                    "CosyVoice 3 (CrispASR) завершился до готовности ({status}). Лог: {}",
                    tail_text(&self.log_tail)
                ));
            }

            // Пробуем эндпоинты /health, /v1/models или корень
            if let Ok(resp) = self.client.get(&health_url).timeout(Duration::from_secs(2)).send() {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if let Ok(resp) = self.client.get(&models_url).timeout(Duration::from_secs(2)).send() {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if let Ok(resp) = self.client.get(&root_url).timeout(Duration::from_secs(2)).send() {
                if resp.status().is_success() {
                    return Ok(());
                }
            }

            std::thread::sleep(Duration::from_millis(400));
        }

        Err(format!(
            "CosyVoice 3 (CrispASR) не поднялся за {timeout_secs}с (порт {}). Лог: {}",
            self.port,
            tail_text(&self.log_tail)
        ))
    }

    /// Синтезировать аудио с возможным zero-shot клонированием голоса
    pub fn synthesize(
        &self,
        text: &str,
        ref_wav: Option<&Path>,
        ref_text: Option<&str>,
        speed: Option<f32>,
    ) -> Result<Vec<u8>, String> {
        let url = format!("{}/v1/audio/speech", self.base_url);
        let mut payload = json!({
            "model": self.backend,
            "input": text,
            "response_format": "wav",
            "speed": speed.unwrap_or(1.0),
            "consent_attestation": "I have the speaker's consent",
        });

        if let Some(rw) = ref_wav {
            let voice_name = rw
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| rw.to_str().unwrap_or(""));
            payload["voice"] = json!(voice_name);
        }
        if let Some(rt) = ref_text {
            if !rt.trim().is_empty() {
                payload["ref_text"] = json!(rt.trim());
                payload["instructions"] = json!(rt.trim());
            }
        }

        tracing::info!(
            "[cosyvoice3] synthesizing speech: len={}, has_ref_wav={}, has_ref_text={}",
            text.len(),
            ref_wav.is_some(),
            ref_text.is_some()
        );

        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .map_err(|e| format!("ошибка HTTP-запроса к CosyVoice ({url}): {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "CosyVoice 3 вернул HTTP {status}: {body} (хвост логов: {})",
                tail_text(&self.log_tail)
            ));
        }

        let bytes = resp
            .bytes()
            .map_err(|e| format!("ошибка чтения аудио от CosyVoice: {e}"))?
            .to_vec();

        if bytes.len() < 100 {
            return Err(format!(
                "CosyVoice 3 вернул слишком короткий аудио-ответ ({} байт)",
                bytes.len()
            ));
        }

        // Проверка базового RIFF/WAVE заголовка
        if bytes.len() >= 12 && &bytes[0..4] != b"RIFF" && &bytes[8..12] != b"WAVE" {
            tracing::warn!("[cosyvoice3] получен аудио-поток без стандартного RIFF заголовка (размер {} байт)", bytes.len());
        }

        Ok(bytes)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Корректно завершить процесс
    pub fn stop(&mut self) {
        tracing::info!("[cosyvoice3] stopping runtime process (pid: {})", self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
        for h in self.drain_handles.drain(..) {
            let _ = h.join();
        }
        // Даем Windows WDDM время вернуть выделенную память
        std::thread::sleep(Duration::from_millis(200));
    }
}

impl Drop for CosyVoiceRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Нормализовать референсный WAV-файл в 16 kHz mono PCM16 WAV во временный файл
pub fn normalize_reference_wav(source: &Path, target: &Path, max_secs: f64) -> Result<(), String> {
    #[cfg(windows)]
    const FFMPEG: &str = "ffmpeg.exe";
    #[cfg(not(windows))]
    const FFMPEG: &str = "ffmpeg";

    let mut cmd = crate::media::cmd_silent(FFMPEG);
    cmd.args(["-y", "-v", "error", "-i"])
        .arg(source)
        .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]);

    if max_secs > 0.0 {
        cmd.args(["-t", &format!("{max_secs:.2}")]);
    }

    cmd.arg(target);

    let out = cmd
        .output()
        .map_err(|e| format!("ошибка вызова ffmpeg для нормализации reference wav: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ffmpeg reference normalization error: {err}"));
    }

    if !target.is_file() {
        return Err(format!(
            "целевой нормализованный WAV не создан: {}",
            target.display()
        ));
    }

    Ok(())
}

/// Диагностика доступности компонентов CosyVoice 3
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CosyVoiceDiagnostics {
    pub ok: bool,
    pub executable_found: bool,
    pub executable_path: Option<String>,
    pub llm_rl_found: bool,
    pub llm_base_found: bool,
    pub active_llm_path: Option<String>,
    pub flow_found: bool,
    pub hift_found: bool,
    pub s3tok_found: bool,
    pub campplus_found: bool,
    pub voices_found: bool,
    pub missing: Vec<String>,
    pub details: String,
}

/// Проверить статус установки всех компонентов CosyVoice 3
pub fn check_cosyvoice3(repo_root: &Path, models_root: &Path) -> CosyVoiceDiagnostics {
    let bin_path = if let Ok(env_bin) = std::env::var("DUB_STUDIO_CRISPASR_BIN") {
        PathBuf::from(env_bin)
    } else {
        repo_root.join("tools").join("crispasr").join("crispasr.exe")
    };

    let cosy_dir = models_root.join("cosyvoice3");
    let llm_rl = cosy_dir.join("cosyvoice3-llm-rl-q4_k.gguf");
    let llm_base = cosy_dir.join("cosyvoice3-llm-q4_k.gguf");
    let flow = cosy_dir.join("cosyvoice3-flow-q8_0.gguf");
    let hift = cosy_dir.join("cosyvoice3-hift-f16.gguf");
    let s3tok = cosy_dir.join("cosyvoice3-s3tok-f16.gguf");
    let campplus = cosy_dir.join("cosyvoice3-campplus-f16.gguf");
    let voices = cosy_dir.join("cosyvoice3-voices.gguf");

    let executable_found = bin_path.is_file();
    let llm_rl_found = llm_rl.is_file();
    let llm_base_found = llm_base.is_file();
    let flow_found = flow.is_file();
    let hift_found = hift.is_file();
    let s3tok_found = s3tok.is_file();
    let campplus_found = campplus.is_file();
    let voices_found = voices.is_file();

    let mut missing = Vec::new();
    if !executable_found {
        missing.push("crispasr-engine (crispasr.exe)".to_string());
    }
    if !llm_rl_found && !llm_base_found {
        missing.push("cosyvoice3-llm (cosyvoice3-llm-rl-q4_k.gguf)".to_string());
    }
    if !flow_found {
        missing.push("cosyvoice3-flow (cosyvoice3-flow-q8_0.gguf)".to_string());
    }
    if !hift_found {
        missing.push("cosyvoice3-hift (cosyvoice3-hift-f16.gguf)".to_string());
    }
    if !s3tok_found {
        missing.push("cosyvoice3-s3tok (cosyvoice3-s3tok-f16.gguf)".to_string());
    }
    if !campplus_found {
        missing.push("cosyvoice3-campplus (cosyvoice3-campplus-f16.gguf)".to_string());
    }
    if !voices_found {
        missing.push("cosyvoice3-voices (cosyvoice3-voices.gguf)".to_string());
    }

    let ok = missing.is_empty();
    let active_llm = if llm_rl_found {
        Some(llm_rl.to_string_lossy().into_owned())
    } else if llm_base_found {
        Some(llm_base.to_string_lossy().into_owned())
    } else {
        None
    };

    let details = if ok {
        "Все обязательные компоненты CosyVoice 3 (CrispASR + 5 GGUF моделей) установлены и готовы к работе."
            .to_string()
    } else {
        format!(
            "Отсутствуют обязательные компоненты CosyVoice 3: {}. Скачайте их через интерфейс или разместите в models/cosyvoice3/ и tools/crispasr/.",
            missing.join(", ")
        )
    };

    CosyVoiceDiagnostics {
        ok,
        executable_found,
        executable_path: if executable_found {
            Some(bin_path.to_string_lossy().into_owned())
        } else {
            None
        },
        llm_rl_found,
        llm_base_found,
        active_llm_path: active_llm,
        flow_found,
        hift_found,
        s3tok_found,
        campplus_found,
        voices_found,
        missing,
        details,
    }
}
