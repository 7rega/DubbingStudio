//! Сайдкар audiocpp_server (0xShug0/audio.cpp).
//! Поднимаем один процесс audiocpp_server на свободном 127.0.0.1:<порт>,
//! ждём готовности по /health, шлём /v1/audio/speech, глушим на Drop.
//!
//! Идентично dub-llm (llama-server): изолированный процесс, одна загрузка в VRAM,
//! нулевой оверхед между фразами, полная безопасность от падений C++ DLL.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudiocppServerError {
    #[error("запуск audiocpp_server: {0}")]
    Spawn(String),
    #[error("HTTP ошибка: {0}")]
    Http(String),
    #[error("API ошибка: {0}")]
    Api(String),
    #[error("ошибка декодирования WAV: {0}")]
    WavDecode(String),
    #[error("I/O ошибка: {0}")]
    Io(#[from] std::io::Error),
}

/// Последние N строк stderr/stdout для диагностики при сбое.
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

/// Параметры запуска сайдкар-сервера audiocpp_server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudiocppServerOpts {
    pub bin: PathBuf,
    pub model_path: PathBuf,
    pub model_family: String,
    pub model_id: String,
    pub backend: String,
    pub device: i32,
    pub threads: i32,
    pub ready_timeout_secs: u64,
}

impl AudiocppServerOpts {
    pub fn new(bin: impl Into<PathBuf>, model_path: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            model_path: model_path.into(),
            model_family: "voxcpm2".to_string(),
            model_id: "voxcpm2".to_string(),
            backend: "cuda".to_string(),
            device: 0,
            threads: 4,
            ready_timeout_secs: 120,
        }
    }
}

/// Опции генерации речи для audiocpp_server (температура, сид, шаги диффузии, CFG scale).
#[derive(Clone, Debug, Default)]
pub struct SpeechOptions {
    pub temperature: Option<f64>,
    pub seed: Option<u64>,
    pub num_inference_steps: Option<u32>,
    pub guidance_scale: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// HTTP клиент к сайдкар-серверу audiocpp_server.
#[derive(Clone)]
pub struct AudiocppClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl AudiocppClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into(),
            client,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Проверка доступности сервера (/health).
    pub fn is_healthy(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        self.client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Синтез речи / клон голоса через POST /v1/audio/speech.
    /// Возвращает сырые WAV-байты (48 кГц / 44.1 кГц, PCM16).
    pub fn speech(
        &self,
        model_id: &str,
        text: &str,
        voice_ref: Option<&Path>,
        reference_text: Option<&str>,
        speed: Option<f32>,
        opts: Option<&SpeechOptions>,
    ) -> Result<Vec<u8>, AudiocppServerError> {
        let url = format!("{}/v1/audio/speech", self.base_url);

        let mut body = serde_json::json!({
            "model": model_id,
            "input": text,
        });

        if let Some(vr) = voice_ref {
            let vr_str = vr.to_string_lossy().to_string();
            if !vr_str.is_empty() {
                body.as_object_mut()
                    .unwrap()
                    .insert("voice_ref".to_string(), serde_json::Value::String(vr_str));
            }
        }

        if let Some(rt) = reference_text {
            // Сохраняем непустую строку (включая " " для изоляции референса в Fish Audio)
            if !rt.is_empty() {
                body.as_object_mut()
                    .unwrap()
                    .insert("reference_text".to_string(), serde_json::Value::String(rt.to_string()));
            }
        }

        if let Some(sp) = speed {
            // VoxCPM C++ сессия в audio.cpp не принимает стилевые условия (speed).
            if sp > 0.0 && !model_id.to_lowercase().contains("voxcpm") {
                body.as_object_mut()
                    .unwrap()
                    .insert("speed".to_string(), serde_json::json!(sp));
            }
        }

        if let Some(opt) = opts {
            let obj = body.as_object_mut().unwrap();
            if let Some(temp) = opt.temperature {
                obj.insert("temperature".to_string(), serde_json::json!(temp));
            }
            if let Some(seed) = opt.seed {
                obj.insert("seed".to_string(), serde_json::json!(seed));
            }
            if let Some(steps) = opt.num_inference_steps {
                obj.insert("num_inference_steps".to_string(), serde_json::json!(steps));
            }
            if let Some(cfg) = opt.guidance_scale {
                obj.insert("guidance_scale".to_string(), serde_json::json!(cfg));
            }
            if let Some(max_tok) = opt.max_tokens {
                obj.insert("max_tokens".to_string(), serde_json::json!(max_tok));
            }
        }

        // Профиль качества по умолчанию для VoxCPM2 (20 шагов диффузии и CFG 1.6, если не переопределено)
        if model_id.to_lowercase().contains("voxcpm") {
            let obj = body.as_object_mut().unwrap();
            if !obj.contains_key("num_inference_steps") {
                obj.insert("num_inference_steps".to_string(), serde_json::json!(20));
            }
            if !obj.contains_key("guidance_scale") {
                obj.insert("guidance_scale".to_string(), serde_json::json!(1.6));
            }
        }

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| AudiocppServerError::Http(format!("POST /v1/audio/speech: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().unwrap_or_default();
            return Err(AudiocppServerError::Api(format!(
                "audiocpp_server вернул HTTP {status}: {err_body}"
            )));
        }

        let bytes = resp
            .bytes()
            .map_err(|e| AudiocppServerError::Http(format!("чтение WAV ответа: {e}")))?
            .to_vec();

        if bytes.len() < 44 {
            return Err(AudiocppServerError::Api(format!(
                "audiocpp_server вернул слишком короткий ответ ({} байт)",
                bytes.len()
            )));
        }

        Ok(bytes)
    }

    /// Синтез речи с декодированием в (Vec<f32>, sample_rate) для прямого использования в дубляже / QC.
    pub fn speech_pcm(
        &self,
        model_id: &str,
        text: &str,
        voice_ref: Option<&Path>,
        reference_text: Option<&str>,
        speed: Option<f32>,
        opts: Option<&SpeechOptions>,
    ) -> Result<(Vec<f32>, i32, Vec<u8>), AudiocppServerError> {
        let wav_bytes = self.speech(model_id, text, voice_ref, reference_text, speed, opts)?;
        let (samples, sr) = decode_wav_mono_f32(&wav_bytes)?;
        Ok((samples, sr, wav_bytes))
    }
}

/// Управляемый процесс сайдкар-сервера audiocpp_server.
pub struct AudiocppServer {
    child: Child,
    port: u16,
    client: AudiocppClient,
    log_tail: LogTail,
    drain_handles: Vec<std::thread::JoinHandle<()>>,
    config_path: PathBuf,
    pub opts: AudiocppServerOpts,
}

impl AudiocppServer {
    /// Принудительно завершить любые висящие зомби-процессы audiocpp_server(.exe) в ОС перед запуском.
    pub fn kill_zombie_processes(bin: &Path) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let bin_name = bin
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("audiocpp_server.exe");
            let _ = Command::new("taskkill")
                .args(["/F", "/IM", bin_name, "/T"])
                .creation_flags(0x08000000)
                .output();
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Проверить, запущен ли сервер с теми же параметрами и отвечает ли на /health.
    pub fn can_reuse(&self, new_opts: &AudiocppServerOpts) -> bool {
        self.opts == *new_opts && self.client.is_healthy()
    }

    /// Запустить сайдкар-сервер с указанными параметрами.
    pub fn start(opts: AudiocppServerOpts) -> Result<Self, AudiocppServerError> {
        if !opts.bin.exists() {
            return Err(AudiocppServerError::Spawn(format!(
                "audiocpp_server бинарник не найден: {}",
                opts.bin.display()
            )));
        }
        if !opts.model_path.exists() {
            return Err(AudiocppServerError::Spawn(format!(
                "модель audiocpp не найдена: {}",
                opts.model_path.display()
            )));
        }

        // Предварительная зачистка зависших процессов audiocpp_server перед стартом
        Self::kill_zombie_processes(&opts.bin);

        // Подбираем свободный порт на 127.0.0.1
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| AudiocppServerError::Spawn(format!("поиск свободного порта: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| AudiocppServerError::Spawn(format!("local_addr порта: {e}")))?
            .port();
        drop(listener);

        // Формируем временный server.json
        let config_dir = std::env::temp_dir().join("audiocpp_configs");
        let _ = std::fs::create_dir_all(&config_dir);
        let config_path = config_dir.join(format!("server_{port}.json"));

        let mut model_entry = serde_json::json!({
            "id": opts.model_id,
            "family": opts.model_family,
            "path": opts.model_path.to_string_lossy().to_string(),
            "task": "tts",
            "mode": "offline"
        });
        if opts.model_family == "voxcpm2" {
            model_entry.as_object_mut().unwrap().insert(
                "default_request_options".to_string(),
                serde_json::json!({
                    "num_inference_steps": 20,
                    "guidance_scale": 1.6
                }),
            );
        }

        let cfg = serde_json::json!({
            "host": "127.0.0.1",
            "port": port,
            "backend": opts.backend,
            "device": opts.device,
            "threads": opts.threads,
            "lazy_load": true,
            "models": [model_entry]
        });

        std::fs::write(&config_path, serde_json::to_vec_pretty(&cfg).unwrap_or_default())
            .map_err(|e| AudiocppServerError::Spawn(format!("запись конфига сервера: {e}")))?;

        let mut cmd = Command::new(&opts.bin);
        cmd.arg("--config").arg(&config_path);
        cmd.arg("--no-ui");
        cmd.arg("--idle-unload-ms").arg("45000");
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // CWD = директория бинарника (чтобы Windows loader нашёл ggml*.dll и cuda DLL рядом)
        if let Some(parent) = opts.bin.parent() {
            cmd.current_dir(parent);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| AudiocppServerError::Spawn(format!("spawn audiocpp_server: {e}")))?;

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            crate::job_object::assign_process_to_global_job(child.as_raw_handle());
        }

        let log_tail = Arc::new(Mutex::new(VecDeque::new()));
        let mut drain_handles = Vec::new();

        if let Some(out) = child.stdout.take() {
            drain_handles.push(drain_to_tail(out, log_tail.clone()));
        }
        if let Some(err) = child.stderr.take() {
            drain_handles.push(drain_to_tail(err, log_tail.clone()));
        }

        let base_url = format!("http://127.0.0.1:{port}");
        let client = AudiocppClient::new(&base_url);

        let mut srv = Self {
            child,
            port,
            client,
            log_tail,
            drain_handles,
            config_path,
            opts,
        };

        srv.wait_ready(srv.opts.ready_timeout_secs)?;
        Ok(srv)
    }

    /// Опрос /health до status=ok или таймаута.
    fn wait_ready(&mut self, timeout_secs: u64) -> Result<(), AudiocppServerError> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(AudiocppServerError::Spawn(format!(
                    "audiocpp_server завершился до готовности ({status}); stderr: {}",
                    tail_text(&self.log_tail)
                )));
            }

            if self.client.is_healthy() {
                return Ok(());
            }

            if Instant::now() > deadline {
                let _ = self.child.kill();
                return Err(AudiocppServerError::Spawn(format!(
                    "audiocpp_server не ответил за {timeout_secs}с; stderr: {}",
                    tail_text(&self.log_tail)
                )));
            }

            std::thread::sleep(Duration::from_millis(150));
        }
    }

    pub fn client(&self) -> &AudiocppClient {
        &self.client
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.drain_handles.clear();
        let _ = std::fs::remove_file(&self.config_path);
    }
}

impl Drop for AudiocppServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Поиск бинарника audiocpp_server(.exe).
pub fn resolve_audiocpp_bin(tools_dir: &Path) -> PathBuf {
    if let Ok(v) = std::env::var("DUB_STUDIO_AUDIOCPP_BIN") {
        let p = PathBuf::from(v);
        if p.exists() {
            return p;
        }
    }
    let name = if cfg!(windows) {
        "audiocpp_server.exe"
    } else {
        "audiocpp_server"
    };

    let direct = tools_dir.join(name);
    if direct.exists() {
        return direct;
    }

    let sub = tools_dir.join("audiocpp").join(name);
    if sub.exists() {
        return sub;
    }

    // Fallback на E:\audio.cpp если скачано туда
    let ext_fallback = PathBuf::from("E:\\audio.cpp").join(name);
    if ext_fallback.exists() {
        return ext_fallback;
    }

    direct
}

/// Поиск модели VoxCPM2 (q8_0 или bf16).
pub fn resolve_voxcpm2_path(models_root: &Path, quant: &str) -> Option<PathBuf> {
    let filename = match quant {
        "bf16" => "voxcpm2-bf16.gguf",
        _ => "voxcpm2-q8_0.gguf",
    };

    let candidates = [
        models_root.join("voxcpm2").join(filename),
        models_root.join(filename),
        PathBuf::from("E:\\audio.cpp\\models").join(filename),
        PathBuf::from("F:\\DubStudio\\models\\voxcpm2").join(filename),
    ];

    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

/// Поиск модели Fish Audio S2 Pro (q8_0 или bf16).
pub fn resolve_fish_audio_path(models_root: &Path, quant: &str) -> Option<PathBuf> {
    let filename = match quant {
        "bf16" => "fish-audio-s2-pro-bf16.gguf",
        _ => "fish-audio-s2-pro-q8_0.gguf",
    };

    let candidates = [
        models_root.join("fish_audio").join(filename),
        models_root.join(filename),
        PathBuf::from("E:\\audio.cpp\\models").join(filename),
        PathBuf::from("F:\\DubStudio\\models\\fish_audio").join(filename),
    ];

    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

/// Декодер RIFF WAV (PCM16 или Float32) в моно f32 сэмплы + sample_rate.
pub fn decode_wav_mono_f32(bytes: &[u8]) -> Result<(Vec<f32>, i32), AudiocppServerError> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudiocppServerError::WavDecode("невалидный RIFF WAV заголовок".into()));
    }

    let mut cursor = 12;
    let mut audio_format: u16 = 1; // 1 = PCM, 3 = IEEE float
    let mut channels: u16 = 1;
    let mut sample_rate: u32 = 48000;
    let mut bits_per_sample: u16 = 16;
    let mut data_start: usize = 0;
    let mut data_len: usize = 0;

    while cursor + 8 <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;

        if chunk_id == b"fmt " && cursor + 16 <= bytes.len() {
            audio_format = u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap());
            channels = u16::from_le_bytes(bytes[cursor + 2..cursor + 4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
            bits_per_sample = u16::from_le_bytes(bytes[cursor + 14..cursor + 16].try_into().unwrap());
        } else if chunk_id == b"data" {
            data_start = cursor;
            data_len = chunk_size.min(bytes.len() - cursor);
            break;
        }

        cursor += chunk_size;
    }

    if data_start == 0 || data_len == 0 {
        return Err(AudiocppServerError::WavDecode("chunk data не найден".into()));
    }

    let data = &bytes[data_start..data_start + data_len];
    let ch = channels as usize;
    if ch == 0 {
        return Err(AudiocppServerError::WavDecode("channels == 0".into()));
    }

    let samples: Vec<f32> = if audio_format == 1 && bits_per_sample == 16 {
        // 16-bit signed PCM
        let count = data.len() / 2;
        let mut mono = Vec::with_capacity(count / ch);
        for frame in (0..count).step_by(ch) {
            let i16_val = i16::from_le_bytes([data[frame * 2], data[frame * 2 + 1]]);
            mono.push(i16_val as f32 / 32768.0);
        }
        mono
    } else if audio_format == 3 && bits_per_sample == 32 {
        // 32-bit float
        let count = data.len() / 4;
        let mut mono = Vec::with_capacity(count / ch);
        for frame in (0..count).step_by(ch) {
            let f32_val = f32::from_le_bytes(data[frame * 4..frame * 4 + 4].try_into().unwrap());
            mono.push(f32_val);
        }
        mono
    } else {
        return Err(AudiocppServerError::WavDecode(format!(
            "неподдерживаемый формат WAV: format={audio_format}, bits={bits_per_sample}"
        )));
    };

    Ok((samples, sample_rate as i32))
}
