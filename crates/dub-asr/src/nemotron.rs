use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use serde::Deserialize;

use crate::{postprocess_diar_turns, AsrError, DiarTurns, Turn};

#[derive(Clone, Debug)]
pub struct NemotronDiarConfig {
    pub model_path: PathBuf,
    pub backend: String,          // "cuda" или "cpu"
    pub threshold: f32,           // дефолт: 0.48
    pub min_frames: u32,          // дефолт: 10 (100 мс)
    pub merge_gap: f64,           // дефолт: 0.5 с
    pub min_speaker_dur: f64,     // дефолт: 1.5 с
}

impl Default for NemotronDiarConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            backend: "cuda".to_string(),
            threshold: 0.48,
            min_frames: 10,
            merge_gap: 0.5,
            min_speaker_dur: 1.5,
        }
    }
}

#[derive(Deserialize, Debug)]
struct RawNemotronTurn {
    start_sample: u64,
    end_sample: u64,
    speaker_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    confidence: Option<f64>,
}

/// Диаризация аудио через Nemotron-3 (audio.cpp audiocpp_cli.exe).
pub fn diarize_nemotron(
    wav_path: &Path,
    cli_bin: &Path,
    cfg: &NemotronDiarConfig,
) -> Result<DiarTurns, AsrError> {
    if !cli_bin.is_file() {
        return Err(AsrError::Diarize(format!(
            "audiocpp_cli не найден: {}",
            cli_bin.display()
        )));
    }
    if !cfg.model_path.is_file() {
        return Err(AsrError::Diarize(format!(
            "модель Nemotron-3 не найдена: {}",
            cfg.model_path.display()
        )));
    }
    if !wav_path.is_file() {
        return Err(AsrError::Diarize(format!(
            "WAV-файл не найден: {}",
            wav_path.display()
        )));
    }

    let temp_dir = std::env::temp_dir().join("nemotron_diar");
    let _ = std::fs::create_dir_all(&temp_dir);
    let temp_json = temp_dir.join(format!(
        "turns_{}_{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    let backend = match cfg.backend.to_lowercase().as_str() {
        "cpu" => "cpu",
        _ => "cuda",
    };

    let mut cmd = Command::new(cli_bin);
    cmd.arg("--task").arg("diar");
    cmd.arg("--family").arg("nemotron_3_diar");
    cmd.arg("--model").arg(&cfg.model_path);
    cmd.arg("--backend").arg(backend);
    cmd.arg("--audio").arg(wav_path);
    cmd.arg("--turns-out").arg(&temp_json);
    cmd.arg("--request-option").arg(format!("speaker_threshold={}", cfg.threshold));
    if cfg.min_frames > 0 {
        cmd.arg("--request-option").arg(format!("speaker_min_frames={}", cfg.min_frames));
    }

    // Запуск из директории бинарника (для загрузки ggml*.dll и CUDA рантайма)
    if let Some(parent) = cli_bin.parent() {
        cmd.current_dir(parent);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().map_err(|e| {
        let _ = std::fs::remove_file(&temp_json);
        AsrError::Diarize(format!("ошибка запуска audiocpp_cli: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _ = std::fs::remove_file(&temp_json);
        return Err(AsrError::Diarize(format!(
            "audiocpp_cli завершился со сбоем (status: {}): {stderr} {stdout}",
            output.status
        )));
    }

    if !temp_json.is_file() {
        let _ = std::fs::remove_file(&temp_json);
        return Err(AsrError::Diarize("audiocpp_cli не создал файл turns.json".into()));
    }

    let json_bytes = std::fs::read(&temp_json).map_err(|e| {
        let _ = std::fs::remove_file(&temp_json);
        AsrError::Diarize(format!("ошибка чтения turns.json: {e}"))
    })?;
    let _ = std::fs::remove_file(&temp_json);

    let raw_entries: Vec<RawNemotronTurn> = serde_json::from_slice(&json_bytes)
        .map_err(|e| AsrError::Diarize(format!("ошибка парсинга turns.json: {e}")))?;

    // Преобразуем сэмплы (16000 Гц) в секунды и сопоставляем speaker_id в целочисленные индексы
    let mut speaker_map: HashMap<String, i32> = HashMap::new();
    let mut raw_turns: Vec<Turn> = Vec::with_capacity(raw_entries.len());

    for entry in raw_entries {
        let start = entry.start_sample as f64 / 16000.0;
        let end = entry.end_sample as f64 / 16000.0;
        if end <= start {
            continue;
        }

        let next_id = speaker_map.len() as i32;
        let spk = *speaker_map.entry(entry.speaker_id).or_insert(next_id);

        raw_turns.push(Turn {
            start,
            end,
            speaker: spk,
        });
    }

    Ok(postprocess_diar_turns(&raw_turns, cfg.merge_gap, cfg.min_speaker_dur))
}

/// Упрощённый вызов Nemotron-3 диаризации с плоскими параметрами.
pub fn diarize_nemotron_simple(
    wav_path: &Path,
    cli_bin: &Path,
    model_path: &Path,
    backend: &str,
    threshold: f32,
    merge_gap: f64,
    min_speaker_dur: f64,
) -> Result<DiarTurns, AsrError> {
    let cfg = NemotronDiarConfig {
        model_path: model_path.to_path_buf(),
        backend: backend.to_string(),
        threshold,
        min_frames: 10,
        merge_gap,
        min_speaker_dur,
    };
    diarize_nemotron(wav_path, cli_bin, &cfg)
}
