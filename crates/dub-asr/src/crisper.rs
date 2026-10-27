//! CrisperWhisper ASR движок через audio.cpp (audiocpp_server) + VoxLingua107 (transcribe.cpp).
//!
//! Высокоточное распознавание речи на 99 языках с пословными таймкодами.
//! Поддерживает режимы:
//! - "verbatim": 100% дословный текст без пропуска запинок
//! - "intended": очищенный литературный текст
//! Если язык оригинала установлен в "auto" или не задан — предварительно вызывается
//! VoxLingua107 на первых 30 секундах аудио для определения языка.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use audiocpp::{AudiocppClient, AudiocppServer, AudiocppServerOpts};
use crate::segment::{
    clean_bracket_tag, segment_words, segment_words_with_diarization, Segment, Word, SEG_MAX_DUR,
    SEG_MAX_GAP,
};
use crate::whisper_lang::whisper_code;
use crate::{AsrEngine, AsrError, SpeakerSegment, Turn};

pub const CRISPER_MODEL_ID: &str = "crisperwhisper";

#[derive(Clone)]
pub struct CrisperWhisperAsr {
    pub server_bin: PathBuf,
    pub model_path: PathBuf,
    pub tcpp_dll: PathBuf,
    pub langid_model: PathBuf,
    pub backend: String,
    pub mode: String, // "verbatim" | "intended"
    pub detected_language: Arc<Mutex<Option<String>>>,
    pub server_instance: Arc<Mutex<Option<AudiocppServer>>>,
}

impl std::fmt::Debug for CrisperWhisperAsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrisperWhisperAsr")
            .field("server_bin", &self.server_bin)
            .field("model_path", &self.model_path)
            .field("tcpp_dll", &self.tcpp_dll)
            .field("langid_model", &self.langid_model)
            .field("backend", &self.backend)
            .field("mode", &self.mode)
            .finish()
    }
}

impl CrisperWhisperAsr {
    pub fn new(
        server_bin: impl Into<PathBuf>,
        model_path: impl Into<PathBuf>,
        tcpp_dll: impl Into<PathBuf>,
        langid_model: impl Into<PathBuf>,
        backend: impl Into<String>,
        mode: impl Into<String>,
    ) -> Self {
        Self {
            server_bin: server_bin.into(),
            model_path: model_path.into(),
            tcpp_dll: tcpp_dll.into(),
            langid_model: langid_model.into(),
            backend: backend.into(),
            mode: mode.into(),
            detected_language: Arc::new(Mutex::new(None)),
            server_instance: Arc::new(Mutex::new(None)),
        }
    }

    /// Определить или нормализовать язык речи.
    /// Если "auto" или пустой -> определение через VoxLingua107 на первых 30 сек.
    fn resolve_language(&self, wav: &Path, lang: &str) -> Result<String, AsrError> {
        let chosen = lang.trim();
        if !chosen.is_empty() && !chosen.eq_ignore_ascii_case("auto") {
            let code = whisper_code(chosen).map_err(AsrError::Transcribe)?;
            *self.detected_language.lock().unwrap() = Some(code.to_string());
            return Ok(code.to_string());
        }

        // Авто-определение через VoxLingua107
        if !self.tcpp_dll.is_file() || !self.langid_model.is_file() {
            eprintln!(
                "[crisper] transcribe_engine.dll или langid не найдены (dll: {}, model: {}), fallback на 'en'",
                self.tcpp_dll.display(),
                self.langid_model.display()
            );
            *self.detected_language.lock().unwrap() = Some("en".to_string());
            return Ok("en".to_string());
        }

        let (pcm, _) = crate::load_wav_16k_mono(wav)?;
        let head_len = pcm.len().min(30 * 16_000);
        let head = &pcm[..head_len];

        let guess = audiocpp::tcpp::detect_language(&self.tcpp_dll, &self.langid_model, head, &[])
            .map_err(|e| AsrError::Transcribe(format!("VoxLingua107: {e}")))?;

        let top_code = guess
            .ranked
            .first()
            .map(|(c, _)| c.as_str())
            .ok_or_else(|| AsrError::Transcribe("VoxLingua107 не вернул кандидатов языка".into()))?;

        eprintln!("[crisper] VoxLingua107 определил язык: {top_code}");
        let whisper_lang = whisper_code(top_code).map_err(AsrError::Transcribe)?;
        *self.detected_language.lock().unwrap() = Some(whisper_lang.to_string());
        Ok(whisper_lang.to_string())
    }

    /// Получить или запустить сайдкар-сервер audio.cpp
    fn ensure_server(&self) -> Result<AudiocppClient, AsrError> {
        let mut opts = AudiocppServerOpts::new(&self.server_bin, &self.model_path);
        opts.model_family = "crisperwhisper".to_string();
        opts.model_id = CRISPER_MODEL_ID.to_string();
        opts.task = "asr".to_string();
        opts.backend = if self.backend == "cpu" { "cpu".to_string() } else { "cuda".to_string() };
        opts.default_request_options = Some(serde_json::json!({
            "return_timestamps": true,
            "transcription_mode": self.mode
        }));

        let mut lock = self.server_instance.lock().unwrap();
        if let Some(srv) = lock.as_ref() {
            if srv.can_reuse(&opts) {
                return Ok(srv.client().clone());
            }
        }

        let srv = AudiocppServer::start(opts)
            .map_err(|e| AsrError::Transcribe(format!("запуск audiocpp_server: {e}")))?;
        let client = srv.client().clone();
        *lock = Some(srv);
        Ok(client)
    }
}

fn turn_of(turns: &[Turn], mid: f64) -> usize {
    turns.iter().position(|t| mid >= t.start && mid <= t.end).unwrap_or_else(|| {
        turns
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (mid - (a.start + a.end) / 2.0)
                    .abs()
                    .total_cmp(&(mid - (b.start + b.end) / 2.0).abs())
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    })
}

impl AsrEngine for CrisperWhisperAsr {
    fn detected_language(&self) -> Option<String> {
        self.detected_language.lock().unwrap().clone()
    }

    fn transcribe(&mut self, wav: &Path, lang: &str) -> Result<Vec<Segment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        Ok(segment_words(&words, SEG_MAX_GAP, SEG_MAX_DUR))
    }

    fn transcribe_turns(&mut self, wav: &Path, turns: &[Turn], lang: &str) -> Result<Vec<SpeakerSegment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        if turns.is_empty() {
            return Ok(segment_words(&words, SEG_MAX_GAP, SEG_MAX_DUR)
                .into_iter()
                .map(|s| SpeakerSegment {
                    start: s.start,
                    end: s.end,
                    text: s.text,
                    speaker: 0,
                })
                .collect());
        }

        let mut buckets: Vec<Vec<Word>> = vec![Vec::new(); turns.len()];
        for w in words {
            let idx = turn_of(turns, (w.start + w.end) / 2.0);
            buckets[idx].push(w);
        }

        let mut out = Vec::new();
        for (i, ws) in buckets.into_iter().enumerate() {
            for s in segment_words(&ws, SEG_MAX_GAP, SEG_MAX_DUR) {
                out.push(SpeakerSegment {
                    start: s.start,
                    end: s.end,
                    text: s.text,
                    speaker: turns[i].speaker,
                });
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        Ok(out)
    }

    fn transcribe_words(&mut self, wav: &Path, lang: &str) -> Result<Vec<Word>, AsrError> {
        let resolved_lang = self.resolve_language(wav, lang)?;
        let client = self.ensure_server()?;

        let transcript = client
            .transcribe_details(CRISPER_MODEL_ID, wav, Some(&resolved_lang), None, &self.mode)
            .map_err(|e| AsrError::Transcribe(format!("транскрибация CrisperWhisper: {e}")))?;

        Ok(transcript
            .words
            .into_iter()
            .filter_map(|w| {
                // Игнорируем паразитные нулевые токены (длительность < 0.001с), возникающие на тишине
                if (w.end - w.start) < 0.001 {
                    return None;
                }
                // Очистка от неречевых тегов ([breath], [laughter], [UM], [cough] и т.д.)
                let clean_text = clean_bracket_tag(&w.word)?;
                Some(Word {
                    word: clean_text,
                    start: w.start,
                    end: w.end,
                    is_asr_boundary: false,
                })
            })
            .collect())
    }

    fn transcribe_with_diar(&mut self, wav: &Path, turns: &[Turn], lang: &str) -> Result<Vec<Segment>, AsrError> {
        let words = self.transcribe_words(wav, lang)?;
        Ok(segment_words_with_diarization(&words, turns, SEG_MAX_GAP, SEG_MAX_DUR))
    }
}
