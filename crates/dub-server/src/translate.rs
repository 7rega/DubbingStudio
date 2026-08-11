//! Стадия перевода + vision для analyze (раунд 3). Порт pipeline._build_dub translate-ветки поверх
//! крейта dub-translate (Gemma через сайдкар llama-server). Заполняет tgt_text сегментов, titles/brands/
//! sub_style/sub_y и raw_ctx в Project. Все решения (do_translate / same_lang / rewrite) — как в питоне.
//!
//! Fail-safe: любой сбой (нет llama-бинаря / нет весов / упал сервер) логируется в SSE и оставляет tgt
//! пустым — перевод не блокирует транскрипт-стадию analyze (её результат уже валиден).

use dub_core::{Brand, Project, SubStyle};
use dub_llm::{ChatClient, LlamaServer, ServerOpts};
use dub_translate::{classify_content_type, ctx_run, CtxConfig, Seg};
use serde_json::Value;

use crate::analyze::{AnalyzeArgs, AnalyzePaths, Progress};

fn emit(progress: &Progress, stage: &str, msg: &str) {
    progress(serde_json::json!({ "stage": stage, "msg": msg }));
}

/// tgt = исходный текст (без MT) — для transcribe- и same-lang-веток.
fn copy_src_to_tgt(proj: &mut Project) {
    for s in &mut proj.segments {
        s.tgt_text = s.src_text.clone();
    }
}

/// Нужно ли переводить: dub/voiceover-режим ИЛИ subs=translate (как do_translate в pipeline).
fn wants_translate(proj: &Project) -> bool {
    proj.mode == "dub" || proj.mode == "voiceover" || proj.subs.mode == "translate"
}

/// Автономная классификация типа контента (real/anime) для кастинга. Нужна, когда translate-стадия
/// пропущена ранним return (same-lang / transcribe / нет LLM), а content_type="auto": иначе casting
/// молча берёт "real"-детектор для анимации. Поднимает Gemma+mmproj ТОЛЬКО ради классификации и гасит.
/// None -> классифицировать не удалось (нет бинаря/весов/сервер не встал) -> вызывающий оставит дефолт.
pub fn classify_content_type_standalone(
    paths: &AnalyzePaths,
    total: f64,
    progress: &Progress,
) -> Option<String> {
    // Без mmproj (vision-проектор) классификация невозможна: слать кадры в text-only модель = молча "real"
    // с ложным «0 голосов». Нет проектора -> None, вызывающий честно оставит дефолт.
    if !paths.llama_bin.is_file() || !paths.mt_model.is_file() || !paths.mmproj.is_file() {
        return None;
    }
    let opts = ServerOpts::new(&paths.llama_bin, &paths.mt_model)
        .with_ubatch(crate::models::sel_num(&paths.models_root, "llama_ubatch").map(|f| f as u32))
        .with_mmproj(&paths.mmproj);
    let srv = LlamaServer::start(&opts).ok()?;
    let client = ChatClient::new(srv.base_url()).ok()?;
    let tmp = paths.work_dir.join("ctype_frame.png");
    let ct = classify_content_type(&client, &paths.input, &tmp, total, |m| emit(progress, "vision", m));
    let _ = std::fs::remove_file(&tmp);
    Some(ct)
}

/// Прогнать стадию. proj уже собран транскрипт-стадией (segments + mode/tgt_lang). vh/total — из probe.
pub fn stage(
    args: &AnalyzeArgs,
    paths: &AnalyzePaths,
    proj: &mut Project,
    vocals16: &std::path::Path,
    vh: i64,
    total: f64,
    progress: &Progress,
) {
    // Нет сегментов -> нечего переводить (auto-nodub / музыка). Как ранний return в питоне.
    if proj.segments.is_empty() {
        return;
    }
    // Импортированы субтитры УЖЕ на языке перевода: tgt заполнен из cues (analyze import-ветка),
    // MT и vision-раскладка не нужны — Даб Студио только озвучивает готовый текст.
    if args.import_translated {
        emit(progress, "translate", "субтитры уже на языке перевода -> без MT (только озвучка)");
        return;
    }
    let rewrite = if args.rewrite.is_empty() { None } else { Some(args.rewrite.as_str()) };
    let do_translate = wants_translate(proj) || rewrite.is_some();
    if !do_translate {
        // transcribe-режим: tgt = исходный текст, БЕЗ MT (parity с pipeline «transcribe» веткой).
        copy_src_to_tgt(proj);
        emit(progress, "translate", "transcribe: tgt=исходный текст, без перевода");
        return;
    }

    // src == tgt -> оставить исходник, ноль MT (same_lang в питоне). src берём из query (auto -> не знаем
    // язык детерминированно тут; ASR его не вернул типизированно, потому same_lang проверяем лишь по
    // явному src_lang — как str(src).lower()==tgt в питоне при известном src).
    let src = &args.src_lang;
    let src_lc = src.to_lowercase();
    let same_lang = !src.is_empty() && src_lc != "auto" && src_lc == proj.tgt_lang.to_lowercase();
    if same_lang && rewrite.is_none() {
        copy_src_to_tgt(proj);
        emit(progress, "translate", "same-lang -> без MT (tgt=исходник)");
        return;
    }

    // Поднять сайдкар Gemma (+mmproj для vision). Существование бинаря/весов проверяет start().
    if !paths.llama_bin.is_file() {
        emit(progress, "translate", &format!(
            "перевод пропущен: llama-server не найден ({})", paths.llama_bin.display()));
        return;
    }
    if !paths.mt_model.is_file() {
        emit(progress, "translate", &format!(
            "перевод пропущен: GGUF Gemma не найден ({})", paths.mt_model.display()));
        return;
    }

    // LLM-провайдер: облако OpenRouter (если включено в настройках + есть ключ) ИЛИ локальный llama-server
    // (Gemma+mmproj). Vision-режим: облаку — multimodal-модель, локали — mmproj. Fail-safe как раньше.
    let prov = match crate::llm_provider::open(
        &crate::llm_provider::LlmOpen {
            llama_bin: &paths.llama_bin,
            mt_model: &paths.mt_model,
            mmproj: &paths.mmproj,
            models_root: &paths.models_root,
        },
        crate::llm_provider::LlmMode::Vision,
    ) {
        Ok(p) => {
            emit(progress, "translate", if p.is_remote() {
                "перевод/vision через облако (OpenRouter)"
            } else {
                "поднимаю llama-server (Gemma + mmproj)"
            });
            p
        }
        Err(e) => {
            emit(progress, "translate", &format!("LLM недоступен: {e}; перевод пропущен"));
            return;
        }
    };
    let client = prov.client();

    // Авто-детект типа контента для кастинга (#115): юзер выбрал «Авто» + кастинг включён -> классифицируем
    // live-action vs анимация Gemma-vision (сервер уже поднят). Только при наличии mmproj (иначе vision нет
    // -> casting-стадия сделает автономный детект/дефолт). Результат в проект; casting-стадия прочитает.
    if args.casting && args.content_type == "auto" && paths.mmproj.is_file() {
        let tmp = paths.work_dir.join("ctype_frame.png");
        let ct = classify_content_type(&client, &paths.input, &tmp, total, |m| {
            emit(progress, "vision", m);
        });
        let _ = std::fs::remove_file(&tmp);
        proj.audio.content_type = ct;
    }

    // Seg-вью для dub-translate (text/speaker). speaker -> i64 (питон speaker=0 по умолчанию).
    let mut segs: Vec<Seg> = proj
        .segments
        .iter()
        .map(|s| {
            let spk = crate::analyze::speaker_to_i64(s.speaker.as_deref());
            let mut seg = Seg::new(s.src_text.clone(), spk);
            seg.start = s.start;
            seg.end = s.end;
            seg
        })
        .collect();

    // VISION-layout нужен только когда его выход (sub_style/titles/brands) реально попадёт на экран:
    // вжигание включено И субтитры не «none». Иначе (например «Дубляж без субтитров») это 10-20
    // vision-вызовов Gemma впустую — на длинном видео минуты (баг-репорт юзера).
    let want_layout = proj.subs.burn && proj.subs.mode != "none";
    let cfg = CtxConfig {
        input: paths.input.clone(),
        work_dir: paths.work_dir.clone(),
        tgt_lang: proj.tgt_lang.clone(),
        vocals16: if vocals16.is_file() { Some(vocals16.to_path_buf()) } else { None },
        vh: vh as f64,
        total,
        want_layout,
        // Стиль перевода (#112): из проекта. Тем же путём, что rewrite попадает в ctx_run.
        style: proj.audio.translate_style.clone(),
    };

    emit(progress, "vision", "ctx-проход: vision layout/scene + перевод транскрипта");
    let res = ctx_run(&client, &cfg, &mut segs, rewrite, |m| {
        emit(progress, "vision", m);
    });

    // Сервер больше не нужен -> глушим (освобождаем VRAM, как del llm в питоне перед TTS/берном).
    // ГЕЙТ ПОКРЫТИЯ ПЕРЕВОДА (валидация В пайплайне): сегменты, оставшиеся английскими/непереведёнными
    // (tgt≈src ИЛИ латиница при нелатинском tgt), доперевести точечно flat_run — пока LLM ещё жив.
    if res.is_ok() {
        ensure_translation_coverage(client, &mut segs, &args.src_lang, &proj.tgt_lang, progress);
    }
    drop(prov); // глушим локальный llama-server (освобождаем VRAM перед TTS/берном); облако — no-op

    let extra = match res {
        Ok(r) => r.extra,
        Err(e) => {
            emit(progress, "translate", &format!("ctx-перевод не удался: {e}; tgt оставлен пустым"));
            return;
        }
    };

    // Перенести tgt в сегменты Project. segs строился 1:1 из proj.segments и дальше не используется —
    // переносим строки перемещением (zip по равной длине, без клонов).
    for (s, sg) in proj.segments.iter_mut().zip(segs) {
        s.tgt_text = sg.tgt;
    }

    // Замапить extra -> типизированные поля Project + сохранить сырой ctx (byte-identical passthrough,
    // как raw_ctx = ce_d в from_artifacts).
    apply_extra(proj, &extra);

    let translated = proj.segments.iter().filter(|s| !s.tgt_text.is_empty()).count();
    emit(progress, "translate", &format!(
        "перевод готов: {}/{} строк, тайтлов={}",
        translated, proj.segments.len(), proj.captions.titles.len()));
}

/// extra (ctx_extra.json) -> типизированные captions.sub_style/sub_y/titles/brands + raw_ctx.
/// Точная параллель project.from_artifacts (строки 239-249): raw_ctx = сырой extra; sub_style/titles/
/// brands десериализуются в типы (extra="allow" ловит все ключи vision-словаря).
fn apply_extra(proj: &mut Project, extra: &Value) {
    // raw_ctx — весь ctx как есть (для будущего byte-identical re-render captions-стадии раунда 4).
    if let Value::Object(m) = extra {
        proj.raw_ctx = m.clone();
    }
    // sub_style
    if let Some(ss) = extra.get("sub_style") {
        if ss.is_object() {
            if let Ok(style) = serde_json::from_value::<SubStyle>(ss.clone()) {
                proj.captions.sub_style = Some(style);
            }
        }
    }
    // sub_y
    if let Some(y) = extra.get("sub_y").and_then(|v| v.as_i64()) {
        proj.captions.sub_y = Some(y);
    }
    // titles: НЕ строим здесь. Их финальный вид (bbox + время + стиль) собирает caption-композит
    // (compose.rs) ПОСЛЕ OCR-стадии — до OCR у нас нет localize-боксов для матчинга y_frac -> bbox, а
    // без bbox emit_title молча скипает титр. raw_ctx["titles"] (уже с tgt) переносится выше как есть,
    // композит читает его оттуда. Порт pipeline.run:497-543 живёт в compose::run.
    // brands
    if let Some(arr) = extra.get("brands").and_then(|v| v.as_array()) {
        proj.captions.brands = arr
            .iter()
            .filter(|b| b.is_object())
            .filter_map(|b| serde_json::from_value::<Brand>(b.clone()).ok())
            .collect();
    }
}


/// Целевой язык пишется НЕлатиницей (кириллица/CJK/RTL/индийские/…)? — для детекции «английский пролез».
fn tgt_expects_non_latin(lang: &str) -> bool {
    let l = lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase();
    matches!(
        l.as_str(),
        "ru" | "uk" | "be" | "bg" | "sr" | "mk" | "kk" | "ky" | "tg" | "mn" | "ab" | "os"
            | "zh" | "ja" | "ko"
            | "ar" | "fa" | "ur" | "he" | "ps" | "sd"
            | "el" | "hy" | "ka" | "hi" | "bn" | "pa" | "gu" | "ta" | "te" | "kn" | "ml"
            | "th" | "lo" | "km" | "my" | "si" | "am"
    )
}

/// Сегмент выглядит НЕ переведённым: пусто, равен исходнику, ИЛИ tgt преимущественно латиница при
/// нелатинском целевом языке (английский «пролез сквозь» перевод).
fn looks_untranslated(src: &str, tgt: &str, tgt_lang: &str) -> bool {
    let t = tgt.trim();
    if t.is_empty() {
        return true;
    }
    if t.eq_ignore_ascii_case(src.trim()) {
        return true;
    }
    if tgt_expects_non_latin(tgt_lang) {
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        if letters > 0 {
            let latin = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
            if (latin as f64) / (letters as f64) > 0.5 {
                return true;
            }
        }
    }
    false
}

/// Гейт покрытия перевода: доперевести сегменты, оставшиеся непереведёнными (english leak), точечным
/// flat_run по их исходным текстам. До 2 проходов; меняем только реально улучшившиеся tgt; логируем остаток.
fn ensure_translation_coverage(
    client: &ChatClient,
    segs: &mut [Seg],
    src: &str,
    tgt_lang: &str,
    progress: &Progress,
) {
    let bad: Vec<usize> = segs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.text.trim().is_empty() && looks_untranslated(&s.text, &s.tgt, tgt_lang))
        .map(|(i, _)| i)
        .collect();
    if bad.is_empty() {
        return;
    }
    emit(progress, "translate", &format!("покрытие перевода: {} строк без перевода — доперевожу", bad.len()));
    for _ in 0..2 {
        let mut sub: Vec<Seg> = bad
            .iter()
            .map(|&i| {
                let mut g = Seg::new(segs[i].text.clone(), segs[i].speaker);
                g.start = segs[i].start;
                g.end = segs[i].end;
                g
            })
            .collect();
        if dub_translate::flat_run(client, &mut sub, src, tgt_lang, true, "").is_err() {
            break;
        }
        for (k, &i) in bad.iter().enumerate() {
            if !looks_untranslated(&segs[i].text, &sub[k].tgt, tgt_lang) {
                segs[i].tgt = std::mem::take(&mut sub[k].tgt);
            }
        }
        let still = bad
            .iter()
            .filter(|&&i| looks_untranslated(&segs[i].text, &segs[i].tgt, tgt_lang))
            .count();
        emit(progress, "translate", &format!("покрытие перевода: осталось {still} без перевода"));
        if still == 0 {
            break;
        }
    }
}
