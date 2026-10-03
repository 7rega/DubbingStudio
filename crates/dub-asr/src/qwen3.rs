//! Qwen3-ASR + Qwen3 Forced Aligner движок через audio.cpp (audiocpp_cli.exe).
//!
//! Интеграция связки Qwen3 ASR (1.7B) и Qwen3 Forced Aligner (0.6B) со встроенным Silero VAD
//! для нарезки длинных файлов (>30-45 сек). Запуск сабпроцессом через audiocpp_cli.exe,
//! выдача пословных таймингов в формате words.json (start_sample / end_sample / word).
//!
//! Для диаризации слова автоматически маппятся на turns Nemotron-3 или Sortformer
//! через единый алгоритм segment_words_with_diarization.

use std::path::{Path, PathBuf};
use std::process::Command;
use serde::Deserialize;

use crate::segment::{segment_words, Segment, Word, SEG_MAX_DUR, SEG_MAX_GAP};
use crate::{AsrEngine, AsrError, SpeakerSegment, Turn};

#[derive(Clone, Debug)]
pub struct Qwen3Asr {
    pub cli_bin: PathBuf,
    pub asr_model: PathBuf,
    pub aligner_model: PathBuf,
    pub backend: String,
    pub detected_language: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

#[derive(Deserialize, Debug)]
struct RawQwenWord {
    start_sample: u64,
    end_sample: u64,
    word: String,
    #[serde(default)]
    #[allow(dead_code)]
    confidence: Option<serde_json::Value>,
}

impl Qwen3Asr {
    pub fn new(
        cli_bin: impl AsRef<Path>,
        asr_model: impl AsRef<Path>,
        aligner_model: impl AsRef<Path>,
        backend: impl Into<String>,
    ) -> Self {
        Self {
            cli_bin: cli_bin.as_ref().to_path_buf(),
            asr_model: asr_model.as_ref().to_path_buf(),
            aligner_model: aligner_model.as_ref().to_path_buf(),
            backend: backend.into(),
            detected_language: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

impl AsrEngine for Qwen3Asr {
    fn detected_language(&self) -> Option<String> {
        self.detected_language.lock().ok()?.clone()
    }

    fn transcribe_words(&mut self, wav: &Path, lang: &str) -> Result<Vec<Word>, AsrError> {
        if !self.cli_bin.is_file() {
            return Err(AsrError::Transcribe(format!(
                "audiocpp_cli не найден: {}",
                self.cli_bin.display()
            )));
        }
        if !self.asr_model.is_file() {
            return Err(AsrError::Transcribe(format!(
                "модель Qwen3 ASR не найдена: {}",
                self.asr_model.display()
            )));
        }
        if !self.aligner_model.is_file() {
            return Err(AsrError::Transcribe(format!(
                "модель Qwen3 Forced Aligner не найдена: {}",
                self.aligner_model.display()
            )));
        }
        if !wav.is_file() {
            return Err(AsrError::Transcribe(format!(
                "WAV-файл не найден: {}",
                wav.display()
            )));
        }

        let temp_dir = std::env::temp_dir().join("qwen3_asr");
        let _ = std::fs::create_dir_all(&temp_dir);
        let temp_words = temp_dir.join(format!(
            "words_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));

        let backend = match self.backend.to_lowercase().as_str() {
            "cpu" => "cpu",
            _ => "cuda",
        };

        let mut cmd = Command::new(&self.cli_bin);
        cmd.arg("--task").arg("asr");
        cmd.arg("--family").arg("qwen3_asr");
        cmd.arg("--model").arg(&self.asr_model);
        cmd.arg("--backend").arg(backend);
        cmd.arg("--audio").arg(wav);
        cmd.arg("--session-option").arg(format!(
            "qwen3_asr.forced_aligner_model_path={}",
            self.aligner_model.display()
        ));
        cmd.arg("--request-option").arg("qwen3_asr.preserve_punctuation=true");
        cmd.arg("--words-out").arg(&temp_words);

        if !lang.is_empty() && lang != "auto" {
            cmd.arg("--language").arg(lang);
        }

        // Запуск из директории бинарника (для загрузки ggml*.dll, cublas и assets/...)
        if let Some(parent) = self.cli_bin.parent() {
            cmd.current_dir(parent);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd.output().map_err(|e| {
            let _ = std::fs::remove_file(&temp_words);
            AsrError::Transcribe(format!("ошибка запуска audiocpp_cli: {e}"))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let _ = std::fs::remove_file(&temp_words);
            return Err(AsrError::Transcribe(format!(
                "audiocpp_cli завершился со сбоем (status: {}): {stderr} {stdout}",
                output.status
            )));
        }

        if !temp_words.is_file() {
            let _ = std::fs::remove_file(&temp_words);
            return Err(AsrError::Transcribe(
                "audiocpp_cli не создал файл words.json".into(),
            ));
        }

        let json_bytes = std::fs::read(&temp_words).map_err(|e| {
            let _ = std::fs::remove_file(&temp_words);
            AsrError::Transcribe(format!("ошибка чтения words.json: {e}"))
        })?;
        let _ = std::fs::remove_file(&temp_words);

        let raw_entries: Vec<RawQwenWord> = serde_json::from_slice(&json_bytes)
            .map_err(|e| AsrError::Transcribe(format!("ошибка парсинга words.json: {e}")))?;

        // Преобразуем сэмплы (16000 Гц) в секунды
        let mut words: Vec<Word> = Vec::with_capacity(raw_entries.len());
        for entry in raw_entries {
            let start = entry.start_sample as f64 / 16000.0;
            let end = entry.end_sample as f64 / 16000.0;
            let text = entry.word.trim();
            if end > start && !text.is_empty() {
                words.push(Word::new(text, start, end));
            }
        }

        if !lang.is_empty() && lang != "auto" {
            if let Ok(mut l) = self.detected_language.lock() {
                *l = Some(lang.to_string());
            }
        }

        Ok(words)
    }

    fn transcribe(&mut self, wav: &Path, lang: &str) -> Result<Vec<Segment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        Ok(segment_words(&words, SEG_MAX_GAP, SEG_MAX_DUR))
    }

    fn transcribe_turns(
        &mut self,
        wav: &Path,
        turns: &[Turn],
        lang: &str,
    ) -> Result<Vec<SpeakerSegment>, AsrError> {
        let segments = self.transcribe_with_diar(wav, turns, lang)?;
        Ok(segments
            .into_iter()
            .map(|s| SpeakerSegment {
                speaker: s.speaker.and_then(|spk| spk.parse::<i32>().ok()).unwrap_or(0),
                start: s.start,
                end: s.end,
                text: s.text,
            })
            .collect())
    }
}
