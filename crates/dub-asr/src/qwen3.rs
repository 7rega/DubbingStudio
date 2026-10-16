//! Qwen3-ASR + Qwen3 Forced Aligner движок через audio.cpp (audiocpp_cli.exe).
//!
//! Интеграция связки Qwen3 ASR (1.7B) и Qwen3 Forced Aligner (0.6B) со встроенным Silero VAD.
//! Пайплайн работает в 3 этапа:
//! 1. Standalone Silero VAD через audiocpp_cli (`--task vad --family silero_vad`) с тюнингованными
//!    параметрами (`threshold=0.50`, `speech_pad_ms=250`, `min_silence_duration_ms=1000`, `min_speech_duration_ms=300`, `max_seconds=30`).
//!    Это полностью отсекает фоновую музыку, песни и шум, предотвращая зацикливания и галлюцинации,
//!    при этом надёжно захватывая тихие реплики и имена в начале фраз (например, «Aaron»).
//! 2. Нарезка аудио-чанков по границам VAD в 16 кГц моно WAV.
//! 3. Пакетное распознавание всех чанков через `--request-sequence` в единой CUDA-сессии с Forced Aligner
//!    (без штрафа на повторную загрузку весов в VRAM), с автоматическим маппингом таймстемпов на глобальное
//!    время и сквозным переносом капитализации и пунктуации.
//!
//! Для диаризации слова автоматически маппятся на turns Nemotron-3 или Sortformer
//! через единый алгоритм segment_words_with_diarization.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use serde::Deserialize;

use crate::segment::{segment_words, segment_words_with_diarization, Segment, Word, SEG_MAX_DUR};
use crate::{AsrEngine, AsrError, SpeakerSegment, Turn};

pub const QWEN_MAX_GAP: f64 = 0.50;

#[derive(Clone, Debug)]
pub struct Qwen3Asr {
    pub cli_bin: PathBuf,
    pub asr_model: PathBuf,
    pub aligner_model: PathBuf,
    pub vad_model: PathBuf,
    pub backend: String,
    pub detected_language: Arc<Mutex<Option<String>>>,
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

#[derive(Deserialize, Debug)]
struct VadChunk {
    #[allow(dead_code)]
    index: usize,
    start_sample: u64,
    end_sample: u64,
}

struct TempDirGuard<'a>(&'a Path);

impl<'a> Drop for TempDirGuard<'a> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0);
    }
}

fn normalize_asr_language(lang: &str) -> Option<String> {
    let lower = lang.trim().to_lowercase();
    match lower.as_str() {
        "" | "auto" => None,
        "en" | "eng" | "english" => Some("English".into()),
        "ru" | "rus" | "russian" => Some("Russian".into()),
        "de" | "deu" | "ger" | "german" => Some("German".into()),
        "fr" | "fra" | "fre" | "french" => Some("French".into()),
        "es" | "spa" | "spanish" => Some("Spanish".into()),
        "it" | "ita" | "italian" => Some("Italian".into()),
        "pt" | "por" | "portuguese" => Some("Portuguese".into()),
        "ja" | "jpn" | "japanese" => Some("Japanese".into()),
        "ko" | "kor" | "korean" => Some("Korean".into()),
        "zh" | "zho" | "chi" | "chinese" => Some("Chinese".into()),
        "yue" | "cantonese" => Some("Cantonese".into()),
        "ar" | "ara" | "arabic" => Some("Arabic".into()),
        "id" | "ind" | "indonesian" => Some("Indonesian".into()),
        "tr" | "tur" | "turkish" => Some("Turkish".into()),
        "vi" | "vie" | "vietnamese" => Some("Vietnamese".into()),
        "th" | "tha" | "thai" => Some("Thai".into()),
        "pl" | "pol" | "polish" => Some("Polish".into()),
        "nl" | "nld" | "dut" | "dutch" => Some("Dutch".into()),
        "sv" | "swe" | "swedish" => Some("Swedish".into()),
        "da" | "dan" | "danish" => Some("Danish".into()),
        "fi" | "fin" | "finnish" => Some("Finnish".into()),
        "cs" | "ces" | "cze" | "czech" => Some("Czech".into()),
        "el" | "ell" | "gre" | "greek" => Some("Greek".into()),
        "hu" | "hun" | "hungarian" => Some("Hungarian".into()),
        "ro" | "ron" | "rum" | "romanian" => Some("Romanian".into()),
        "hi" | "hin" | "hindi" => Some("Hindi".into()),
        other => Some(other.to_string()),
    }
}

fn write_wav_16k_mono(path: &Path, samples: &[f32]) -> Result<(), AsrError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| AsrError::Io(e.to_string()))?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(v).map_err(|e| AsrError::Io(e.to_string()))?;
    }
    writer.finalize().map_err(|e| AsrError::Io(e.to_string()))?;
    Ok(())
}

fn normalize_word(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF      // CJK Unified Ideographs
        | 0x3400..=0x4DBF    // CJK Extension A
        | 0x20000..=0x2A6DF  // CJK Extension B
        | 0xAC00..=0xD7AF    // Hangul Syllables
        | 0x3040..=0x309F    // Hiragana
        | 0x30A0..=0x30FF    // Katakana
    )
}

struct BatchChunkOutput {
    raw_words: Vec<RawQwenWord>,
    text_output: String,
}

fn parse_batch_stdout(stdout: &str) -> HashMap<usize, BatchChunkOutput> {
    let mut results: HashMap<usize, BatchChunkOutput> = HashMap::new();
    let mut cur_idx: Option<usize> = None;
    let mut cur_words: Vec<RawQwenWord> = Vec::new();
    let mut cur_text = String::new();

    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("request_index=") {
            if let Some(idx) = cur_idx.take() {
                results.insert(idx, BatchChunkOutput {
                    raw_words: std::mem::take(&mut cur_words),
                    text_output: std::mem::take(&mut cur_text),
                });
            }
            if let Ok(idx) = rest.parse::<usize>() {
                cur_idx = Some(idx);
            }
        } else if let Some(rest) = line.strip_prefix("word_timestamps=") {
            if let Ok(parsed) = serde_json::from_str::<Vec<RawQwenWord>>(rest) {
                cur_words = parsed;
            }
        } else if let Some(rest) = line.strip_prefix("text_output=") {
            cur_text = rest.to_string();
        }
    }
    if let Some(idx) = cur_idx.take() {
        results.insert(idx, BatchChunkOutput {
            raw_words: cur_words,
            text_output: cur_text,
        });
    }
    results
}

impl Qwen3Asr {
    pub fn new(
        cli_bin: impl AsRef<Path>,
        asr_model: impl AsRef<Path>,
        aligner_model: impl AsRef<Path>,
        vad_model: impl AsRef<Path>,
        backend: impl Into<String>,
    ) -> Self {
        Self {
            cli_bin: cli_bin.as_ref().to_path_buf(),
            asr_model: asr_model.as_ref().to_path_buf(),
            aligner_model: aligner_model.as_ref().to_path_buf(),
            vad_model: vad_model.as_ref().to_path_buf(),
            backend: backend.into(),
            detected_language: Arc::new(Mutex::new(None)),
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
        if !self.vad_model.is_file() {
            return Err(AsrError::Transcribe(format!(
                "модель Silero VAD не найдена: {}",
                self.vad_model.display()
            )));
        }
        if !wav.is_file() {
            return Err(AsrError::Transcribe(format!(
                "WAV-файл не найден: {}",
                wav.display()
            )));
        }

        let temp_dir = std::env::temp_dir()
            .join("dub_qwen3_asr")
            .join(format!(
                "{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ));
        std::fs::create_dir_all(&temp_dir).map_err(|e| AsrError::Io(e.to_string()))?;
        let _guard = TempDirGuard(&temp_dir);

        let (audio_samples, _) = crate::load_wav_16k_mono(wav)?;
        if audio_samples.is_empty() {
            return Ok(Vec::new());
        }

        let backend = match self.backend.to_lowercase().as_str() {
            "cpu" => "cpu",
            _ => "cuda",
        };

        // 1. Standalone Silero VAD для надежного вычленения речи
        let vad_chunks_file = temp_dir.join("vad_chunks.json");
        let mut vad_cmd = Command::new(&self.cli_bin);
        vad_cmd.arg("--task").arg("vad");
        vad_cmd.arg("--family").arg("silero_vad");
        vad_cmd.arg("--model").arg(&self.vad_model);
        vad_cmd.arg("--backend").arg(backend);
        vad_cmd.arg("--audio").arg(wav);
        vad_cmd.arg("--request-option").arg("threshold=0.50");
        vad_cmd.arg("--request-option").arg("speech_pad_ms=250");
        vad_cmd.arg("--request-option").arg("min_silence_duration_ms=1000");
        vad_cmd.arg("--request-option").arg("min_speech_duration_ms=300");
        vad_cmd.arg("--vad-chunk-max-seconds").arg("30");
        vad_cmd.arg("--vad-chunks-out").arg(&vad_chunks_file);

        if let Some(parent) = self.cli_bin.parent() {
            vad_cmd.current_dir(parent);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            vad_cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let vad_output = vad_cmd.output().map_err(|e| {
            AsrError::Transcribe(format!("ошибка запуска Silero VAD: {e}"))
        })?;

        if !vad_output.status.success() {
            let stderr = String::from_utf8_lossy(&vad_output.stderr);
            let stdout = String::from_utf8_lossy(&vad_output.stdout);
            return Err(AsrError::Transcribe(format!(
                "Silero VAD завершился со сбоем (status: {}): {stderr} {stdout}",
                vad_output.status
            )));
        }

        let vad_json_bytes = std::fs::read(&vad_chunks_file).unwrap_or_default();
        let chunks: Vec<VadChunk> = serde_json::from_slice(&vad_json_bytes).unwrap_or_default();
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Нарезка аудио по VAD-окнам и формирование запросов
        let target_lang = normalize_asr_language(lang);
        let mut reqs = Vec::with_capacity(chunks.len());

        for (i, c) in chunks.iter().enumerate() {
            let st = c.start_sample as usize;
            let en = (c.end_sample as usize).min(audio_samples.len());
            if en <= st {
                continue;
            }

            let chunk_wav = temp_dir.join(format!("chunk_{i:04}.wav"));
            write_wav_16k_mono(&chunk_wav, &audio_samples[st..en])?;

            let mut req = serde_json::json!({
                "audio": chunk_wav.to_string_lossy(),
                "temperature": 0,
                "options": {
                    "return_timestamps": true,
                    "clamp_timestamps_to_audio": true,
                    "preserve_punctuation": true
                }
            });

            if let Some(ref l) = target_lang {
                req["language"] = serde_json::Value::String(l.clone());
            }

            reqs.push(req);
        }

        if reqs.is_empty() {
            return Ok(Vec::new());
        }

        let req_seq_path = temp_dir.join("req_seq.json");
        let req_seq_bytes = serde_json::to_vec(&reqs).map_err(|e| AsrError::Io(e.to_string()))?;
        std::fs::write(&req_seq_path, req_seq_bytes).map_err(|e| AsrError::Io(e.to_string()))?;

        // 3. Пакетный запуск Qwen3 ASR + Forced Aligner в единой CUDA-сессии
        let mut asr_cmd = Command::new(&self.cli_bin);
        asr_cmd.arg("--task").arg("asr");
        asr_cmd.arg("--family").arg("qwen3_asr");
        asr_cmd.arg("--model").arg(&self.asr_model);
        asr_cmd.arg("--backend").arg(backend);
        asr_cmd.arg("--session-option").arg(format!(
            "qwen3_asr.forced_aligner_path={}",
            self.aligner_model.display()
        ));
        asr_cmd.arg("--request-sequence").arg(&req_seq_path);

        if let Some(parent) = self.cli_bin.parent() {
            asr_cmd.current_dir(parent);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            asr_cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let asr_output = asr_cmd.output().map_err(|e| {
            AsrError::Transcribe(format!("ошибка запуска audiocpp_cli ASR: {e}"))
        })?;

        if !asr_output.status.success() {
            let stderr = String::from_utf8_lossy(&asr_output.stderr);
            let stdout = String::from_utf8_lossy(&asr_output.stdout);
            return Err(AsrError::Transcribe(format!(
                "audiocpp_cli ASR завершился со сбоем (status: {}): {stderr} {stdout}",
                asr_output.status
            )));
        }

        // 4. Разбор результатов и пересчет локальных сэмплов чанков в глобальное время
        let stdout_str = String::from_utf8_lossy(&asr_output.stdout);
        let mut results_by_idx = parse_batch_stdout(&stdout_str);

        let mut all_words: Vec<Word> = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let chunk_base_sec = chunk.start_sample as f64 / 16000.0;
            if let Some(chunk_res) = results_by_idx.remove(&i) {
                if chunk_res.raw_words.is_empty() {
                    continue;
                }
                let tokens: Vec<&str> = chunk_res.text_output.split_whitespace().collect();
                let mut tok_idx = 0;

                for (j, entry) in chunk_res.raw_words.iter().enumerate() {
                    let rel_start = entry.start_sample as f64 / 16000.0;
                    let rel_end = entry.end_sample as f64 / 16000.0;
                    if rel_end <= rel_start {
                        continue;
                    }
                    let start = chunk_base_sec + rel_start;
                    let end = chunk_base_sec + rel_end;

                    let e_norm = normalize_word(&entry.word);
                    let mut word_text = entry.word.trim();

                    if !e_norm.is_empty() {
                        let lookahead = (tokens.len().saturating_sub(tok_idx)).min(6);
                        for offset in 0..lookahead {
                            let t_pos = tok_idx + offset;
                            let t_norm = normalize_word(tokens[t_pos]);
                            if t_norm == e_norm || (t_norm.starts_with(&e_norm) && t_norm.len() > e_norm.len()) {
                                word_text = tokens[t_pos].trim();
                                tok_idx = t_pos + 1;
                                break;
                            }
                        }
                    }

                    if !word_text.is_empty() {
                        // Подавление CJK-галлюцинаций (звукоподражания 嗯, 啊 на вздохах/стонах):
                        // если целевой язык проекта не восточно-азиатский, отбрасываем слова,
                        // состоящие только из CJK-иероглифов и пунктуации.
                        let is_cjk_target = target_lang
                            .as_deref()
                            .map(|l| matches!(l, "Chinese" | "Cantonese" | "Japanese" | "Korean"))
                            .unwrap_or(false);
                        if !is_cjk_target && word_text.chars().any(is_cjk) {
                            let letters: Vec<char> = word_text.chars().filter(|c| c.is_alphabetic()).collect();
                            if !letters.is_empty() && letters.iter().all(|&c| is_cjk(c)) {
                                continue;
                            }
                        }

                        let mut word = Word::new(word_text, start, end);
                        let next_gap = chunk_res.raw_words
                            .get(j + 1)
                            .map(|next| (next.start_sample as f64 - entry.end_sample as f64) / 16000.0)
                            .unwrap_or(1.0);
                        let is_boundary = next_gap >= 0.45
                            || word_text.ends_with(['.', '!', '?', '…'])
                            || word_text.ends_with("...");
                        if is_boundary {
                            word = word.with_boundary(true);
                        }
                        all_words.push(word);
                    }
                }
            }
        }

        if let Some(ref l) = target_lang {
            if let Ok(mut lock) = self.detected_language.lock() {
                *lock = Some(l.clone());
            }
        }

        Ok(all_words)
    }

    fn transcribe(&mut self, wav: &Path, lang: &str) -> Result<Vec<Segment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        Ok(segment_words(&words, QWEN_MAX_GAP, SEG_MAX_DUR))
    }

    fn transcribe_with_diar(
        &mut self,
        wav: &Path,
        turns: &[Turn],
        lang: &str,
    ) -> Result<Vec<Segment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        Ok(segment_words_with_diarization(&words, turns, QWEN_MAX_GAP, SEG_MAX_DUR))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_language() {
        assert_eq!(normalize_asr_language("en"), Some("English".into()));
        assert_eq!(normalize_asr_language("ru"), Some("Russian".into()));
        assert_eq!(normalize_asr_language("auto"), None);
        assert_eq!(normalize_asr_language(""), None);
    }

    #[test]
    fn test_parse_batch_stdout() {
        let stdout = r#"
family=qwen3_asr
task=asr
mode=offline
request_index=0
request_id=request_0
[TIMING] request.request_0.wall_ms 425.266
word_timestamps=[{"start_sample":33408,"end_sample":37248,"word":"Yeah","confidence":0}]
text_output=Yeah.
request_index=1
request_id=request_1
[TIMING] request.request_1.wall_ms 10.227
text_output=
"#;
        let parsed = parse_batch_stdout(stdout);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[&0].text_output, "Yeah.");
        assert_eq!(parsed[&0].raw_words.len(), 1);
        assert_eq!(parsed[&0].raw_words[0].word, "Yeah");
        assert_eq!(parsed[&1].text_output, "");
        assert_eq!(parsed[&1].raw_words.len(), 0);
    }
}
