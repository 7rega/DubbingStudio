//! Multilingual forced alignment using Qwen3 Forced Aligner (audio.cpp GGUF).
//! Supports 11 languages (Russian, English, German, French, Spanish, Italian,
//! Portuguese, Japanese, Korean, Chinese, Cantonese) on CUDA / CPU.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = "qwen3-forced-aligner-v1";
pub const COMPONENT: &str = "alignment-q8_0";
pub const MODEL_FILE_NAME: &str = "qwen3-forced-aligner-0.6b-q8_0.gguf";
pub const MODEL_SUBDIR: &str = "alignment";
pub const MODEL_REL_PATH: &str = "models/alignment/qwen3-forced-aligner-0.6b-q8_0.gguf";
pub const MODEL_SIZE: u64 = 1_129_966_496;
pub const MODEL_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/main/Qwen3-ForcedAligner-0.6B-GGUF/qwen3-forced-aligner-0.6b-q8_0.gguf?download=true";

pub fn resolve_model_file(models_root: &Path) -> Option<PathBuf> {
    // Строгая проверка в подпапке models/alignment
    let direct = models_root.join(MODEL_SUBDIR).join(MODEL_FILE_NAME);
    if direct.is_file() {
        return Some(direct);
    }
    let with_models = models_root.join("models").join(MODEL_SUBDIR).join(MODEL_FILE_NAME);
    if with_models.is_file() {
        return Some(with_models);
    }
    None
}

pub fn model_ready(models_root: &Path) -> bool {
    resolve_model_file(models_root).is_some()
}

pub fn verified_model_ready(models_root: &Path) -> bool {
    resolve_model_file(models_root)
        .and_then(|p| std::fs::metadata(&p).ok())
        .map(|m| m.len() == MODEL_SIZE)
        .unwrap_or(false)
}

pub fn resolve_audiocpp_cli(models_root: &Path) -> Option<PathBuf> {
    if let Ok(v) = std::env::var("DUB_STUDIO_AUDIOCPP_CLI") {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }
    let name = if cfg!(windows) { "audiocpp_cli.exe" } else { "audiocpp_cli" };
    let mut candidates = vec![
        models_root.join("tools").join("audiocpp").join(name),
        models_root.join("tools").join(name),
        models_root.join(name),
        PathBuf::from("tools/audiocpp").join(name),
    ];
    if let Some(parent) = models_root.parent() {
        candidates.push(parent.join("tools").join("audiocpp").join(name));
        candidates.push(parent.join("tools").join(name));
    }
    // Также проверяем стандартную рабочую директорию DubStudio
    candidates.push(PathBuf::from("F:\\DubStudio\\tools\\audiocpp").join(name));
    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }
    None
}

pub fn normalize_language(lang: &str) -> Result<String, String> {
    let l = lang.trim().to_lowercase();
    match l.as_str() {
        "ru" | "rus" | "russian" | "русский" | "рус" => Ok("Russian".into()),
        "en" | "eng" | "english" | "английский" | "англ" => Ok("English".into()),
        "de" | "ger" | "german" | "немецкий" => Ok("German".into()),
        "fr" | "fre" | "french" | "французский" => Ok("French".into()),
        "es" | "spa" | "spanish" | "испанский" => Ok("Spanish".into()),
        "it" | "ita" | "italian" | "итальянский" => Ok("Italian".into()),
        "pt" | "por" | "portuguese" | "португальский" => Ok("Portuguese".into()),
        "ja" | "jpn" | "japanese" | "японский" => Ok("Japanese".into()),
        "ko" | "kor" | "korean" | "корейский" => Ok("Korean".into()),
        "zh" | "chi" | "chinese" | "китайский" => Ok("Chinese".into()),
        "yue" | "cantonese" | "кантонский" => Ok("Cantonese".into()),
        "auto" | "" => Err("ALIGN_LANGUAGE_REQUIRED: Не указан язык для выравнивания речи.".into()),
        other => Err(format!(
            "ALIGN_LANGUAGE_UNSUPPORTED: {other}. Модель поддерживает: Russian, English, German, French, Spanish, Italian, Portuguese, Japanese, Korean, Chinese, Cantonese."
        )),
    }
}

pub fn round_ms(t: f64) -> f64 {
    (t * 1000.0).round() / 1000.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub id: String,
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aligned {
    pub start: f64,
    pub end: f64,
    pub words: Vec<TimedWord>,
    pub review: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub id: String,
    pub aligned: Option<Aligned>,
    pub reason: Option<String>,
}

impl Outcome {
    pub fn skipped(id: &str, reason: &str) -> Self {
        Self {
            id: id.into(),
            aligned: None,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Deserialize, Debug)]
struct RawWordTimestamp {
    start_sample: u64,
    end_sample: u64,
    #[allow(dead_code)]
    word: String,
    #[serde(default)]
    confidence: Option<f32>,
}

pub struct Aligner {
    pub model_path: PathBuf,
    pub cli_bin: PathBuf,
    pub backend: String,
    pub language: String,
}

impl Aligner {
    pub fn load(models_root: &Path) -> Result<Self, String> {
        let model_path = resolve_model_file(models_root)
            .ok_or_else(|| "ALIGN_MODEL_MISSING: Модель «Выравнивание речи Qwen3 (Q8_0)» не найдена. Установите её в меню компонентов.".to_string())?;
        let cli_bin = resolve_audiocpp_cli(models_root)
            .ok_or_else(|| "ALIGN_CLI_MISSING: audiocpp_cli не найден в папке tools/audiocpp.".to_string())?;
        let backend = std::env::var("DUB_ALIGN_BACKEND").unwrap_or_else(|_| "cuda".to_string());
        Ok(Self {
            model_path,
            cli_bin,
            backend,
            language: "Russian".to_string(),
        })
    }

    pub fn set_language(&mut self, lang: &str) {
        if let Ok(normalized) = normalize_language(lang) {
            self.language = normalized;
        }
    }

    pub fn align(
        &mut self,
        inputs: &[Input],
        samples: &[f32],
        progress: &dyn Fn(usize, usize),
    ) -> Result<Vec<Outcome>, String> {
        self.align_with_language(inputs, samples, &self.language.clone(), progress)
    }

    pub fn align_with_language(
        &mut self,
        inputs: &[Input],
        samples: &[f32],
        lang: &str,
        progress: &dyn Fn(usize, usize),
    ) -> Result<Vec<Outcome>, String> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let target_lang = normalize_language(lang)?;
        let duration = samples.len() as f64 / 16000.0;
        let mut outcomes: Vec<Outcome> = inputs
            .iter()
            .map(|s| Outcome::skipped(&s.id, "not_aligned"))
            .collect();

        // 1. Фильтрация валидных сегментов
        let valid: Vec<bool> = inputs
            .iter()
            .map(|s| {
                s.start.is_finite()
                    && s.end.is_finite()
                    && s.start >= 0.0
                    && s.end > s.start
                    && s.end <= duration + 0.1
                    && !s.text.trim().is_empty()
            })
            .collect();

        for (i, ok) in valid.iter().enumerate() {
            if !ok {
                outcomes[i].reason = Some("unsupported_text_bounds_or_overlap".into());
            }
        }

        let valid_indices: Vec<usize> = (0..inputs.len()).filter(|&i| valid[i]).collect();
        if valid_indices.is_empty() {
            return Ok(outcomes);
        }

        // 2. Создание временной директории для сэмплов
        let temp_dir = std::env::temp_dir().join(format!(
            "dub_align_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("ALIGN_IO_TEMP_DIR: {e}"))?;

        struct SnippetMeta {
            index: usize,
            win_start: f64,
            prev_end: f64,
            next_start: f64,
        }

        let envelope = crate::speech_edges::Envelope::new(samples);
        let mut requests = Vec::new();
        let mut metas = Vec::new();

        // 3. Нарезка адаптивных окон звука и подготовка requests.json
        for &i in &valid_indices {
            let s = &inputs[i];
            let prev_end = if i > 0 { inputs[i - 1].end } else { 0.0 };
            let next_start = if i + 1 < inputs.len() { inputs[i + 1].start } else { duration };

            // Проверяем наличие мертвой тишины перед репликой:
            // если перед s.start тишина (RMS < 0.0025), отступ минимальный (50 мс),
            // чтобы Qwen3 не прилипал к сэмплу 0 тишины на коротких словах (Oi. и др.)
            let silence_check_start = (s.start - 0.35).max(prev_end);
            let is_silent_before = envelope.max_rms(silence_check_start, s.start) < 0.0025;

            let pad_left = if is_silent_before {
                0.05
            } else {
                (0.35f64).min((0.10f64).max((s.start - prev_end) * 0.5))
            };
            let pad_right = (0.60f64).min((0.15f64).max((next_start - s.end) * 0.7));

            let win_start = (s.start - pad_left).max(0.0);
            let win_end = (s.end + pad_right).min(duration);

            let a = (win_start * 16000.0) as usize;
            let b = ((win_end * 16000.0) as usize).min(samples.len());

            if b.saturating_sub(a) < 160 {
                continue;
            }

            let seg_wav = temp_dir.join(format!("seg_{i}.wav"));
            if let Err(e) = write_wav_16k_mono(&seg_wav, &samples[a..b]) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(format!("ALIGN_IO_WAV: {e}"));
            }

            let req_id = format!("req_{i}");
            requests.push(serde_json::json!({
                "id": req_id,
                "audio": seg_wav.to_string_lossy(),
                "language": target_lang,
                "text": s.text.trim()
            }));

            metas.push(SnippetMeta {
                index: i,
                win_start,
                prev_end,
                next_start,
            });
        }

        let seq_file = temp_dir.join("requests.json");
        let seq_data = serde_json::json!({ "requests": requests });
        if let Err(e) = std::fs::write(&seq_file, serde_json::to_vec(&seq_data).unwrap_or_default()) {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(format!("ALIGN_IO_SEQ: {e}"));
        }

        // 4. Потоковый запуск audiocpp_cli.exe с отображением прогресса в реальном времени
        let run_and_stream = |backend: &str| -> Result<std::collections::HashMap<String, Vec<RawWordTimestamp>>, (String, String)> {
            let mut cmd = Command::new(&self.cli_bin);
            cmd.arg("--task").arg("align");
            cmd.arg("--family").arg("qwen3_forced_aligner");
            cmd.arg("--model").arg(&self.model_path);
            cmd.arg("--backend").arg(backend);
            cmd.arg("--request-sequence").arg(&seq_file);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            if let Some(parent) = self.cli_bin.parent() {
                cmd.current_dir(parent);
            }

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }

            let mut child = cmd.spawn().map_err(|e| (format!("ALIGN_SPAWN: {e}"), String::new()))?;
            let child_stdout = child.stdout.take().ok_or_else(|| ("ALIGN_SPAWN_STDOUT".into(), String::new()))?;

            let mut results_by_req: std::collections::HashMap<String, Vec<RawWordTimestamp>> =
                std::collections::HashMap::new();
            let mut cur_req_id = None;
            let mut completed = 0;
            let total_reqs = metas.len();

            use std::io::BufRead;
            let reader = std::io::BufReader::new(child_stdout);
            for line_res in reader.lines() {
                if let Ok(line) = line_res {
                    if let Some(id_part) = line.strip_prefix("request_id=") {
                        cur_req_id = Some(id_part.trim().to_string());
                    } else if let Some(ts_part) = line.strip_prefix("word_timestamps=") {
                        if let Some(id) = cur_req_id.take() {
                            if let Ok(words) = serde_json::from_str::<Vec<RawWordTimestamp>>(ts_part.trim()) {
                                results_by_req.insert(id, words);
                                completed += 1;
                                progress(completed, total_reqs);
                            }
                        }
                    }
                }
            }

            let status = child.wait().map_err(|e| (format!("ALIGN_WAIT: {e}"), String::new()))?;
            if !status.success() {
                let mut err_msg = String::new();
                if let Some(mut child_stderr) = child.stderr.take() {
                    use std::io::Read;
                    let _ = child_stderr.read_to_string(&mut err_msg);
                }
                return Err((format!("ALIGN_CLI_EXIT: code {:?}", status.code()), err_msg));
            }

            Ok(results_by_req)
        };

        let results_by_req = match run_and_stream(&self.backend) {
            Ok(res) => res,
            Err((_err, stderr)) if self.backend != "cpu" => {
                // Фоллбэк на CPU, если GPU-запуск вернул ошибку
                match run_and_stream("cpu") {
                    Ok(cpu_res) => cpu_res,
                    Err((_, cpu_stderr)) => {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        let out_err = if !cpu_stderr.is_empty() { cpu_stderr } else { stderr };
                        return Err(format!("ALIGN_CLI_FAILED: {out_err}"));
                    }
                }
            }
            Err((err, stderr)) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                let out_err = if !stderr.is_empty() { stderr } else { err };
                return Err(format!("ALIGN_CLI_FAILED: {out_err}"));
            }
        };

        // 5. Очистка временных файлов
        let _ = std::fs::remove_dir_all(&temp_dir);

        // 6. Построение Aligned с акустической огибающей и гарантией валидности границ
        let total_requests = metas.len();
        for (step, meta) in metas.into_iter().enumerate() {
            let req_id = format!("req_{}", meta.index);
            let s_text = &inputs[meta.index].text;
            let orig_tokens: Vec<&str> = s_text.split_whitespace().collect();

            if orig_tokens.is_empty() {
                outcomes[meta.index] = Outcome::skipped(&inputs[meta.index].id, "empty_text");
                progress(step + 1, total_requests);
                continue;
            }

            if let Some(raw_words) = results_by_req.get(&req_id) {
                if raw_words.is_empty() {
                    outcomes[meta.index] = Outcome::skipped(&inputs[meta.index].id, "empty_words");
                } else if raw_words.len() != orig_tokens.len() {
                    // Число распознанных слов не совпадает с числом токенов (valid_words требует строгого совпадения)
                    outcomes[meta.index] = Outcome::skipped(&inputs[meta.index].id, "word_count_mismatch");
                } else {
                    let mut timed_words = Vec::with_capacity(raw_words.len());
                    let mut prev_word_end = 0.0f64;

                    for (k, w) in raw_words.iter().enumerate() {
                        let mut w_start = round_ms(meta.win_start + (w.start_sample as f64 / 16000.0));
                        let mut w_end = round_ms(meta.win_start + (w.end_sample as f64 / 16000.0));

                        // Гарантируем монотонность и отсутствие нахлёстов между соседними словами
                        if w_start < prev_word_end {
                            w_start = prev_word_end;
                        }
                        if w_end < w_start {
                            w_end = w_start;
                        }
                        w_start = w_start.min(duration);
                        w_end = w_end.min(duration);
                        prev_word_end = w_end;

                        let score = w.confidence.unwrap_or(1.0).clamp(0.0, 1.0);
                        timed_words.push(TimedWord {
                            word: orig_tokens[k].to_string(),
                            start: w_start,
                            end: w_end,
                            score,
                        });
                    }

                    // Акустическое уточнение:
                    // 1) Отсечение предречевой тишины на первом слове
                    let trimmed_start = envelope.trim_start_silence(timed_words[0].start, timed_words[0].end);
                    timed_words[0].start = trimmed_start;
                    if timed_words[0].end < timed_words[0].start {
                        timed_words[0].end = timed_words[0].start;
                    }

                    // 2) Акустическая калибровка последнего слова:
                    let last_idx = timed_words.len() - 1;
                    // а) Отсечение наведённого хвоста фонового шума, если Qwen3 залез в тишину/шум комнаты
                    let last_trimmed = envelope.trim_end_silence(timed_words[last_idx].start, timed_words[last_idx].end);
                    timed_words[last_idx].end = last_trimmed.max(timed_words[last_idx].start);

                    // б) Ведение огибающей для реально затянутых гласных и выкриков
                    let hi = (meta.next_start - 0.02).min(duration).max(timed_words[last_idx].end);
                    let exp_end = envelope.expand_tail(&timed_words[last_idx], hi);
                    timed_words[last_idx].end = exp_end.max(timed_words[last_idx].start).min(hi);

                    let first_start = timed_words[0].start;
                    let last_end = timed_words.last().unwrap().end;

                    // Границы сегмента: безопасный запас, строго first_start <= aligned_start и last_end >= aligned_end
                    let aligned_start = (first_start - 0.020).max(meta.prev_end).min(first_start);
                    let aligned_start = round_ms(aligned_start.max(0.0)).min(first_start);

                    let aligned_end = (last_end + 0.020).min(meta.next_start).max(last_end);
                    let aligned_end = round_ms(aligned_end.min(duration)).max(last_end);

                    if aligned_end > aligned_start {
                        let review = timed_words.iter().any(|w| w.score < 0.3);
                        outcomes[meta.index] = Outcome {
                            id: inputs[meta.index].id.clone(),
                            aligned: Some(Aligned {
                                start: aligned_start,
                                end: aligned_end,
                                words: timed_words,
                                review,
                            }),
                            reason: None,
                        };
                    } else {
                        outcomes[meta.index] = Outcome::skipped(&inputs[meta.index].id, "implausible_bounds");
                    }
                }
            } else {
                outcomes[meta.index] = Outcome::skipped(&inputs[meta.index].id, "alignment_failed");
            }

            progress(step + 1, total_requests);
        }

        // 7. Коррекция граничных акустических запасов между соседними сегментами (для исключения микронахлёстов)
        for i in 1..inputs.len() {
            if outcomes[i - 1].aligned.is_some() && outcomes[i].aligned.is_some() {
                let p_end = outcomes[i - 1].aligned.as_ref().unwrap().end;
                let c_start = outcomes[i].aligned.as_ref().unwrap().start;
                if p_end > c_start && inputs[i - 1].end <= inputs[i].start {
                    let p_last_w = outcomes[i - 1].aligned.as_ref().unwrap().words.last().unwrap().end;
                    let c_first_w = outcomes[i].aligned.as_ref().unwrap().words.first().unwrap().start;
                    if p_last_w <= c_first_w {
                        let mid = round_ms((p_last_w + c_first_w) / 2.0);
                        if let Some(a_prev) = outcomes[i - 1].aligned.as_mut() {
                            a_prev.end = mid.max(p_last_w);
                        }
                        if let Some(a_cur) = outcomes[i].aligned.as_mut() {
                            a_cur.start = mid.min(c_first_w);
                        }
                    }
                }
            }
        }

        // Разрешение оставшихся нахлёстов (если слова физически накладываются)
        loop {
            let mut reject = Vec::new();
            for i in 1..inputs.len() {
                let prev = outcomes[i - 1].aligned.as_ref().map(|a| a.end).unwrap_or(inputs[i - 1].end);
                let next = outcomes[i].aligned.as_ref().map(|a| a.start).unwrap_or(inputs[i].start);
                if prev > next && inputs[i - 1].end <= inputs[i].start {
                    if outcomes[i - 1].aligned.is_some() {
                        reject.push(i - 1);
                    }
                    if outcomes[i].aligned.is_some() {
                        reject.push(i);
                    }
                }
            }
            if reject.is_empty() {
                break;
            }
            for i in reject {
                outcomes[i] = Outcome::skipped(&inputs[i].id, "neighbour_conflict");
            }
        }

        Ok(outcomes)
    }
}

fn write_wav_16k_mono(path: &Path, samples: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(v).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_language() {
        assert_eq!(normalize_language("ru").unwrap(), "Russian");
        assert_eq!(normalize_language("rus").unwrap(), "Russian");
        assert_eq!(normalize_language("russian").unwrap(), "Russian");
        assert_eq!(normalize_language("en").unwrap(), "English");
        assert_eq!(normalize_language("eng").unwrap(), "English");
        assert_eq!(normalize_language("de").unwrap(), "German");
        assert_eq!(normalize_language("fr").unwrap(), "French");
        assert_eq!(normalize_language("es").unwrap(), "Spanish");
        assert_eq!(normalize_language("it").unwrap(), "Italian");
        assert_eq!(normalize_language("pt").unwrap(), "Portuguese");
        assert_eq!(normalize_language("ja").unwrap(), "Japanese");
        assert_eq!(normalize_language("ko").unwrap(), "Korean");
        assert_eq!(normalize_language("zh").unwrap(), "Chinese");
        assert_eq!(normalize_language("yue").unwrap(), "Cantonese");
        assert!(normalize_language("auto").is_err());
        assert!(normalize_language("").is_err());
        assert!(normalize_language("unsupported_xyz").is_err());
    }
}

