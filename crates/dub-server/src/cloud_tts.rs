//! Облачный TTS через сайдкар openrouter-helper (Go SDK OpenRouter, операция `tts` -> /audio/speech).
//! helper пишет mp3-байты в файл; читаем их и отдаём вызывающему, который кладёт ПРЯМО в seg-файл дубляжа
//! (без декода/перекодировки — запрет тупых перегенераций). Дальнейший fit_to_slot (ffmpeg) читает mp3
//! ОДИН раз. Модель/голос — только из настроек юзера (or_tts_model/or_tts_voice), без хардкода.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

/// Уникализатор temp-файлов синтеза (для потокобезопасности synth_batch).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "windows")]
const FFMPEG: &str = "ffmpeg.exe";
#[cfg(not(target_os = "windows"))]
const FFMPEG: &str = "ffmpeg";

/// Синтез одной реплики через Go-SDK-сайдкар -> WAV-байты 24кГц моно (готовы к записи в seg-файл).
/// helper отдаёт mp3 (часть моделей, напр. voxtral, только mp3), а конвейер читает seg как WAV (hound),
/// поэтому ОДИН раз конвертируем mp3->wav (не туда-обратно — это единственная необходимая конвертация,
/// как локальный Higgs пишет seg через encode_wav). `voice` пусто -> дефолт из настроек.
pub fn synth_audio(models_root: &Path, text: &str, voice: &str) -> Result<Vec<u8>, String> {
    let key = crate::models::openrouter_key(models_root)
        .ok_or("облачный TTS включён, но ключ OpenRouter не задан")?;
    let model = crate::models::openrouter_model(models_root, "tts");
    if model.is_empty() {
        return Err("TTS-модель не выбрана в настройках (Облачные модели · OpenRouter)".into());
    }
    let v = if voice.trim().is_empty() {
        crate::models::openrouter_tts_voice(models_root)
    } else {
        voice.trim().to_string()
    };
    if v.is_empty() {
        return Err("голос TTS не задан в настройках (у каждой модели свои голоса)".into());
    }

    let repo = crate::openrouter_cli::repo_from_models(models_root);
    // helper пишет аудио в файл. Имя УНИКАЛЬНО (pid + атомарный счётчик) — иначе параллельные потоки
    // синтеза (synth_batch) писали бы в один и тот же temp по pid и гонка портила бы аудио.
    let uid = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("dub_cloud_tts_{}_{}.bin", std::process::id(), uid));
    let payload = serde_json::json!({
        "model": model,
        "input": text,
        "voice": v,
        "out": tmp.to_string_lossy(),
    });
    if let Err(e) = crate::openrouter_cli::run_json(&repo, &key, "tts", &payload) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // helper всегда отдаёт УНИВЕРСАЛЬНЫЙ pcm (сырой 24кГц s16le mono). Оборачиваем в wav ОДИН раз
    // (пайплайн читает seg как WAV; локальный Higgs так же пишет seg через encode_wav).
    let wav_tmp = tmp.with_extension("wav");
    let out = crate::media::cmd_silent(FFMPEG)
        .args([
            "-v", "error",
            "-f", "s16le", "-ar", "24000", "-ac", "1",
            "-i", &tmp.to_string_lossy(),
            "-c:a", "pcm_s16le",
            "-y", &wav_tmp.to_string_lossy(),
        ])
        .output();
    let _ = std::fs::remove_file(&tmp);
    let out = out.map_err(|e| format!("ffmpeg mp3->wav: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&wav_tmp);
        return Err(format!("ffmpeg mp3->wav: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let bytes = std::fs::read(&wav_tmp).map_err(|e| format!("чтение облачного wav: {e}"));
    let _ = std::fs::remove_file(&wav_tmp);
    let bytes = bytes?;
    if bytes.len() < 200 {
        return Err(format!("облачный TTS: слишком короткое аудио ({} байт)", bytes.len()));
    }
    Ok(bytes)
}

/// Параллельный пре-синтез: гонит `jobs` (out-путь, текст, голос) в `concurrency` потоков (OpenRouter
/// держит десятки конкурентных запросов). Каждый успешный сегмент пишется в свой out-файл; провал ->
/// файл не создаётся (основной цикл ретраит/фолбэкнет на оригинал). Возвращает число успешных.
pub fn synth_batch(models_root: &Path, jobs: Vec<(PathBuf, String, String)>, concurrency: usize) -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let n = jobs.len();
    if n == 0 {
        return 0;
    }
    let workers = concurrency.max(1).min(n);
    let next = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicUsize::new(0));
    let jobs = Arc::new(jobs);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let next = next.clone();
            let done = done.clone();
            let jobs = jobs.clone();
            let mroot = models_root.to_path_buf();
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= jobs.len() {
                    break;
                }
                let (out, text, voice) = &jobs[i];
                if let Ok(bytes) = synth_audio(&mroot, text, voice) {
                    if std::fs::write(out, &bytes).is_ok() {
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    done.load(Ordering::Relaxed)
}
