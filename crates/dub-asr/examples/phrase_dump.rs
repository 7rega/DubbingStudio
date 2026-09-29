//! phrase_dump: инструмент сравнительной оценки качества сегментации
//! между новым алгоритмом DP (Natural Phrases) и baseline (segment_words_with_diarization + merge_short_turns).
//!
//! Использование:
//!   cargo run -p dub-asr --example phrase_dump -- --input path/to/input.json [ПАРАМЕТРЫ]
//!
//! Входной формат:
//!   JSON { "language": "ru", "units": [...], "diarization": [...], "config": {...} }
//!   (также поддерживается прямое чтение project.json).

use dub_asr::phrase::{
    self,
    config::SegmentationConfig,
    profile::ProfileRegistry,
    time::Seconds,
    types::{AsrUnit, DiarInterval, DiarizationResult, SpeechSpan},
};
use dub_asr::{segment_words_with_diarization, Word, SEG_MAX_DUR, SEG_MAX_GAP};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct InputUnit {
    #[serde(alias = "word")]
    pub text: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub is_whisper_boundary: bool,
    #[serde(default = "default_true")]
    pub can_split_after: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct InputInterval {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DiarizationInput {
    List(Vec<InputInterval>),
    Obj { intervals: Vec<InputInterval> },
}

impl DiarizationInput {
    pub fn into_intervals(self) -> Vec<InputInterval> {
        match self {
            Self::List(list) => list,
            Self::Obj { intervals } => intervals,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InputSpeechRegion {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InputPayload {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub units: Vec<InputUnit>,
    #[serde(default)]
    pub diarization: Option<DiarizationInput>,
    #[serde(default)]
    pub speech_regions: Option<Vec<InputSpeechRegion>>,
    #[serde(default)]
    pub vad: Option<Vec<InputSpeechRegion>>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,

    // Поддержка project.json
    #[serde(default)]
    pub tgt_lang: Option<String>,
    #[serde(default)]
    pub segments: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone)]
struct ParsedInput {
    language: String,
    units: Vec<AsrUnit>,
    diarization: DiarizationResult,
    speech_regions: Option<Vec<SpeechSpan>>,
    config_override: Option<serde_json::Value>,
}

fn parse_input(json_str: &str) -> Result<ParsedInput, Box<dyn std::error::Error>> {
    let payload: InputPayload = serde_json::from_str(json_str)?;

    // Если передан project.json (есть segments, но нет units)
    if payload.units.is_empty() && payload.segments.is_some() {
        return parse_project_json(payload);
    }

    let language = payload
        .language
        .or(payload.tgt_lang)
        .unwrap_or_else(|| "und".to_string());

    let units = payload
        .units
        .into_iter()
        .map(|u| AsrUnit {
            text: u.text,
            start: Seconds(u.start),
            end: Seconds(u.end),
            confidence: u.confidence,
            is_whisper_boundary: u.is_whisper_boundary,
            can_split_after: u.can_split_after,
        })
        .collect();

    let diarization_intervals = match payload.diarization {
        Some(d) => d
            .into_intervals()
            .into_iter()
            .map(|i| DiarInterval {
                speaker: i.speaker,
                start: Seconds(i.start),
                end: Seconds(i.end),
                confidence: i.confidence,
            })
            .collect(),
        None => Vec::new(),
    };

    let raw_spans = payload.speech_regions.or(payload.vad);
    let speech_regions = raw_spans.map(|spans| {
        spans
            .into_iter()
            .map(|s| SpeechSpan {
                start: Seconds(s.start),
                end: Seconds(s.end),
            })
            .collect()
    });

    Ok(ParsedInput {
        language,
        units,
        diarization: DiarizationResult {
            intervals: diarization_intervals,
        },
        speech_regions,
        config_override: payload.config,
    })
}

fn parse_project_json(payload: InputPayload) -> Result<ParsedInput, Box<dyn std::error::Error>> {
    let language = payload
        .language
        .or(payload.tgt_lang)
        .unwrap_or_else(|| "und".to_string());
    let segments = payload.segments.unwrap_or_default();

    let mut units = Vec::new();
    let mut intervals = Vec::new();

    for seg in segments {
        let speaker = seg
            .get("speaker")
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .to_string();
        let seg_start = seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let seg_end = seg.get("end").and_then(|v| v.as_f64()).unwrap_or(seg_start);

        intervals.push(DiarInterval {
            speaker,
            start: Seconds(seg_start),
            end: Seconds(seg_end),
            confidence: Some(1.0),
        });

        if let Some(words_arr) = seg
            .get("extra")
            .and_then(|e| e.get("words"))
            .and_then(|w| w.as_array())
        {
            let count = words_arr.len();
            for (idx, w) in words_arr.iter().enumerate() {
                let word_text = w
                    .get("word")
                    .or_else(|| w.get("text"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if word_text.is_empty() {
                    continue;
                }
                let start = w.get("start").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let end = w.get("end").and_then(|x| x.as_f64()).unwrap_or(start);
                let is_last = idx + 1 == count;
                units.push(AsrUnit {
                    text: word_text,
                    start: Seconds(start),
                    end: Seconds(end),
                    confidence: None,
                    is_whisper_boundary: is_last,
                    can_split_after: true,
                });
            }
        } else if let Some(src_text) = seg.get("src_text").and_then(|v| v.as_str()) {
            units.push(AsrUnit {
                text: src_text.to_string(),
                start: Seconds(seg_start),
                end: Seconds(seg_end),
                confidence: None,
                is_whisper_boundary: true,
                can_split_after: true,
            });
        }
    }

    eprintln!(
        "[phrase_dump] project.json распознан: извлечено {} юнитов и {} интервалов диаризации",
        units.len(),
        intervals.len()
    );

    Ok(ParsedInput {
        language,
        units,
        diarization: DiarizationResult { intervals },
        speech_regions: None,
        config_override: payload.config,
    })
}

// ---------------------------------------------------------------------------
// Baseline: воспроизведение merge_short_turns из crates/dub-server/src/analyze.rs
// ---------------------------------------------------------------------------

fn ends_sentence_text(text: &str) -> bool {
    let trimmed = text.trim_end_matches(|c: char| {
        c.is_whitespace()
            || c == '"'
            || c == '\''
            || c == '»'
            || c == '”'
            || c == '’'
            || c == ')'
            || c == ']'
            || c == '}'
    });
    trimmed.ends_with(['.', '!', '?', '…', '。', '！', '？', '؟', '۔']) || trimmed.ends_with("...")
}

fn merge_short_turns_baseline(segs: &mut Vec<dub_asr::Segment>) {
    if segs.len() < 2 {
        return;
    }
    const GAP: f64 = 0.35;
    const OVERLAP: f64 = 0.2;
    const SHORT: f64 = 1.6;
    const MAX_DUR: f64 = 12.0;
    const MAX_CH: usize = 200;

    let src = std::mem::take(segs);
    let mut out: Vec<dub_asr::Segment> = Vec::with_capacity(src.len());
    for s in src {
        if let Some(last) = out.last_mut() {
            let same_spk = last.speaker == s.speaker;
            let gap = s.start - last.end;
            let short = (last.end - last.start) < SHORT || (s.end - s.start) < SHORT;
            let dur_ok = (s.end - last.start) <= MAX_DUR;
            let ch_ok = last.text.chars().count() + s.text.chars().count() < MAX_CH;
            let last_finished = ends_sentence_text(&last.text);
            let can_merge = if last_finished {
                gap > -OVERLAP
                    && gap < 0.12
                    && ((last.end - last.start) < 0.8 || (s.end - s.start) < 0.8)
            } else {
                gap > -OVERLAP && gap < GAP && short
            };
            if same_spk && can_merge && dur_ok && ch_ok {
                let lt = last.text.trim_end();
                let rt = s.text.trim_start();
                let sep = if lt.is_empty() || rt.is_empty() {
                    ""
                } else {
                    " "
                };
                last.text = format!("{lt}{sep}{rt}");
                last.end = last.end.max(s.end);
                last.words.extend(s.words);
                continue;
            }
        }
        out.push(s);
    }
    *segs = out;
}

// ---------------------------------------------------------------------------
// Статистика и метрики качества
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct UtteranceStats {
    total: usize,
    under_1s: usize,
    from_1_to_2s: usize,
    from_2_to_8s: usize,
    from_8_to_15s: usize,
    over_15s: usize,
    under_0_6s: usize,
}

impl UtteranceStats {
    fn from_durations(durations: &[f64]) -> Self {
        let mut stats = Self {
            total: durations.len(),
            ..Default::default()
        };
        for &d in durations {
            if d < 1.0 {
                stats.under_1s += 1;
            } else if d < 2.0 {
                stats.from_1_to_2s += 1;
            } else if d < 8.0 {
                stats.from_2_to_8s += 1;
            } else if d < 15.0 {
                stats.from_8_to_15s += 1;
            } else {
                stats.over_15s += 1;
            }

            if d < 0.6 {
                stats.under_0_6s += 1;
            }
        }
        stats
    }

    fn share_2_to_8s(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.from_2_to_8s as f64 / self.total as f64) * 100.0
        }
    }
}

// ---------------------------------------------------------------------------
// Выходные структуры
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct DpUtteranceRow {
    index: usize,
    speaker: String,
    start: f64,
    end: f64,
    dur: f64,
    reason: String,
    is_oversize: bool,
    is_soft_max_exceeded: bool,
    cost: f64,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct BaselineUtteranceRow {
    index: usize,
    speaker: String,
    start: f64,
    end: f64,
    dur: f64,
    text: String,
}

// ---------------------------------------------------------------------------
// CLI аргументы и точка входа
// ---------------------------------------------------------------------------

struct CliArgs {
    input_path: Option<PathBuf>,
    output_json: bool,
    override_lambda: Option<f64>,
    override_ideal: Option<f64>,
    override_soft_max: Option<f64>,
    override_hard_max: Option<f64>,
    override_min: Option<f64>,
    override_weight_ideal: Option<f64>,
    override_weight_boundary: Option<f64>,
    override_weight_short: Option<f64>,
    override_weight_soft: Option<f64>,
    override_weight_hard: Option<f64>,
}

fn parse_cli_args() -> CliArgs {
    let mut args = CliArgs {
        input_path: None,
        output_json: false,
        override_lambda: None,
        override_ideal: None,
        override_soft_max: None,
        override_hard_max: None,
        override_min: None,
        override_weight_ideal: None,
        override_weight_boundary: None,
        override_weight_short: None,
        override_weight_soft: None,
        override_weight_hard: None,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--input" | "-i" => {
                args.input_path = it.next().map(PathBuf::from);
            }
            "--json" => {
                args.output_json = true;
            }
            "--lambda" | "--dp-segment-penalty" => {
                args.override_lambda = it.next().and_then(|v| v.parse().ok());
            }
            "--ideal" => {
                args.override_ideal = it.next().and_then(|v| v.parse().ok());
            }
            "--soft-max" => {
                args.override_soft_max = it.next().and_then(|v| v.parse().ok());
            }
            "--hard-max" => {
                args.override_hard_max = it.next().and_then(|v| v.parse().ok());
            }
            "--min" => {
                args.override_min = it.next().and_then(|v| v.parse().ok());
            }
            "--weight-ideal" => {
                args.override_weight_ideal = it.next().and_then(|v| v.parse().ok());
            }
            "--weight-boundary" => {
                args.override_weight_boundary = it.next().and_then(|v| v.parse().ok());
            }
            "--weight-short" => {
                args.override_weight_short = it.next().and_then(|v| v.parse().ok());
            }
            "--weight-soft" => {
                args.override_weight_soft = it.next().and_then(|v| v.parse().ok());
            }
            "--weight-hard" => {
                args.override_weight_hard = it.next().and_then(|v| v.parse().ok());
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                if args.input_path.is_none() && !other.starts_with('-') {
                    args.input_path = Some(PathBuf::from(other));
                } else {
                    eprintln!("Неизвестный аргумент: {other}");
                    print_help();
                    std::process::exit(2);
                }
            }
        }
    }

    args
}

fn print_help() {
    eprintln!(
        r#"phrase_dump — инструмент сравнительной оценки качества сегментации

ИСПОЛЬЗОВАНИЕ:
  cargo run -p dub-asr --example phrase_dump -- [ОПЦИИ]

ОПЦИИ:
  -i, --input <PATH>               Путь к JSON-файлу со словами и диаризацией (или project.json).
                                   Если не указан или "-", читается stdin.
      --json                       Вывести результат дополнительно в формате JSON.
      --lambda <F64>               Переопределить штраф за разрез реплики (dp_segment_penalty).
      --ideal <F64>                Идеальная длительность реплики в секундах (ideal_utterance_sec).
      --soft-max <F64>             Мягкий максимум длительности в секундах (soft_max_utterance_sec).
      --hard-max <F64>             Жёсткий потолок длительности в секундах (hard_max_utterance_sec).
      --min <F64>                  Минимум длительности в секундах (min_utterance_sec).
      --weight-ideal <F64>         Вес отклонения от идеала (dp_weight_ideal_dev).
      --weight-boundary <F64>      Вес награды за естественную границу (dp_weight_boundary_reward).
      --weight-short <F64>         Вес штрафа за короткую реплику (dp_weight_short_penalty).
      --weight-soft <F64>          Вес штрафа за превышение soft_max (dp_weight_soft_max_penalty).
      --weight-hard <F64>          Вес крутизны штрафа за hard_max (dp_weight_hard_max_slope).
  -h, --help                       Показать эту справку.
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse_cli_args();

    let json_content = match &cli.input_path {
        Some(p) if p.to_string_lossy() != "-" => std::fs::read_to_string(p)?,
        _ => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
    };

    if json_content.trim().is_empty() {
        eprintln!("Ошибка: входной JSON пуст.");
        std::process::exit(1);
    }

    let parsed = parse_input(&json_content)?;
    let profile = ProfileRegistry::get(Some(&parsed.language));

    // Настройка конфига DP
    let mut cfg = SegmentationConfig::default();
    if let Some(cfg_val) = parsed.config_override {
        if let Ok(deserialized) = serde_json::from_value::<SegmentationConfig>(cfg_val) {
            cfg = deserialized;
        }
    }

    // Применение CLI оверрайдов
    if let Some(val) = cli.override_lambda {
        cfg.dp_segment_penalty = val;
    }
    if let Some(val) = cli.override_ideal {
        cfg.ideal_utterance_sec = Seconds(val);
    }
    if let Some(val) = cli.override_soft_max {
        cfg.soft_max_utterance_sec = Seconds(val);
    }
    if let Some(val) = cli.override_hard_max {
        cfg.hard_max_utterance_sec = Seconds(val);
    }
    if let Some(val) = cli.override_min {
        cfg.min_utterance_sec = Seconds(val);
    }
    if let Some(val) = cli.override_weight_ideal {
        cfg.dp_weight_ideal_dev = val;
    }
    if let Some(val) = cli.override_weight_boundary {
        cfg.dp_weight_boundary_reward = val;
    }
    if let Some(val) = cli.override_weight_short {
        cfg.dp_weight_short_penalty = val;
    }
    if let Some(val) = cli.override_weight_soft {
        cfg.dp_weight_soft_max_penalty = val;
    }
    if let Some(val) = cli.override_weight_hard {
        cfg.dp_weight_hard_max_slope = val;
    }

    cfg.validate()?;

    // =========================================================================
    // 1. Пайплайн DP (Natural Phrases)
    // =========================================================================
    let aligned =
        phrase::align_and_smooth(&parsed.units, &parsed.diarization, &cfg, profile.as_ref());
    let turns = phrase::build_speaker_turns(&aligned, Some(&parsed.diarization));

    let mut dp_rows: Vec<DpUtteranceRow> = Vec::new();
    let mut row_idx = 1;

    for (turn_idx, turn) in turns.iter().enumerate() {
        let is_last_turn = turn_idx + 1 == turns.len();
        let dp_res = phrase::segment_turn_dp(
            turn,
            profile.as_ref(),
            &cfg,
            parsed.speech_regions.as_deref(),
            is_last_turn,
        );

        for seg in dp_res.segments {
            let start = turn.units[seg.start_idx].start.as_f64();
            let end = turn.units[seg.end_idx - 1].end.as_f64();
            let dur = (end - start).max(0.0);
            let unit_texts: Vec<&str> = turn.units[seg.start_idx..seg.end_idx]
                .iter()
                .map(|u| u.text.as_str())
                .collect();
            let text = profile.join_units(&unit_texts);

            dp_rows.push(DpUtteranceRow {
                index: row_idx,
                speaker: turn.speaker.clone(),
                start,
                end,
                dur,
                reason: format!("{:?}", seg.reason),
                is_oversize: seg.is_oversize,
                is_soft_max_exceeded: seg.is_soft_max_exceeded,
                cost: seg.cost.total_cost,
                text,
            });
            row_idx += 1;
        }
    }

    // =========================================================================
    // 2. Пайплайн Baseline (segment_words_with_diarization + merge_short_turns)
    // =========================================================================
    // Конвертация AsrUnit -> dub_asr::Word
    let baseline_words: Vec<Word> = parsed
        .units
        .iter()
        .map(|u| Word {
            word: u.text.clone(),
            start: u.start.as_f64(),
            end: u.end.as_f64(),
            is_asr_boundary: u.is_whisper_boundary,
        })
        .collect();

    // Карта спикеров: String -> i32 (для Turn) и i32 -> String (для восстановления)
    let mut spk_str_to_i32: HashMap<String, i32> = HashMap::new();
    let mut spk_i32_to_str: HashMap<i32, String> = HashMap::new();
    let mut next_spk_id = 0i32;

    let baseline_turns: Vec<dub_asr::Turn> = parsed
        .diarization
        .intervals
        .iter()
        .map(|d| {
            let spk_id = *spk_str_to_i32.entry(d.speaker.clone()).or_insert_with(|| {
                let id = next_spk_id;
                next_spk_id += 1;
                spk_i32_to_str.insert(id, d.speaker.clone());
                id
            });
            dub_asr::Turn {
                start: d.start.as_f64(),
                end: d.end.as_f64(),
                speaker: spk_id,
            }
        })
        .collect();

    let mut baseline_segs = segment_words_with_diarization(
        &baseline_words,
        &baseline_turns,
        SEG_MAX_GAP,
        SEG_MAX_DUR,
    );
    merge_short_turns_baseline(&mut baseline_segs);

    let mut baseline_rows: Vec<BaselineUtteranceRow> = Vec::new();
    for (i, s) in baseline_segs.into_iter().enumerate() {
        let dur = (s.end - s.start).max(0.0);
        let orig_speaker = s.speaker.as_ref().map(|spk_str| {
            if let Ok(id) = spk_str.parse::<i32>() {
                spk_i32_to_str
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| spk_str.clone())
            } else {
                spk_str.clone()
            }
        }).unwrap_or_else(|| "0".to_string());

        baseline_rows.push(BaselineUtteranceRow {
            index: i + 1,
            speaker: orig_speaker,
            start: s.start,
            end: s.end,
            dur,
            text: s.text,
        });
    }

    // =========================================================================
    // 3. Вычисление статистик
    // =========================================================================
    let dp_durations: Vec<f64> = dp_rows.iter().map(|r| r.dur).collect();
    let base_durations: Vec<f64> = baseline_rows.iter().map(|r| r.dur).collect();

    let dp_stats = UtteranceStats::from_durations(&dp_durations);
    let base_stats = UtteranceStats::from_durations(&base_durations);

    // =========================================================================
    // 4. Печать результатов
    // =========================================================================
    println!();
    println!("{}", "=".repeat(120));
    println!("PHRASE EVALUATION TOOL: DP (NATURAL PHRASES) vs BASELINE");
    println!("Language: {}, Total input units: {}, Diarization intervals: {}", parsed.language, parsed.units.len(), parsed.diarization.intervals.len());
    println!("{}", "=".repeat(120));
    println!();

    println!("--- 1. DP SEGMENTATION (Natural Phrases) ---");
    println!("{:<4} | {:<12} | {:<17} | {:<7} | {:<18} | {:<8} | {:<7} | {:<7} | Text",
        "#", "Speaker", "Start - End", "Dur(s)", "Reason", "Oversize", "SoftMax", "Cost"
    );
    println!("{}+{}+{}+{}+{}+{}+{}+{}+{}",
        "-".repeat(5), "-".repeat(14), "-".repeat(19), "-".repeat(9),
        "-".repeat(20), "-".repeat(10), "-".repeat(9), "-".repeat(9), "-".repeat(35)
    );

    for r in &dp_rows {
        println!("{:<4} | {:<12} | {:>7.3} - {:>7.3} | {:>7.3} | {:<18} | {:<8} | {:<7} | {:>7.3} | {}",
            r.index,
            r.speaker,
            r.start,
            r.end,
            r.dur,
            r.reason,
            if r.is_oversize { "YES" } else { "no" },
            if r.is_soft_max_exceeded { "YES" } else { "no" },
            r.cost,
            r.text
        );
    }
    println!();

    println!("--- 2. BASELINE SEGMENTATION (segment_words_with_diarization + merge_short_turns) ---");
    println!("{:<4} | {:<12} | {:<17} | {:<7} | Text",
        "#", "Speaker", "Start - End", "Dur(s)"
    );
    println!("{}+{}+{}+{}+{}",
        "-".repeat(5), "-".repeat(14), "-".repeat(19), "-".repeat(9), "-".repeat(65)
    );

    for r in &baseline_rows {
        println!("{:<4} | {:<12} | {:>7.3} - {:>7.3} | {:>7.3} | {}",
            r.index,
            r.speaker,
            r.start,
            r.end,
            r.dur,
            r.text
        );
    }
    println!();

    println!("{}", "=".repeat(120));
    println!("--- 3. СВОДКА СРАВНЕНИЯ КАЧЕСТВА (SUMMARY COMPARISON) ---");
    println!("{}", "=".repeat(120));
    println!("{:<35} | {:<25} | {:<25}",
        "Метрика", "Baseline", "DP (Natural Phrases)"
    );
    println!("{}+{}+{}",
        "-".repeat(36), "-".repeat(27), "-".repeat(27)
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Число реплик (Total)", base_stats.total, dp_stats.total
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Длительность < 1.0 с",
        format!("{} ({:.1}%)", base_stats.under_1s, pct(base_stats.under_1s, base_stats.total)),
        format!("{} ({:.1}%)", dp_stats.under_1s, pct(dp_stats.under_1s, dp_stats.total))
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Длительность 1.0 - 2.0 с",
        format!("{} ({:.1}%)", base_stats.from_1_to_2s, pct(base_stats.from_1_to_2s, base_stats.total)),
        format!("{} ({:.1}%)", dp_stats.from_1_to_2s, pct(dp_stats.from_1_to_2s, dp_stats.total))
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Длительность 2.0 - 8.0 с (sweet)",
        format!("{} ({:.1}%)", base_stats.from_2_to_8s, pct(base_stats.from_2_to_8s, base_stats.total)),
        format!("{} ({:.1}%)", dp_stats.from_2_to_8s, pct(dp_stats.from_2_to_8s, dp_stats.total))
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Длительность 8.0 - 15.0 с",
        format!("{} ({:.1}%)", base_stats.from_8_to_15s, pct(base_stats.from_8_to_15s, base_stats.total)),
        format!("{} ({:.1}%)", dp_stats.from_8_to_15s, pct(dp_stats.from_8_to_15s, dp_stats.total))
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Длительность > 15.0 с (oversize)",
        format!("{} ({:.1}%)", base_stats.over_15s, pct(base_stats.over_15s, base_stats.total)),
        format!("{} ({:.1}%)", dp_stats.over_15s, pct(dp_stats.over_15s, dp_stats.total))
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Доля реплик 2.0 - 8.0 с",
        format!("{:.1}%", base_stats.share_2_to_8s()),
        format!("{:.1}%", dp_stats.share_2_to_8s())
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Число oversize (> 15.0 с)", base_stats.over_15s, dp_stats.over_15s
    );
    println!("{:<35} | {:<25} | {:<25}",
        "Число ультракоротких (< 0.6 с)", base_stats.under_0_6s, dp_stats.under_0_6s
    );
    println!("{}", "=".repeat(120));
    println!();

    if cli.output_json {
        let json_out = serde_json::json!({
            "language": parsed.language,
            "dp": {
                "utterances": dp_rows,
                "stats": {
                    "total": dp_stats.total,
                    "under_1s": dp_stats.under_1s,
                    "from_1_to_2s": dp_stats.from_1_to_2s,
                    "from_2_to_8s": dp_stats.from_2_to_8s,
                    "from_8_to_15s": dp_stats.from_8_to_15s,
                    "over_15s": dp_stats.over_15s,
                    "share_2_to_8s_percent": dp_stats.share_2_to_8s(),
                    "under_0_6s": dp_stats.under_0_6s,
                }
            },
            "baseline": {
                "utterances": baseline_rows,
                "stats": {
                    "total": base_stats.total,
                    "under_1s": base_stats.under_1s,
                    "from_1_to_2s": base_stats.from_1_to_2s,
                    "from_2_to_8s": base_stats.from_2_to_8s,
                    "from_8_to_15s": base_stats.from_8_to_15s,
                    "over_15s": base_stats.over_15s,
                    "share_2_to_8s_percent": base_stats.share_2_to_8s(),
                    "under_0_6s": base_stats.under_0_6s,
                }
            }
        });
        println!("JSON_OUTPUT_START");
        println!("{}", serde_json::to_string_pretty(&json_out)?);
        println!("JSON_OUTPUT_END");
    }

    Ok(())
}

fn pct(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}
