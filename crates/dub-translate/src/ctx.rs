//! Порт dubengine/ctx_translate.py run() — единый Gemma-проход: (1) vision layout -> sub_style/sub_y/
//! titles/brands/captions; (2) vision scene-контекст; (3) audio-контекст (окна <=28с); (4) перевод ВСЕГО
//! транскрипта (+тайтлы) С полным vision+audio контекстом. Каждая фаза fail-safe: упавшая фаза даёт пустой
//! контекст, перевод всё равно случается. Промпты TP/AP перенесены ДОСЛОВНО.

use std::path::{Path, PathBuf};

use base64::Engine;
use regex::Regex;
use serde_json::Value;

use dub_llm::{strip_think, ChatClient, Message, Part, Sampling};

use crate::seg::Seg;
use crate::vision;
use crate::TranslateError;

/// _LANG из ctx_translate — код -> имя (для vision/перевода). Линейный поиск по срезу (как lang_name
/// в translate.rs) — без построения HashMap на каждый вызов.
fn lang_name(code: &str) -> String {
    let lc = code.to_lowercase();
    crate::WHISPER_LANGS
        .iter()
        .find(|(k, _)| *k == lc.as_str())
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| code.to_string())
}

/// Результат ctx-перевода: заполненные segs + extra (vision/audio/scene) как в питоне.
pub struct CtxResult {
    /// sub_style / sub_y / titles(+tgt) / brands / captions / audio_context / scene_context.
    pub extra: Value,
}

/// Конфиг ctx-прохода — минимальные поля cfg питона, нужные тут.
pub struct CtxConfig {
    pub input: PathBuf,     // исходное видео
    pub work_dir: PathBuf,  // рабочий каталог (_ctx_kf.png)
    pub tgt_lang: String,
    pub vocals16: Option<PathBuf>, // вокал для audio-контекста (может отсутствовать в порту — separation в раунде 4)
    pub vh: f64,            // высота кадра
    pub total: f64,         // длительность
    /// Нужен ли VISION-layout (sub_style/titles/brands). Его выход используется ТОЛЬКО при вжигании
    /// субтитров/титров — в режимах без субтитров это 10-20 лишних vision-вызовов Gemma (минуты на
    /// длинном видео) ради данных, которые никто не прочитает. false -> фаза 1 пропускается целиком.
    pub want_layout: bool,
    /// Стилевая инструкция перевода (#112): доп-указание тона/регистра/лексики. Пусто = без стиля.
    /// Вставляется в инструкционную часть TP-промпта ПЕРЕД форматом-контрактом (он остаётся приоритетным).
    pub style: String,
}

/// run — единый проход. rewrite=Some(instr) -> творческий ре-дубляж; None -> точный перевод.
/// llm — уже поднятый клиент к llama-server с mmproj. Пишет segs[i].tgt и возвращает extra.
pub fn run(
    llm: &ChatClient,
    cfg: &CtxConfig,
    segs: &mut [Seg],
    rewrite: Option<&str>,
    mut log: impl FnMut(&str),
) -> Result<CtxResult, TranslateError> {
    let tgt = lang_name(&cfg.tgt_lang);
    let tmp = cfg.work_dir.join("_ctx_kf.png");

    let mut extra = serde_json::json!({
        "sub_style": Value::Null, "sub_y": Value::Null, "titles": [], "captions": [],
        "brands": [], "audio_context": "", "scene_context": ""
    });

    // ── фаза 1: VISION layout — ТОЛЬКО если субтитры/титры будут вжигаться ─
    // (гейт по режиму: в «без субтитров»/burn=off выход layout никем не используется, а это 2 vision-
    // вызова на каждый из 5-10 кейфреймов = минуты Gemma на длинном видео впустую).
    if cfg.want_layout {
        match vision::analyze_layout(llm, &cfg.input, &tmp, cfg.total, cfg.vh) {
            Ok(layout) => {
                extra["sub_style"] = layout.sub_style.unwrap_or(Value::Null);
                extra["sub_y"] = layout.sub_y.map(|y| Value::from(y)).unwrap_or(Value::Null);
                extra["titles"] = Value::Array(layout.titles.clone());
                extra["captions"] = Value::Array(layout.captions);
                extra["brands"] = Value::Array(layout.brands.clone());
                let tnames: Vec<String> = layout.titles.iter().filter_map(|t| t.get("text").and_then(|x| x.as_str()).map(String::from)).collect();
                let bnames: Vec<String> = layout.brands.iter().filter_map(|b| b.get("text").and_then(|x| x.as_str()).map(String::from)).collect();
                log(&format!("  ctx vision: sub_style={} titles={:?} brands={:?}", extra["sub_style"], tnames, bnames));
            }
            Err(e) => log(&format!("  ctx vision skipped: {e}")),
        }
    } else {
        log("  ctx vision layout: пропущен (субтитры не вжигаются — раскладка не нужна)");
    }

    // ── фаза 2: VISION scene-контекст ──────────────────────────────────────
    match vision::scene_context(llm, &cfg.input, &tmp, cfg.total, &tgt) {
        Ok(sc) => extra["scene_context"] = Value::from(sc),
        Err(e) => log(&format!("  ctx scene skipped: {e}")),
    }

    // ── фаза 3: AUDIO-контекст (окна <=28с). Fail-safe: нет вокала / модель не умеет audio -> пусто ──
    if let Some(vocals) = &cfg.vocals16 {
        match audio_context(llm, vocals, &tgt) {
            Ok(ac) if !ac.is_empty() => extra["audio_context"] = Value::from(ac),
            Ok(_) => {}
            Err(e) => log(&format!("  ctx audio skipped: {e}")),
        }
    }

    // ── фаза 4: TRANSLATE весь транскрипт (+тайтлы) С контекстом ───────────
    let n_seg = segs.len();
    let title_texts: Vec<String> = extra["titles"].as_array().map(|a| {
        a.iter().filter_map(|t| t.get("text").and_then(|x| x.as_str()).map(String::from)).collect()
    }).unwrap_or_default();

    // Единый список исходных строк (речь + тайтлы) в ГЛОБАЛЬНОЙ нумерации 1..N (тайтлы после речи), как
    // раньше. Хранится без "N. " префикса — нумеруем локально внутри чанка при отправке.
    let mut line_texts: Vec<String> = segs.iter().map(|s| s.text.trim().to_string()).collect();
    line_texts.extend(title_texts.iter().cloned());

    // Бюджет символов на строку (#107): 14 симв/сек × длительность сегмента — мягкий лимит для укладки
    // перевода в тайминг слота. У тайтлов длительности нет (None -> без лимита в промпте).
    let mut budgets: Vec<Option<usize>> = segs.iter().map(|s| char_budget(s.end - s.start)).collect();
    budgets.extend(std::iter::repeat(None).take(title_texts.len()));

    let mut ctx = String::new();
    if let Some(sc) = extra["scene_context"].as_str() {
        if !sc.is_empty() {
            ctx += &format!("=== VISUAL SCENE ===\n{sc}\n\n");
        }
    }
    if let Some(ac) = extra["audio_context"].as_str() {
        if !ac.is_empty() {
            ctx += &format!("=== AUDIO (tone/slang/speakers) ===\n{ac}\n\n");
        }
    }

    // Батч-перевод длинного скрипта (#82): чанки по бюджету + скользящий контекст + term-lock глоссарий.
    // Возвращает by_n = {глобальный_N -> перевод} — тот же контракт, что раньше давал единый вызов.
    let by_n = translate_lines(llm, &line_texts, &budgets, &tgt, rewrite, &cfg.style, &ctx, &mut log)?;

    for (i, s) in segs.iter_mut().enumerate() {
        let t = by_n.get(&(i + 1)).cloned().unwrap_or_default();
        s.tgt = if t.is_empty() { s.text.trim().to_string() } else { t };
    }
    // переводы тайтлов идут после речевых строк.
    if let Some(arr) = extra["titles"].as_array_mut() {
        for (j, ttl) in arr.iter_mut().enumerate() {
            let default = ttl.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let tr = by_n.get(&(n_seg + j + 1)).cloned().unwrap_or(default);
            ttl.as_object_mut().map(|o| o.insert("tgt".into(), Value::from(tr)));
        }
    }

    let _ = std::fs::remove_file(&tmp);
    Ok(CtxResult { extra })
}

// ── Батч-перевод длинного скрипта (#82) ────────────────────────────────────
// Порог «короткого» транскрипта: до и включая — один чанк (ПАРИТЕТ со старым единым Gemma-вызовом);
// выше — режем на чанки по бюджету строк/символов со скользящим контекстом.
const SHORT_LINES: usize = 30;
// Бюджет чанка при длинном скрипте: не больше MAX_LINES строк И не больше ~MAX_CHARS символов исходника.
const MAX_LINES: usize = 10;
const MAX_CHARS: usize = 600;
// Скользящий контекст: сколько уже-переведённых строк прошлого чанка и сырых строк следующего показать.
const CTX_BEFORE: usize = 3;
const CTX_AFTER: usize = 2;

/// Границы чанков [start,end) над line_texts. len<=SHORT_LINES -> один чанк (паритет). Иначе жадно
/// пакуем по MAX_LINES строк / MAX_CHARS символов (минимум 1 строка на чанк, даже если она длиннее бюджета).
fn chunk_bounds(line_texts: &[String]) -> Vec<(usize, usize)> {
    let n = line_texts.len();
    if n <= SHORT_LINES {
        return if n == 0 { vec![] } else { vec![(0, n)] };
    }
    let mut bounds = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = start;
        let mut chars = 0usize;
        while end < n {
            let add = line_texts[end].chars().count();
            // всегда берём хотя бы одну строку; далее — пока в оба бюджета влезаем
            if end > start && (end - start >= MAX_LINES || chars + add > MAX_CHARS) {
                break;
            }
            chars += add;
            end += 1;
        }
        bounds.push((start, end));
        start = end;
    }
    bounds
}

/// Плотность речи для бюджета длины (#107): ~14 символов/сек комфортной дикции. Бюджет строки =
/// round(14 × длительность_сек); длительность ≤0 (нет таймингов) -> None (без лимита).
const CHARS_PER_SEC: f64 = 14.0;
const MIN_BUDGET: usize = 12; // пол бюджета (#116): «(≤3)» на междометиях искажает перевод
pub(crate) fn char_budget(dur: f64) -> Option<usize> {
    if dur > 0.0 {
        Some(((dur * CHARS_PER_SEC).round() as usize).max(MIN_BUDGET))
    } else {
        None
    }
}

/// Вычистить ведущий маркер лимита «(≤NN)» из перевода. Устойчиво (#116): пробелы после «(» и перед
/// числом, варианты ≤/<=/=<. Цифры/скобки в самом переводе не трогаем.
pub(crate) fn strip_budget_marker(s: &str) -> String {
    let re = Regex::new(r"^\s*\(\s*(?:\u{2264}|<=|=<)\s*\d+\s*\)\s*").unwrap();
    re.replace(s, "").into_owned()
}

/// Перевести все строки чанками со скользящим контекстом и глоссарием (term-lock).
/// Ключи результата — ГЛОБАЛЬНЫЕ номера строк 1..line_texts.len() (как by_n у старого единого вызова).
fn translate_lines(
    llm: &ChatClient,
    line_texts: &[String],
    budgets: &[Option<usize>],
    tgt: &str,
    rewrite: Option<&str>,
    style: &str,
    ctx: &str,
    log: &mut impl FnMut(&str),
) -> Result<std::collections::HashMap<usize, String>, TranslateError> {
    let mut by_n: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    if line_texts.is_empty() {
        return Ok(by_n);
    }

    // Страховка от переполнения n_ctx: блок контекста (scene+audio) приклеивается к КАЖДОМУ чанку.
    // Если он раздулся (любой будущий источник) — обрезаем по бюджету символов, а не роняем перевод.
    const CTX_CHAR_BUDGET: usize = 6000; // ≈1.5-2К токенов; n_ctx=12288 остаётся с запасом под строки+ответ
    let ctx_capped: String;
    let ctx: &str = if ctx.chars().count() > CTX_CHAR_BUDGET {
        ctx_capped = format!(
            "{}\n[context truncated]\n\n",
            ctx.chars().take(CTX_CHAR_BUDGET).collect::<String>()
        );
        log(&format!(
            "  ctx translate: блок контекста {} симв. -> обрезан до {} (защита n_ctx)",
            ctx.chars().count(),
            CTX_CHAR_BUDGET
        ));
        &ctx_capped
    } else {
        ctx
    };

    let bounds = chunk_bounds(line_texts);
    // Глоссарий/term-lock — ТОЛЬКО на длинном скрипте (>1 чанка). На коротком (1 чанк) в HEAD глоссария
    // НЕ было → gloss_suffix в промпте + лишние Gemma-вызовы + term_lock над выходом меняли бы старый
    // единый вызов = нарушение питон-паритета (ревью-находка F). При 1 чанке gloss пуст → suffix пуст →
    // term_lock no-op → промпт и выход совпадают со старым путём.
    let gloss = if bounds.len() > 1 {
        crate::translate::glossary_pairs(
            llm,
            line_texts.iter().map(|s| s.as_str()),
            &crate::translate::name_src(""),
            tgt,
        )
        .unwrap_or_default()
    } else {
        Default::default()
    };
    let gloss_suffix = crate::translate::glossary_suffix(&gloss);
    if bounds.len() > 1 {
        log(&format!("  ctx translate: {} строк -> {} чанков (глоссарий: {} терм.)", line_texts.len(), bounds.len(), gloss.len()));
    }

    // re: 'N. <line>' — держим ПОСЛЕДНЕЕ вхождение номера (питон dict-comprehension).
    let re = Regex::new(r"(?m)^\s*(\d+)[.)\]:]\s*(.+?)\s*$").unwrap();

    // БРОНЕБОЙНЫЙ перевод (BORROWINGS #5 / VideoLingo per-chunk degrade): каждый чанк переводим отдельно
    // из рабочего стека. Если запрос НЕ влез в контекст модели (или ЛЮБАЯ ошибка) — рубим чанк пополам и
    // кладём половины обратно в стек; так до 1 строки. Дошли до 1 строки и всё равно сбой — оставляем строку
    // без перевода (фолбэк на исходник в run()), но ОСТАЛЬНЫЕ переводим. Раньше `?` ронял ВЕСЬ перевод при
    // первом же переполнении (все N строк пустые) — теперь один сбой затрагивает только свою строку.
    let mut stack: Vec<(usize, usize)> = bounds.iter().rev().cloned().collect();
    let mut ok_lines = 0usize;
    let mut fail_lines = 0usize;
    while let Some((start, end)) = stack.pop() {
        let clen = end - start;
        if clen == 0 {
            continue;
        }
        // локально-нумерованный блок 1..clen для этого чанка (маленькие номера надёжнее больших).
        // После номера — мягкий лимит символов «(≤NN)» из бюджета строки (#107); у строк без бюджета
        // (тайтлы) лимита нет. Лимит вычищается из ответа защитно (strip_budget_marker).
        let numbered = line_texts[start..end]
            .iter()
            .enumerate()
            .map(|(k, t)| match budgets.get(start + k).copied().flatten() {
                Some(lim) => format!("{}. (\u{2264}{lim}) {t}", k + 1),
                None => format!("{}. {t}", k + 1),
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Скользящий контекст: CTX_BEFORE уже-переведённых строк прошлого чанка (src -> tgt) + CTX_AFTER
        // сырых исходных строк следующего чанка. Только как СПРАВКА для связности, НЕ переводить их.
        let mut ctx_block = String::new();
        if start > 0 {
            let b0 = start.saturating_sub(CTX_BEFORE);
            let prev: Vec<String> = (b0..start)
                .map(|gi| {
                    let tr = by_n.get(&(gi + 1)).map(|s| s.as_str()).unwrap_or("");
                    format!("{} => {}", line_texts[gi], tr)
                })
                .collect();
            if !prev.is_empty() {
                ctx_block += &format!("=== PREVIOUS LINES (already translated, for continuity — do NOT re-output) ===\n{}\n\n", prev.join("\n"));
            }
        }
        if end < line_texts.len() {
            let a1 = (end + CTX_AFTER).min(line_texts.len());
            let after: Vec<String> = line_texts[end..a1].to_vec();
            if !after.is_empty() {
                ctx_block += &format!("=== UPCOMING LINES (context only — do NOT translate) ===\n{}\n\n", after.join("\n"));
            }
        }

        // TP — ДОСЛОВНО как в старом едином вызове (rewrite -> творческий; иначе точный перевод), плюс
        // суффикс глоссария и блок скользящего контекста. При одном чанке (короткий скрипт) блоки пусты =>
        // промпт совпадает со старым по формулировке.
        // Мягкий бюджет длины (#107): если после номера в скобках стоит «(≤NN)» — уложиться в NN символов;
        // при нехватке места убирать вводные слова и дубли, НЕ выдумывать факты. Скобку в ответ не писать.
        let budget_rule = " After each number, a parenthesis like (\u{2264}45) gives a soft character limit for that \
line — stay within it: if it doesn't fit, drop filler words and repetitions, keep the meaning, invent nothing. \
Do NOT copy the (\u{2264}NN) marker into your output.";
        // Стиль (#112) — доп-инструкция в инструкционной части ПЕРЕД форматом-контрактом «Output ONLY 'N. …'»
        // (он остаётся финальным и приоритетным, поэтому стиль не ломает разбор нумерованного ответа).
        let style_c = crate::translate::style_clause(style);
        let tp = if let Some(instr) = rewrite {
            format!(
                "You are a creative scriptwriter writing a BRAND-NEW voice-over script in {tgt} for this video. \
IGNORE the literal meaning of the source lines — they are ONLY a rhythm/length template. Write a completely NEW \
script whose CONTENT follows this instruction: \"{instr}\". Every line must fit the instruction, NOT translate the \
source. Keep the SAME number of lines and each line about the SAME LENGTH (it will be dubbed to fit the timing).{budget_rule}{style_c} \
Use the scene/audio context below for tone.{gloss_suffix}\n\n{ctx}{ctx_block}=== LINES (rhythm template) ===\n{numbered}\n\nOutput ONLY 'N. <line>' per line, nothing else."
            )
        } else {
            format!(
                "Translate EACH numbered line into natural, spoken {tgt} for dubbing — keep the order and the \
numbering, match tone/slang/intent.{budget_rule}{style_c} Use ALL the context below (what the words alone don't convey):{gloss_suffix}\n\n\
{ctx}{ctx_block}=== LINES ===\n{numbered}\n\nOutput ONLY 'N. <translation>' per line, nothing else."
            )
        };

        // mt (макс. выход) капим — не резервировать гигантский n_predict из контекста на большой чанк.
        let mt = (80 + 45 * clen).min(2048) as u32;
        let s = Sampling::new(0.2, 0.95, mt).top_k(64);
        match llm.chat(&[Message::user_text(tp)], &s) {
            Ok(resp) => {
                let raw = strip_think(&resp);
                // Парсим локальные номера 1..clen -> глобальный (start+k). Term-lock применяем к выходу.
                let mut local: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
                for c in re.captures_iter(&raw) {
                    if let Ok(k) = c[1].parse::<usize>() {
                        // защита: модель могла протащить маркер лимита «(≤NN)» в перевод -> вычищаем.
                        local.insert(k, term_lock(&strip_budget_marker(c[2].trim()), &gloss));
                    }
                }
                let mut got = 0usize;
                for k in 1..=clen {
                    if let Some(t) = local.get(&k) {
                        by_n.insert(start + k, t.clone());
                        got += 1;
                    }
                }
                ok_lines += got;
                fail_lines += clen - got; // не вернула модель -> фолбэк на исходник в run()
            }
            Err(e) => {
                if clen > 1 {
                    // не влезло / ошибка -> рубим пополам и повторяем (pop-порядок: сначала левая половина)
                    let mid = start + clen / 2;
                    stack.push((mid, end));
                    stack.push((start, mid));
                    log(&format!("  ctx translate: чанк [{}..{}) не влёз ({e}) -> дроблю пополам", start + 1, end));
                } else {
                    // одна строка и всё равно сбой -> оставляем как есть (исходник), НЕ рушим остальные
                    fail_lines += 1;
                    log(&format!("  ctx translate: строка {} не переведена ({e}) -> оставлена как есть", start + 1));
                }
            }
        }
    }
    if line_texts.len() > SHORT_LINES {
        log(&format!("  ctx translate: готово — {ok_lines} строк переведено, {fail_lines} на исходнике"));
    }
    Ok(by_n)
}

/// term-lock: если исходное ИМЯ утекло в перевод непереведённым — заменить на его целевую форму из
/// глоссария (посл. страховка к промпту "Keep these names consistent"). Регистрозависимо, целыми словами.
fn term_lock(line: &str, gloss: &[(String, String)]) -> String {
    if gloss.is_empty() {
        return line.to_string();
    }
    let mut out = line.to_string();
    for (src, dst) in gloss {
        if src == dst || !out.contains(src.as_str()) {
            continue;
        }
        out = replace_word(&out, src, dst);
    }
    out
}

/// Заменить целые вхождения `src` (границы — не буквенно-цифровой символ) на `dst`. Не трогает src внутри
/// более длинных слов (напр. "Sam" в "Samples"). Учитывает Unicode-алфавит для границ.
fn replace_word(hay: &str, src: &str, dst: &str) -> String {
    let bytes_ok = |c: char| c.is_alphanumeric();
    let mut out = String::with_capacity(hay.len());
    let mut rest = hay;
    while let Some(pos) = rest.find(src) {
        let before_ok = rest[..pos].chars().next_back().map_or(true, |c| !bytes_ok(c));
        let after = &rest[pos + src.len()..];
        let after_ok = after.chars().next().map_or(true, |c| !bytes_ok(c));
        out.push_str(&rest[..pos]);
        if before_ok && after_ok {
            out.push_str(dst);
        } else {
            out.push_str(src);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// AUDIO-контекст — нарезка вокала на окна <=28с и запрос input_audio. Fail-safe вызывающим.
/// AP — ДОСЛОВНО из ctx_translate.py.
fn audio_context(llm: &ChatClient, vocals: &Path, tgt: &str) -> Result<String, TranslateError> {
    let mut reader = hound::WavReader::open(vocals).map_err(|e| TranslateError::Audio(e.to_string()))?;
    let spec = reader.spec();
    let sr = spec.sample_rate as usize;
    if sr == 0 {
        return Err(TranslateError::Audio("WAV sample rate is 0".into()));
    }
    // читаем как f32 mono (микшируем каналы усреднением, как d.mean(axis=1) в питоне)
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap_or(0) as f32 / max).collect()
        }
    };
    let mono: Vec<f32> = if ch > 1 {
        samples.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
    } else {
        samples
    };

    let ap = format!(
        "Helping a translator dub to {tgt}. Listen; give context the transcript MISSES (do NOT \
transcribe): situation, tone/register (slang/sarcasm/anger/flirt/...), each speaker gender+vibe, \
slang/idioms and their real meaning here. 4-7 bullets."
    );
    let win = 28 * sr;
    // КАП числа окон: старый `while` шёл по ВСЕМУ файлу (22 мин = 49 окон × до 320 ток. ответа ≈ 15.7К
    // токенов) — этот текст приклеивался к КАЖДОМУ чанку перевода и переполнял n_ctx=12288 (реальный
    // фейл: «request 16946 tokens exceeds 12288» → весь перевод пустой). Теперь окон максимум
    // AC_MAX_WIN, равномерно по файлу. Файл ≤ AC_MAX_WIN окон (≈2.3 мин) — окна подряд, как раньше
    // (паритет коротких). Длиннее — сэмплируем: тон/сленг/вайб спикеров не требуют каждой секунды.
    const AC_MAX_WIN: usize = 5;
    let n_total = mono.len().div_ceil(win).max(1);
    let starts: Vec<usize> = if n_total <= AC_MAX_WIN {
        (0..n_total).map(|k| k * win).collect()
    } else {
        (0..AC_MAX_WIN)
            .map(|k| {
                let fr = k as f64 / (AC_MAX_WIN - 1) as f64; // 0.0 .. 1.0
                (((n_total - 1) as f64 * fr).round() as usize) * win
            })
            .collect()
    };
    let mut notes: Vec<String> = vec![];
    for &i in &starts {
        let end = (i + win).min(mono.len());
        if end <= i {
            continue;
        }
        let chunk = &mono[i..end];
        if chunk.len() < 3 * sr {
            continue;
        }
        let wav_b64 = encode_wav_b64(chunk, sr as u32)?;
        let parts = vec![
            Part::AudioB64 { data: wav_b64, format: "wav".into() },
            Part::Text(ap.clone()),
        ];
        let s = Sampling::new(0.2, 0.95, 320).top_k(64);
        let ans = strip_think(&llm.chat(&[Message::user_parts(parts)], &s)?);
        notes.push(format!("[{}s+] {}", i / sr, ans));
    }
    Ok(notes.join("\n\n"))
}

/// Собрать WAV (PCM16) из f32-моно в память и base64-кодировать (как sf.write(buf, ...) в питоне).
fn encode_wav_b64(samples: &[f32], sr: u32) -> Result<String, TranslateError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut w = hound::WavWriter::new(cursor, spec).map_err(|e| TranslateError::Audio(e.to_string()))?;
        for &x in samples {
            let v = (x.clamp(-1.0, 1.0) * 32767.0) as i16;
            w.write_sample(v).map_err(|e| TranslateError::Audio(e.to_string()))?;
        }
        w.finalize().map_err(|e| TranslateError::Audio(e.to_string()))?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: usize, ch: usize) -> Vec<String> {
        (0..n).map(|_| "x".repeat(ch)).collect()
    }

    #[test]
    fn short_script_is_single_chunk() {
        // <= SHORT_LINES -> ровно один чанк (паритет со старым единым вызовом), даже если суммарно
        // символов больше MAX_CHARS.
        assert_eq!(chunk_bounds(&v(1, 5)), vec![(0, 1)]);
        assert_eq!(chunk_bounds(&v(SHORT_LINES, 100)), vec![(0, SHORT_LINES)]);
        assert!(chunk_bounds(&[]).is_empty());
    }

    #[test]
    fn long_script_splits_by_line_budget() {
        // 31 короткая строка -> чанки по MAX_LINES строк.
        let b = chunk_bounds(&v(31, 3));
        assert_eq!(b, vec![(0, 10), (10, 20), (20, 30), (30, 31)]);
        // покрытие полное и без дыр
        assert_eq!(b.first().unwrap().0, 0);
        assert_eq!(b.last().unwrap().1, 31);
        for w in b.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
    }

    #[test]
    fn long_script_splits_by_char_budget() {
        // строки по 200 симв, MAX_CHARS=600 -> 3 строки на чанк (до превышения бюджета).
        let b = chunk_bounds(&v(40, 200));
        assert_eq!(b[0], (0, 3));
        assert_eq!(b[1], (3, 6));
    }

    #[test]
    fn oversized_single_line_still_progresses() {
        // одна строка длиннее MAX_CHARS не должна зациклить — берётся одна и идём дальше.
        let mut lines = v(31, 1);
        lines[0] = "y".repeat(MAX_CHARS + 500);
        let b = chunk_bounds(&lines);
        assert_eq!(b[0], (0, 1));
        assert_eq!(b.last().unwrap().1, lines.len());
    }

    #[test]
    fn replace_word_whole_words_only() {
        assert_eq!(replace_word("Sam went home", "Sam", "Сэм"), "Сэм went home");
        // не трогает src внутри более длинного слова
        assert_eq!(replace_word("Samples of Sam", "Sam", "Сэм"), "Samples of Сэм");
        // несколько вхождений
        assert_eq!(replace_word("Sam and Sam", "Sam", "Сэм"), "Сэм and Сэм");
        // нет вхождений
        assert_eq!(replace_word("nothing", "Sam", "Сэм"), "nothing");
    }

    #[test]
    fn char_budget_and_marker_strip() {
        assert_eq!(char_budget(2.0), Some(28)); // 14 симв/сек × 2с
        assert_eq!(char_budget(0.0), None);
        assert_eq!(strip_budget_marker("(≤45) перевод"), "перевод");
        assert_eq!(strip_budget_marker("(<=12)  x"), "x");
        assert_eq!(strip_budget_marker("без маркера"), "без маркера");
    }

    #[test]
    fn term_lock_applies_glossary() {
        let gloss = vec![("Sam".to_string(), "Сэм".to_string()), ("Bob".to_string(), "Боб".to_string())];
        assert_eq!(term_lock("Sam met Bob today", &gloss), "Сэм met Боб today");
        // src==dst или уже переведено -> без изменений
        let g2 = vec![("Sam".to_string(), "Sam".to_string())];
        assert_eq!(term_lock("Sam here", &g2), "Sam here");
        // пустой глоссарий
        assert_eq!(term_lock("plain", &[]), "plain");
    }
}
