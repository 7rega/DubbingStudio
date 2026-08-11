//! Запись голоса с микрофона (нативный cpal + hound → mono 16-bit WAV). Порт Higgs recorder.rs, но
//! уровень отдаём через атомик (GET /record/level опрашивает), не Tauri-события. WAV идёт в референс
//! клонирования / голос в галерею.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

struct RecState {
    stop_tx: Sender<()>,
    path: std::path::PathBuf,
}

fn recorder() -> &'static Mutex<Option<RecState>> {
    static R: OnceLock<Mutex<Option<RecState>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

fn peak_cell() -> &'static AtomicU32 {
    static P: OnceLock<AtomicU32> = OnceLock::new();
    P.get_or_init(|| AtomicU32::new(0))
}

/// Текущий пик 0..1 (для метра уровня).
pub fn level() -> f32 {
    f32::from_bits(peak_cell().load(Ordering::Relaxed))
}

/// Список микрофонов (первым — системный по умолчанию).
pub fn input_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let def = host.default_input_device().and_then(|d| d.name().ok());
    let mut names = Vec::new();
    if let Some(n) = &def {
        names.push(n.clone());
    }
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(n) = d.name() {
                if Some(&n) != def.as_ref() {
                    names.push(n);
                }
            }
        }
    }
    names
}

/// Начать запись в WAV по пути `path`. `device` — имя микрофона (None = системный).
pub fn start(path: &std::path::Path, device: Option<String>) -> Result<(), String> {
    let mut guard = recorder().lock().map_err(|_| "recorder poisoned".to_string())?;
    if guard.is_some() {
        return Err("Уже идёт запись".into());
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let path_buf = path.to_path_buf();
    let thread_path = path_buf.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = match device {
            Some(name) => host
                .input_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
                .or_else(|| host.default_input_device()),
            None => host.default_input_device(),
        };
        let Some(device) = device else {
            let _ = ready_tx.send(Err("Микрофон не найден".into()));
            return;
        };
        let supported = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => { let _ = ready_tx.send(Err(format!("конфиг микрофона: {e}"))); return; }
        };
        let fmt = supported.sample_format();
        let ch = (supported.channels() as usize).max(1);
        let sr = supported.sample_rate().0;
        let config: cpal::StreamConfig = supported.into();
        let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let writer = match hound::WavWriter::create(&thread_path, spec) {
            Ok(w) => Arc::new(Mutex::new(Some(w))),
            Err(e) => { let _ = ready_tx.send(Err(format!("создать wav: {e}"))); return; }
        };
        let emit_every = (sr / 20).max(1) as usize;

        macro_rules! cb {
            ($ty:ty, $to_f:expr) => {{
                let w = writer.clone();
                let mut peak = 0f32;
                let mut frames = 0usize;
                move |data: &[$ty], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut g) = w.lock() {
                        if let Some(wr) = g.as_mut() {
                            for frame in data.chunks(ch) {
                                let mono = frame.iter().map(|&s| $to_f(s)).sum::<f32>() / ch as f32;
                                peak = peak.max(mono.abs());
                                let _ = wr.write_sample((mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                            }
                        }
                    }
                    frames += data.len() / ch;
                    if frames >= emit_every {
                        peak_cell().store(peak.min(1.0).to_bits(), Ordering::Relaxed);
                        peak = 0.0;
                        frames = 0;
                    }
                }
            }};
        }
        let err_fn = |e| eprintln!("microphone stream error: {e}");
        let built = match fmt {
            cpal::SampleFormat::F32 => device.build_input_stream(&config, cb!(f32, |s: f32| s), err_fn, None),
            cpal::SampleFormat::I16 => device.build_input_stream(&config, cb!(i16, |s: i16| s as f32 / i16::MAX as f32), err_fn, None),
            cpal::SampleFormat::U16 => device.build_input_stream(&config, cb!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0), err_fn, None),
            other => { let _ = ready_tx.send(Err(format!("формат {other:?} не поддержан"))); return; }
        };
        let stream = match built {
            Ok(s) => s,
            Err(e) => { let _ = ready_tx.send(Err(format!("открыть микрофон: {e}"))); return; }
        };
        if let Err(e) = stream.play() {
            let _ = ready_tx.send(Err(format!("старт микрофона: {e}")));
            return;
        }
        let _ = ready_tx.send(Ok(()));
        let _ = stop_rx.recv();
        drop(stream);
        if let Some(w) = writer.lock().ok().and_then(|mut g| g.take()) {
            let _ = w.finalize();
        }
    });

    match ready_rx.recv().map_err(|e| e.to_string())? {
        Ok(()) => { *guard = Some(RecState { stop_tx, path: path_buf }); Ok(()) }
        Err(e) => Err(e),
    }
}

/// Остановить запись → путь к готовому WAV.
pub fn stop() -> Result<std::path::PathBuf, String> {
    let mut guard = recorder().lock().map_err(|_| "recorder poisoned".to_string())?;
    let st = guard.take().ok_or_else(|| "Запись не идёт".to_string())?;
    let _ = st.stop_tx.send(());
    std::thread::sleep(std::time::Duration::from_millis(200));
    peak_cell().store(0, Ordering::Relaxed);
    Ok(st.path)
}

const VOICE_PACK_URL: &str =
    "https://huggingface.co/datasets/nerualdreming/VibeVoice/resolve/main/voice-pack.zip";

// Датасет доп-голосов (mp3+txt пары), поиск/фильтр/прослушка/скачка в поп-апе.
const VOICES_DATASET: &str = "Slait/russia_voices";

fn hf_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http: {e}"))
}

/// Каталог доп-голосов из HF-датасета: [{name, gender, url}]. Кэшируется в статике на время процесса.
pub fn catalog() -> serde_json::Value {
    static CACHE: OnceLock<serde_json::Value> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let url = format!("https://huggingface.co/api/datasets/{VOICES_DATASET}/tree/main?recursive=1");
            let Ok(client) = hf_client() else { return serde_json::json!({ "voices": [] }) };
            let Ok(resp) = client.get(&url).send() else { return serde_json::json!({ "voices": [] }) };
            let Ok(txt) = resp.text() else { return serde_json::json!({ "voices": [] }) };
            let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) else { return serde_json::json!({ "voices": [] }) };
            let mut voices = Vec::new();
            for it in items {
                let path = it.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if !path.ends_with(".mp3") {
                    continue;
                }
                let name = path.trim_end_matches(".mp3");
                let gender = if name.contains("Female") { "female" } else if name.contains("Male") { "male" } else { "?" };
                voices.push(serde_json::json!({
                    "name": name,
                    "gender": gender,
                    "url": format!("https://huggingface.co/datasets/{VOICES_DATASET}/resolve/main/{path}"),
                }));
            }
            serde_json::json!({ "voices": voices })
        })
        .clone()
}

/// Скачать один голос датасета (mp3 + txt) в каталог голосов.
pub fn fetch_voice(dir: &std::path::Path, name: &str) -> Result<(), String> {
    use std::io::Write;
    if name.is_empty() || name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("плохое имя".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let client = hf_client()?;
    let base = std::path::Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or(name);
    for ext in ["mp3", "txt"] {
        let url = format!("https://huggingface.co/datasets/{VOICES_DATASET}/resolve/main/{name}.{ext}");
        let resp = client.get(&url).send().map_err(|e| format!("GET {ext}: {e}"))?;
        if !resp.status().is_success() {
            if ext == "txt" { continue; } // txt может отсутствовать — не критично
            return Err(format!("HTTP {} для {name}.{ext}", resp.status().as_u16()));
        }
        let bytes = resp.bytes().map_err(|e| format!("тело {ext}: {e}"))?;
        let mut f = std::fs::File::create(dir.join(format!("{base}.{ext}"))).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Скачать пак голосов (VibeVoice) и распаковать в `dir` (плоско, .mp3+.txt пары). Прогресс -> cb.
pub fn download_pack(dir: &std::path::Path, cb: &dyn Fn(serde_json::Value)) -> Result<serde_json::Value, String> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    cb(serde_json::json!({ "stage": "voicepack", "msg": "скачивание пака голосов", "pct": 0 }));
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let mut resp = client.get(VOICE_PACK_URL).send().map_err(|e| format!("GET пак: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} для пака", resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let tmp = dir.join("_voice-pack.zip");
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("создать zip: {e}"))?;
    let mut buf = [0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("чтение: {e}"))?;
        if n == 0 { break; }
        f.write_all(&buf[..n]).map_err(|e| format!("запись: {e}"))?;
        got += n as u64;
        if total > 0 {
            cb(serde_json::json!({ "stage": "voicepack", "msg": "скачивание пака голосов", "pct": (got as f64 / total as f64 * 90.0) }));
        }
    }
    drop(f);
    cb(serde_json::json!({ "stage": "voicepack", "msg": "распаковка", "pct": 92 }));
    let zf = std::fs::File::open(&tmp).map_err(|e| format!("открыть zip: {e}"))?;
    let mut zip = zip::ZipArchive::new(zf).map_err(|e| format!("zip: {e}"))?;
    let mut count = 0;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| format!("zip[{i}]: {e}"))?;
        if file.is_dir() { continue; }
        let name = std::path::Path::new(file.name())
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        let Some(name) = name else { continue };
        let out = dir.join(&name);
        let mut o = std::fs::File::create(&out).map_err(|e| format!("создать {name}: {e}"))?;
        std::io::copy(&mut file, &mut o).map_err(|e| format!("распаковка {name}: {e}"))?;
        count += 1;
    }
    let _ = std::fs::remove_file(&tmp);
    cb(serde_json::json!({ "stage": "voicepack", "msg": format!("готово: {count} файлов"), "pct": 100 }));
    Ok(serde_json::json!({ "extracted": count }))
}
