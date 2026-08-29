//! PATCH /projects/{pid} — синхронные правки Project без GPU. Порт первых op из dubengine/api.py +
//! app.py.patch_project. В раунде 2 реализованы: segment (edit текста/voice/hidden/keep), subpos
//! (перетащить полосу субтитров), mode (dub/nodub/transcribe через set_mode). Прочие op -> 400
//! (реализуются в следующих раундах). Ошибки: неизвестный op -> 400; неизвестный seg id -> 404.

use dub_core::{BlurBox, CaptionOverride, SubStyle, Title, Project};
use serde_json::Value;

/// Результат применения op: Ok — Project изменён; Err — (http-код, сообщение).
pub type PatchResult = Result<(), (u16, String)>;

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string())
}

fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

fn b(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool())
}

/// Собрать список id из edit["ids"] (массив строк).
fn ids(edit: &Value) -> Vec<String> {
    edit.get("ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

/// Собрать множество индексов из edit["idxs"], отсортировать по УБЫВАНИЮ (удалять с хвоста).
fn idxs_desc(edit: &Value) -> Vec<usize> {
    let set: std::collections::BTreeSet<i64> = edit
        .get("idxs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default();
    set.iter().rev().filter_map(|&x| usize::try_from(x).ok()).collect::<Vec<_>>()
}

/// Пометить все сегменты dirty (после смены режима/перевода re-gen на render).
fn mark_all_dirty(p: &mut Project) {
    for seg in &mut p.segments {
        seg.dirty = true;
    }
}

/// Найти сегмент по edit["id"]: 400 если id нет, 404 если не найден. Общий шаг op_segment/op_regen/
/// op_hide_segment/op_keep_segment.
fn seg_by_id<'a>(p: &'a mut Project, edit: &Value) -> Result<&'a mut dub_core::Segment, (u16, String)> {
    let sid = s(edit, "id").ok_or((400, "missing segment id".into()))?;
    p.segments
        .iter_mut()
        .find(|x| x.id == sid)
        .ok_or((404, format!("segment {sid:?} not found")))
}

/// Удалить элементы вектора по индексам edit["idxs"] (high->low, вне диапазона пропускаются).
fn del_by_idxs<T>(v: &mut Vec<T>, edit: &Value) {
    for idx in idxs_desc(edit) {
        if idx < v.len() {
            v.remove(idx);
        }
    }
}

/// Удалить ОДИН элемент по edit["idx"]: 400 если ключа нет, 404 если не число/вне диапазона.
fn del_one<T>(v: &mut Vec<T>, edit: &Value, what: &str) -> PatchResult {
    let idx = i(edit, "idx").ok_or((400, format!("missing {what} idx")))?;
    let idx = usize::try_from(idx).map_err(|_| (404, format!("bad {what} idx")))?;
    if idx >= v.len() {
        return Err((404, format!("{what} idx {idx} out of range")));
    }
    v.remove(idx);
    Ok(())
}

/// edit_segment — правка одной строки транскрипта. Порт api.edit_segment.
fn op_segment(p: &mut Project, edit: &Value) -> PatchResult {
    let timing_changed = edit.get("start").is_some() || edit.get("end").is_some();
    let seg = seg_by_id(p, edit)?;
    if let Some(t) = edit.get("tgt_text").and_then(|x| x.as_str()) {
        seg.tgt_text = t.to_string();
    }
    if let Some(t) = edit.get("src_text").and_then(|x| x.as_str()) {
        seg.src_text = t.to_string();
    }
    // правка тайминга: start/end (сек). Клампим end > start; порядок в списке не трогаем (рендер сортирует
    // по времени сам). Полезно, когда ASR-тайминг чуть разъехался с речью.
    if let Some(v) = edit.get("start").and_then(|x| x.as_f64()) {
        seg.start = v.max(0.0);
    }
    if let Some(v) = edit.get("end").and_then(|x| x.as_f64()) {
        seg.end = v.max(seg.start + 0.1);
    }
    if seg.end <= seg.start {
        seg.end = seg.start + 0.1;
    }
    // переброс фразы другому спикеру (в т.ч. НОВОМУ id, если ASR определил меньше спикеров, чем есть):
    // голос спикера задаётся в настройках голосов, здесь лишь меняем принадлежность -> ref_of при рендере
    // берёт реф целевого спикера. "" -> None (снять привязку).
    if let Some(sp) = edit.get("speaker") {
        seg.speaker = sp.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
    }
    // индивидуальный голос фразы (override): имя из voices/ или "clone:ID" / "donor:ID" или None
    if let Some(v) = edit.get("voice") {
        seg.voice = v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
    }
    // hidden / keep_original / lane — хранятся в extra (dub-core Segment их не типизирует, но проносит).
    if let Some(h) = edit.get("hidden").and_then(|x| x.as_bool()) {
        seg.extra.insert("hidden".into(), Value::Bool(h));
    }
    if let Some(k) = edit.get("keep_original").and_then(|x| x.as_bool()) {
        seg.extra.insert("keep_original".into(), Value::Bool(k));
    }
    if let Some(l) = edit.get("lane").and_then(|x| x.as_i64()) {
        seg.extra.insert("lane".into(), Value::from(l.clamp(0, 1)));
    }
    if let Some(extra_map) = edit.get("extra").and_then(|x| x.as_object()) {
        for (k, v) in extra_map {
            seg.extra.insert(k.clone(), v.clone());
        }
    }
    seg.dirty = true;
    // Правка тайминга могла нарушить монотонность списка по времени, а render считает слот озвучки по
    // ИНДЕКСУ списка (nxt = segments[i+1].start) — поэтому пересортируем по start (как op_add_segment).
    // Кэш seg-файлов привязан к id сегмента (render.rs), переупорядочивание безопасно.
    if timing_changed {
        p.segments
            .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    }
    Ok(())
}

/// subpos — перетащить полосу субтитров вертикально; ставит sub_y_locked=true (honor для всех строк).
fn op_subpos(p: &mut Project, edit: &Value) -> PatchResult {
    let sub_y = edit
        .get("sub_y")
        .and_then(|x| x.as_i64())
        .ok_or((400, "bad subpos sub_y".to_string()))?;
    p.captions.sub_y = Some(sub_y);
    p.captions.sub_y_locked = true;
    Ok(())
}

/// mode — верхнеуровневый режим вывода. Порт api.set_mode:
///   subtitles -> nodub + subs.translate; dub -> dub + subs.translate; funny -> dub + subs.translate + rewrite.
/// Помечает все сегменты dirty. ValueError (неизвестное значение) -> 400.
fn op_mode(p: &mut Project, edit: &Value) -> PatchResult {
    let value = edit.get("value").and_then(|x| x.as_str()).unwrap_or_default();
    // Композируемость: пресет режима НЕ трогает ЯВНЫЙ выбор «без субтитров». Иначе клик по чипу режима
    // (op_mode) воскрешал бы субтитры, которые юзер выключил через subs_content=none (баг-репорт code-review:
    // «subs=none всё равно прожигает субтитры» — через редактор). subs.mode остаётся под управлением
    // независимого subs_content-контрола, пресет задаёт лишь его дефолт, когда он НЕ «none».
    let keep_no_subs = p.subs.mode == "none";
    let set_subs = |p: &mut Project, m: &str| {
        if !keep_no_subs {
            p.subs.mode = m.into();
        }
    };
    match value {
        "subtitles" => {
            p.mode = "nodub".into();
            set_subs(p, "transcribe"); // субтитры = язык оригинала, без перевода
            p.audio.rewrite = None;
        }
        "dub" => {
            p.mode = "dub".into();
            set_subs(p, "translate");
            p.audio.rewrite = None;
        }
        "voiceover" => {
            // закадровый: перевод+TTS поверх приглушённого оригинала (громкость — audio.voiceover_gain_db)
            p.mode = "voiceover".into();
            set_subs(p, "translate");
            p.audio.rewrite = None;
        }
        "transcribe" => {
            // транскрипт+диаризация: без дубляжа/перевода, субтитры на языке оригинала
            p.mode = "transcribe".into();
            set_subs(p, "transcribe");
            p.audio.rewrite = None;
        }
        "funny" => {
            p.mode = "dub".into();
            set_subs(p, "translate");
            if p.audio.rewrite.is_none() {
                p.audio.rewrite = Some("make it a funny, playful dub".into());
            }
        }
        other => return Err((400, format!("unknown mode {other:?}"))),
    }
    mark_all_dirty(p);
    Ok(())
}

/// dub — независимо задать аудио-выход: none (оригинал, без дубляжа) | dub | voiceover. Развязано от
/// субтитров и шуточного ремикса (audio.rewrite сохраняется) — можно комбинировать: шуточный дубляж +
/// свои голоса, дубляж без субтитров, перевод субтитров без дубляжа и т.д.
fn op_dub(p: &mut Project, edit: &Value) -> PatchResult {
    let v = edit.get("value").and_then(|x| x.as_str()).unwrap_or_default();
    match v {
        "none" => p.mode = "nodub".into(),
        "dub" => p.mode = "dub".into(),
        "voiceover" => p.mode = "voiceover".into(),
        other => return Err((400, format!("unknown audio output {other:?}"))),
    }
    // dub/voiceover требуют TTS -> пометить сегменты dirty (следующий /render синтезирует озвучку).
    if p.mode == "dub" || p.mode == "voiceover" {
        mark_all_dirty(p);
    }
    Ok(())
}

/// subs_burn — вжигать ли субтитры/титры на видео (композируемость: дубляж без сабов и т.п.).
/// {on: bool}. Меняет только наложение на выходе — НЕ трогает TTS (dirty не ставим), следующий
/// /render пересоберёт видео с учётом флага.
fn op_subs_burn(p: &mut Project, edit: &Value) -> PatchResult {
    p.subs.burn = edit.get("on").and_then(Value::as_bool).unwrap_or(true);
    Ok(())
}

/// subs_content — независимо задать содержимое субтитров: none (нет) | transcribe (язык оригинала) |
/// translate (перевод). Развязывает субтитры от аудио-режима (перевод сабов без дубляжа и наоборот).
fn op_subs_content(p: &mut Project, edit: &Value) -> PatchResult {
    let v = s(edit, "value").unwrap_or_default();
    match v.as_str() {
        "none" | "transcribe" | "translate" => p.subs.mode = v,
        other => return Err((400, format!("unknown subs content {other:?}"))),
    }
    Ok(())
}

/// translate — сменить целевой язык (+режим subs=translate; funny -> rewrite). Порт api.translate.
/// Помечает все сегменты dirty (перевод/дубляж перегенерятся на следующем analyze/render). Смена языка
/// требует ре-перевода, но analyze здесь не запускаем — это GPU-джоба; PATCH лишь фиксирует намерение.
fn op_translate(p: &mut Project, edit: &Value) -> PatchResult {
    // api.translate(project, lang, mode="plain"): tgt_lang=lang; subs=translate; funny -> rewrite.
    if let Some(lang) = s(edit, "lang") {
        p.tgt_lang = lang;
    }
    p.subs.mode = "translate".into();
    if s(edit, "mode").as_deref() == Some("funny") {
        p.audio.rewrite = Some("make it a funny, playful dub".into());
    }
    mark_all_dirty(p);
    Ok(())
}

/// rewrite — задать творческую инструкцию ре-дубляжа. Порт api.rewrite: audio.rewrite=instruction;
/// mode=dub; все dirty. Пустая инструкция -> 400 (нечего переписывать).
fn op_rewrite(p: &mut Project, edit: &Value) -> PatchResult {
    let instr = s(edit, "instruction").unwrap_or_default();
    if instr.trim().is_empty() {
        return Err((400, "rewrite requires non-empty instruction".into()));
    }
    p.audio.rewrite = Some(instr);
    p.mode = "dub".into();
    mark_all_dirty(p);
    Ok(())
}

/// translate_style — задать стилевую инструкцию перевода (#112): доп-указание тона/регистра/лексики
/// («formal», «gen-z slang»). Дополняет перевод (в отличие от rewrite, который ЗАМЕНЯЕТ содержимое).
/// Текст нормализуем: trim, переводы строк -> пробелы, кап ~500 символов (как inline-инструкция промпта,
/// не абзац). Помечает все сегменты dirty тем же механизмом, что смена режима/rewrite: сам ре-перевод —
/// GPU-стадия (analyze/vision), PATCH лишь фиксирует намерение (как op_translate). Пустой style снимает
/// стиль (сохраняем ""). Стиль читает translate::stage из proj.audio.translate_style и вносит в sysmsg.
fn op_translate_style(p: &mut Project, edit: &Value) -> PatchResult {
    let raw = s(edit, "style").unwrap_or_default();
    // схлопнуть любые переводы строк/табы в одиночные пробелы, затем trim; кап 500 символов по границам char.
    let flat: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    p.audio.translate_style = flat.chars().take(500).collect();
    mark_all_dirty(p);
    Ok(())
}

/// recast — сменить режим/голос дубляжа. Порт api.recast: audio.voice.mode/name; все сегменты dirty
/// (следующий /render перегенерит дубляж с новым голосом). Порт app.py op=="recast" (раунд 4).
fn op_recast(p: &mut Project, edit: &Value) -> PatchResult {
    let mode = s(edit, "voice_mode").unwrap_or_else(|| "clone".into());
    p.audio.voice.mode = mode;
    p.audio.voice.name = s(edit, "voice_name");
    mark_all_dirty(p);
    Ok(())
}

/// regen — пометить ОДИН сегмент dirty, а ВСЕ ДРУГИЕ сегменты — NOT dirty (ре-TTS только его на /render).
fn op_regen(p: &mut Project, edit: &Value) -> PatchResult {
    let target_id = s(edit, "id").ok_or((400, "missing segment id".into()))?;
    let mut found = false;
    for s in &mut p.segments {
        if s.id == target_id {
            s.dirty = true;
            s.extra.insert("regenerated".into(), Value::Bool(true));
            found = true;
        } else {
            s.dirty = false;
        }
    }
    if !found {
        return Err((404, format!("segment {target_id:?} not found")));
    }
    Ok(())
}

/// regen_multi — пометить НЕСКОЛЬКО сегментов dirty, а ВСЕ ДРУГИЕ сегменты — NOT dirty.
fn op_regen_multi(p: &mut Project, edit: &Value) -> PatchResult {
    let target_ids = ids(edit);
    if target_ids.is_empty() {
        return Err((400, "missing ids array".into()));
    }
    for s in &mut p.segments {
        let is_target = target_ids.contains(&s.id);
        s.dirty = is_target;
        if is_target {
            s.extra.insert("regenerated".into(), Value::Bool(true));
        }
    }
    Ok(())
}

/// regen_all — пометить ВСЕ сегменты dirty (ре-TTS всего дубляжа). Порт app.py op=="regen_all".
fn op_regen_all(p: &mut Project, _edit: &Value) -> PatchResult {
    mark_all_dirty(p);
    for s in &mut p.segments {
        s.extra.remove("regenerated");
    }
    Ok(())
}

/// clear_regen — снять отметки перегенерации («Принять») со всех фраз.
fn op_clear_regen(p: &mut Project, _edit: &Value) -> PatchResult {
    for s in &mut p.segments {
        s.extra.remove("regenerated");
    }
    Ok(())
}

/// add_segment — вставить ПОЛЬЗОВАТЕЛЬСКУЮ фразу (start/end/speaker[/tgt_text]), помеченную dirty →
/// на /render она синтезируется (Higgs клонирует голос спикера) и попадает в субтитры, как обычный сегмент.
/// Текст можно дописать потом (op "segment"). Список пересортировывается по start. Своей речи в источнике
/// нет — reference-голос берётся по speaker (render.rs ref_of: свой спикер → его клон, иначе первый).
fn op_add_segment(p: &mut Project, edit: &Value) -> PatchResult {
    let start = f(edit, "start").unwrap_or(0.0).max(0.0);
    let end = f(edit, "end").unwrap_or(start + 2.0).max(start + 0.2);
    // speaker: явный из запроса, иначе первый существующий (чтобы клон-голос был знакомым).
    let speaker = s(edit, "speaker").or_else(|| p.segments.first().and_then(|x| x.speaker.clone()));
    let id = s(edit, "id")
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| format!("u{}", p.segments.len() + 1));
    // Строим Segment через JSON — #[serde(flatten)] extra заполняется пустым объектом сам.
    let seg: dub_core::Segment = serde_json::from_value(serde_json::json!({
        "id": id, "start": start, "end": end, "speaker": speaker,
        "src_text": "", "tgt_text": s(edit, "tgt_text").unwrap_or_default(),
        "voice": Value::Null, "dirty": true,
    }))
    .map_err(|e| (400, format!("bad segment: {e}")))?;
    p.segments.push(seg);
    p.segments
        .sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    Ok(())
}

/// gain — монтажный гейн всей дорожки (dB). НЕ помечает dirty: ре-TTS не нужен, применяется на рендере
/// поверх нормализации (сегменты берутся из кэша).
fn op_gain(p: &mut Project, edit: &Value) -> PatchResult {
    if let Some(g) = f(edit, "gain_db") {
        p.audio.gain_db = g.clamp(-24.0, 24.0);
    }
    Ok(())
}

/// Громкость ОРИГИНАЛЬНОЙ дорожки в режиме voiceover (закадровый). 0 = в полную силу, отрицательное =
/// тише перевода. Ре-TTS не нужен — только пересведение (лёгкий ре-рендер), сегменты из кэша.
fn op_voiceover_gain(p: &mut Project, edit: &Value) -> PatchResult {
    if let Some(g) = f(edit, "gain_db") {
        p.audio.voiceover_gain_db = g.clamp(-40.0, 0.0);
    }
    if let Some(m) = s(edit, "mode").or_else(|| s(edit, "duck_mode")) {
        let m = m.to_lowercase();
        p.audio.voiceover_duck = if m == "flat" || m == "static" { "flat".into() } else { "dynamic".into() };
    }
    Ok(())
}

/// Режим дакинга в режиме voiceover (закадровый): "dynamic" (DSP) | "flat" (постоянный гейн).
fn op_voiceover_duck(p: &mut Project, edit: &Value) -> PatchResult {
    if let Some(m) = s(edit, "mode").or_else(|| s(edit, "duck_mode")) {
        let m = m.to_lowercase();
        p.audio.voiceover_duck = if m == "flat" || m == "static" { "flat".into() } else { "dynamic".into() };
    }
    Ok(())
}

/// sub_blur — блюр-подложка ПОД сожжёнными субтитрами {on: bool, sigma?: i64, alpha?: f64}. Опция (не всем нужна): выкл -> текст без
/// размытой подложки. Дефолт вкл, sigma дефолт 60, alpha дефолт 0.55. Только рендер-настройка (dirty не ставим — вжигание на экспорте).
fn op_sub_blur(p: &mut Project, edit: &Value) -> PatchResult {
    if let Some(on) = edit.get("on").and_then(|v| v.as_bool()) {
        p.render.blur = on;
    }
    if let Some(sigma) = edit.get("sigma").and_then(|v| v.as_i64()) {
        p.render.blur_sigma = sigma.clamp(1, 200);
    }
    if let Some(alpha) = edit.get("alpha").and_then(|v| v.as_f64()) {
        p.render.extra.insert("blur_alpha".to_string(), serde_json::json!(alpha.clamp(0.0, 1.0)));
    }
    Ok(())
}

/// keep_original — экспортировать вторую дорожку с оригиналом {keep: bool, container?: "mp4"|"mkv"}.
/// Только ремукс на экспорте (дубляж из кэша) — dirty не ставим. Неверный container -> 400.
fn op_keep_original(p: &mut Project, edit: &Value) -> PatchResult {
    p.audio.keep_original_track = b(edit, "keep").unwrap_or(true);
    if let Some(c) = s(edit, "container") {
        match c.as_str() {
            "mp4" | "mkv" => p.audio.container = c,
            other => return Err((400, format!("unknown container {other:?}"))),
        }
    }
    Ok(())
}

/// Наложить caption-поля стиля на SubStyle (типизированные — в поля, прочие — в extra passthrough).
/// Порт edit_caption._apply: неизвестных ключей нет (Pydantic валидирует), но extra="allow" сохраняет
/// vision-поля (background/scene_*). Здесь принимаем любые ключи стиля; типизированные кладём в поля,
/// остальные — в extra, чтобы map_sub_style (render.rs) их подхватил (в т.ч. plate/plate_color — тумблер
/// подложки).
fn apply_substyle_fields(st: &mut SubStyle, fields: &serde_json::Map<String, Value>) {
    for (k, v) in fields {
        match k.as_str() {
            "color" => {
                if let Some(x) = v.as_str() { st.color = x.to_string(); }
            }
            "outline" => {
                if let Some(x) = v.as_str() { st.outline = x.to_string(); }
            }
            "align" => {
                if let Some(x) = v.as_str() { st.align = x.to_string(); }
            }
            "font" => st.font = v.as_str().map(|x| x.to_string()),
            "scene_color" => st.scene_color = v.as_str().map(|x| x.to_string()),
            "italic" => {
                if let Some(x) = v.as_bool() { st.italic = x; }
            }
            "bold" => {
                if let Some(x) = v.as_bool() { st.bold = x; }
            }
            "uppercase" => {
                if let Some(x) = v.as_bool() { st.uppercase = x; }
            }
            "scene_flat" => {
                if let Some(x) = v.as_bool() { st.scene_flat = x; }
            }
            "n_lines" => st.n_lines = v.as_i64(),
            "size_px" => st.size_px = v.as_i64(),
            "outline_w" => st.outline_w = v.as_i64(),
            "shadow_dir" => st.shadow_dir = v.as_i64(), // null -> None (снять тень), int -> угол
            // Прочее (background, size_frac, solid, plate, plate_color, …) — в extra passthrough.
            _ => {
                st.extra.insert(k.clone(), v.clone());
            }
        }
    }
}

/// caption — правка стиля субтитров. seg_id=None -> ГЛОБАЛЬНЫЙ sub_style; иначе per-segment override.
/// Порт api.edit_caption + app.py op=="caption". Тумблер подложки: {op:caption, plate:false} снимает
/// продуктовую плашку глобально (или per-seg с seg_id).
fn op_caption(p: &mut Project, edit: &Value) -> PatchResult {
    // поля стиля = всё, кроме op/seg_id.
    let mut fields = serde_json::Map::new();
    if let Some(obj) = edit.as_object() {
        for (k, v) in obj {
            if k != "op" && k != "seg_id" {
                fields.insert(k.clone(), v.clone());
            }
        }
    }
    let seg_id = s(edit, "seg_id");
    match seg_id {
        None => {
            let mut st = p.captions.sub_style.take().unwrap_or_default();
            apply_substyle_fields(&mut st, &fields);
            p.captions.sub_style = Some(st);
        }
        Some(sid) => {
            let idx = p.captions.overrides.iter().position(|o| o.seg_id == sid);
            let ov = match idx {
                Some(i) => &mut p.captions.overrides[i],
                None => {
                    p.captions.overrides.push(CaptionOverride {
                        seg_id: sid.clone(),
                        ..Default::default()
                    });
                    p.captions.overrides.last_mut().unwrap()
                }
            };
            // Геометрия/текст override — типизированные поля; стиль — во вложенный SubStyle.
            if let Some(t) = fields.get("text").and_then(|v| v.as_str()) {
                ov.text = Some(t.to_string());
            }
            if let Some(x) = fields.get("x").and_then(|v| v.as_i64()) {
                ov.x = Some(x);
            }
            if let Some(y) = fields.get("y").and_then(|v| v.as_i64()) {
                ov.y = Some(y);
            }
            if let Some(w) = fields.get("w").and_then(|v| v.as_i64()) {
                ov.w = Some(w);
            }
            if let Some(fs) = fields.get("fs").and_then(|v| v.as_i64()) {
                ov.fs = Some(fs);
            }
            // Прочие поля -> вложенный style SubStyle (color/font/plate/…).
            let style_fields: serde_json::Map<String, Value> = fields
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "text" | "x" | "y" | "w" | "fs"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !style_fields.is_empty() {
                let mut st = ov.style.take().unwrap_or_default();
                apply_substyle_fields(&mut st, &style_fields);
                ov.style = Some(st);
            }
        }
    }
    Ok(())
}

/// del_segment — удалить строку целиком (уходит субтитр И дубляж). Первый оставшийся -> dirty. Порт api.
fn op_del_segment(p: &mut Project, edit: &Value) -> PatchResult {
    let sid = s(edit, "id").ok_or((400, "missing segment id".into()))?;
    del_segment(p, &sid)
}

fn del_segment(p: &mut Project, sid: &str) -> PatchResult {
    let n = p.segments.len();
    p.segments.retain(|s| s.id != sid);
    if p.segments.len() == n {
        return Err((404, format!("segment {sid:?} not found")));
    }
    if let Some(first) = p.segments.first_mut() {
        first.dirty = true;
    }
    Ok(())
}

/// hide_segment — тоггл/установка hidden (в extra). Порт app.py op=="hide_segment".
fn op_hide_segment(p: &mut Project, edit: &Value) -> PatchResult {
    let seg = seg_by_id(p, edit)?;
    let cur = seg.extra.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
    let new = b(edit, "hidden").unwrap_or(!cur);
    seg.extra.insert("hidden".into(), Value::Bool(new));
    seg.dirty = true;
    Ok(())
}

/// del_segments — массовое удаление (несуществующие пропускаются). Порт app.py op=="del_segments".
fn op_del_segments(p: &mut Project, edit: &Value) -> PatchResult {
    for sid in ids(edit) {
        let _ = del_segment(p, &sid); // KeyError глотается, как в питоне
    }
    Ok(())
}

/// hide_segments — массовое скрытие (явный флаг). Порт app.py op=="hide_segments".
fn op_hide_segments(p: &mut Project, edit: &Value) -> PatchResult {
    let hid = b(edit, "hidden").unwrap_or(true);
    for sid in ids(edit) {
        if let Some(seg) = p.segments.iter_mut().find(|x| x.id == sid) {
            seg.extra.insert("hidden".into(), Value::Bool(hid));
            seg.dirty = true;
        }
    }
    Ok(())
}

/// keep_segment — тоггл keep_original (в extra). Порт app.py op=="keep_segment".
fn op_keep_segment(p: &mut Project, edit: &Value) -> PatchResult {
    let seg = seg_by_id(p, edit)?;
    let cur = seg.extra.get("keep_original").and_then(|v| v.as_bool()).unwrap_or(false);
    let new = b(edit, "keep").unwrap_or(!cur);
    seg.extra.insert("keep_original".into(), Value::Bool(new));
    seg.dirty = true;
    Ok(())
}

/// keep_segments — массовый keep_original (явный флаг). Порт app.py op=="keep_segments".
fn op_keep_segments(p: &mut Project, edit: &Value) -> PatchResult {
    let kp = b(edit, "keep").unwrap_or(true);
    for sid in ids(edit) {
        if let Some(seg) = p.segments.iter_mut().find(|x| x.id == sid) {
            seg.extra.insert("keep_original".into(), Value::Bool(kp));
            seg.dirty = true;
        }
    }
    Ok(())
}

/// del_titles — массовое удаление титров (high->low). Порт app.py op=="del_titles".
fn op_del_titles(p: &mut Project, edit: &Value) -> PatchResult {
    del_by_idxs(&mut p.captions.titles, edit);
    Ok(())
}

/// del_blurs — массовое удаление blur-боксов (high->low). Порт app.py op=="del_blurs".
fn op_del_blurs(p: &mut Project, edit: &Value) -> PatchResult {
    del_by_idxs(&mut p.captions.blur_boxes, edit);
    Ok(())
}

/// blur — правка геометрии/полей одного blur-бокса. Порт api.edit_blur (IndexError -> 404).
fn op_blur(p: &mut Project, edit: &Value) -> PatchResult {
    let idx = i(edit, "idx").ok_or((400, "missing blur idx".into()))?;
    let idx = usize::try_from(idx).map_err(|_| (404, "bad blur idx".to_string()))?;
    let bx = p
        .captions
        .blur_boxes
        .get_mut(idx)
        .ok_or((404, format!("bad blur idx: {idx} out of range")))?;
    if let Some(x) = i(edit, "x") { bx.x = x; }
    if let Some(y) = i(edit, "y") { bx.y = y; }
    if let Some(w) = i(edit, "w") { bx.w = w; }
    if let Some(h) = i(edit, "h") { bx.h = h; }
    if let Some(t0) = f(edit, "t0") { bx.t0 = t0; }
    if let Some(t1) = f(edit, "t1") { bx.t1 = t1; }
    if let Some(hidden) = b(edit, "hidden") { bx.hidden = hidden; }
    if let Some(v) = edit.get("fill") {
        bx.fill = v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
    }
    Ok(())
}

/// blur_add — новый blur-бокс (по умолчанию весь ролик). Порт api.add_blur. Отсутствие x/y/w/h -> 400.
fn op_blur_add(p: &mut Project, edit: &Value) -> PatchResult {
    let bad = |k: &str| (400, format!("bad blur_add: missing/invalid field {k:?}"));
    let x = i(edit, "x").ok_or_else(|| bad("x"))?;
    let y = i(edit, "y").ok_or_else(|| bad("y"))?;
    let w = i(edit, "w").ok_or_else(|| bad("w"))?;
    let h = i(edit, "h").ok_or_else(|| bad("h"))?;
    let t0 = f(edit, "t0").unwrap_or(0.0);
    let t1 = f(edit, "t1").unwrap_or(p.meta.duration);
    p.captions.blur_boxes.push(BlurBox {
        x, y, w, h, t0, t1, hidden: false, fill: None, extra: Default::default(),
    });
    Ok(())
}

/// blur_del — удалить blur-бокс по индексу. Порт api.del_blur (IndexError -> 404).
fn op_blur_del(p: &mut Project, edit: &Value) -> PatchResult {
    del_one(&mut p.captions.blur_boxes, edit, "blur")
}

/// blur_enable — глобальный тоггл блюра (render.blur) + опционально сила размытия (render.blur_sigma).
fn op_blur_enable(p: &mut Project, edit: &Value) -> PatchResult {
    if let Some(on) = b(edit, "on") {
        p.render.blur = on;
    }
    if let Some(sigma) = i(edit, "sigma") {
        p.render.blur_sigma = sigma.clamp(1, 200);
    }
    Ok(())
}

/// preset — имя TEMPLATE-пресета (None/"match" = как оригинал); только re-burn. Порт app.py op=="preset".
fn op_preset(p: &mut Project, edit: &Value) -> PatchResult {
    // name отсутствует ИЛИ пустое -> None (match original).
    p.captions.preset.name = s(edit, "name").filter(|x| !x.is_empty());
    Ok(())
}

/// title — правка титра (текст/italic/font/color/bbox/тайминг). Порт api.edit_title (IndexError -> 404).
fn op_title(p: &mut Project, edit: &Value) -> PatchResult {
    let idx = i(edit, "idx").ok_or((400, "missing title idx".into()))?;
    let idx = usize::try_from(idx).map_err(|_| (404, "bad title idx".to_string()))?;
    let t = p
        .captions
        .titles
        .get_mut(idx)
        .ok_or((404, format!("bad title idx: {idx} out of range")))?;
    if let Some(x) = edit.get("text").and_then(|v| v.as_str()) { t.text = x.to_string(); }
    if let Some(x) = edit.get("tgt").and_then(|v| v.as_str()) { t.tgt = x.to_string(); }
    if let Some(x) = b(edit, "italic") { t.italic = x; }
    if let Some(x) = b(edit, "bold") { t.bold = x; }
    if let Some(x) = b(edit, "uppercase") { t.uppercase = x; }
    if let Some(x) = b(edit, "solid") { t.solid = x; }
    if let Some(v) = edit.get("font") { t.font = v.as_str().map(|x| x.to_string()); }
    if let Some(v) = edit.get("color") { t.color = v.as_str().map(|x| x.to_string()); }
    if let Some(v) = edit.get("bg") { t.bg = v.as_str().map(|x| x.to_string()); }
    if let Some(v) = edit.get("outline") { t.outline = v.as_str().map(|x| x.to_string()); }
    if let Some(v) = edit.get("shadow_dir") { t.shadow_dir = v.as_i64(); } // null->None
    if let Some(a) = edit.get("align").and_then(|v| v.as_str()) { t.align = a.to_string(); }
    if let Some(st) = f(edit, "start") { t.start = st; }
    if let Some(en) = f(edit, "end") { t.end = en; }
    // nullable как shadow_dir: явный null снимает значение (возврат к авто-межстрочному/авто-фиту/авто-контуру),
    // число выставляет, отсутствие ключа не трогает. i()/as_i64() на null давал None -> сброс молча игнорировался.
    if let Some(v) = edit.get("lh") { t.lh = v.as_i64(); }
    if let Some(v) = edit.get("size_px") { t.size_px = v.as_i64(); }
    if let Some(v) = edit.get("outline_w") { t.outline_w = v.as_i64(); }
    if let Some(bbox) = edit.get("bbox").and_then(|v| v.as_array()) {
        t.bbox = Some(bbox.iter().filter_map(|x| x.as_i64()).collect());
    }
    Ok(())
}

/// title_del — удалить титр по индексу. Порт api.del_title (IndexError -> 404).
fn op_title_del(p: &mut Project, edit: &Value) -> PatchResult {
    del_one(&mut p.captions.titles, edit, "title")
}

/// title_add — новый кастомный титр в боксе на [t0,t1]. Порт api.add_title. Нет x/y/w/h -> 400.
fn op_title_add(p: &mut Project, edit: &Value) -> PatchResult {
    let bad = |k: &str| (400, format!("bad title_add: missing/invalid field {k:?}"));
    let text = s(edit, "text").unwrap_or_default();
    let x = i(edit, "x").ok_or_else(|| bad("x"))?;
    let y = i(edit, "y").ok_or_else(|| bad("y"))?;
    let w = i(edit, "w").ok_or_else(|| bad("w"))?;
    let h = i(edit, "h").ok_or_else(|| bad("h"))?;
    let t0 = f(edit, "t0").unwrap_or(0.0);
    let t1 = f(edit, "t1").unwrap_or(p.meta.duration);
    p.captions.titles.push(Title {
        text: text.clone(),
        tgt: text,
        bbox: Some(vec![x, y, w, h]),
        start: t0,
        end: t1,
        italic: b(edit, "italic").unwrap_or(false),
        font: s(edit, "font"),
        color: Some(s(edit, "color").unwrap_or_else(|| "#FFFFFF".into())),
        ..Default::default()
    });
    Ok(())
}

/// Применить одну PATCH-операцию к Project. op берётся из поля "op". Неизвестный op -> 400.
pub fn apply(p: &mut Project, edit: &Value) -> PatchResult {
    let op = s(edit, "op").unwrap_or_default();
    match op.as_str() {
        "caption" => op_caption(p, edit),
        "segment" => op_segment(p, edit),
        "del_segment" => op_del_segment(p, edit),
        "add_segment" => op_add_segment(p, edit),
        "hide_segment" => op_hide_segment(p, edit),
        "del_segments" => op_del_segments(p, edit),
        "hide_segments" => op_hide_segments(p, edit),
        "del_titles" => op_del_titles(p, edit),
        "del_blurs" => op_del_blurs(p, edit),
        "keep_segment" => op_keep_segment(p, edit),
        "keep_segments" => op_keep_segments(p, edit),
        "blur" => op_blur(p, edit),
        "blur_add" => op_blur_add(p, edit),
        "blur_del" => op_blur_del(p, edit),
        "blur_enable" => op_blur_enable(p, edit),
        "preset" => op_preset(p, edit),
        "title" => op_title(p, edit),
        "title_del" => op_title_del(p, edit),
        "title_add" => op_title_add(p, edit),
        "subpos" => op_subpos(p, edit),
        "mode" => op_mode(p, edit),
        "dub" => op_dub(p, edit),
        "subs_burn" => op_subs_burn(p, edit),
        "subs_content" => op_subs_content(p, edit),
        "translate" => op_translate(p, edit),
        "translate_style" => op_translate_style(p, edit),
        "rewrite" => op_rewrite(p, edit),
        "recast" => op_recast(p, edit),
        "regen" => op_regen(p, edit),
        "regen_multi" => op_regen_multi(p, edit),
        "regen_all" => op_regen_all(p, edit),
        "clear_regen" => op_clear_regen(p, edit),
        "gain" => op_gain(p, edit),
        "voiceover_gain" => op_voiceover_gain(p, edit),
        "voiceover_duck" => op_voiceover_duck(p, edit),
        "sub_blur" => op_sub_blur(p, edit),
        "keep_original" => op_keep_original(p, edit),
        "reorder_segments" => op_reorder_segments(p, edit),
        other => Err((400, format!("unknown op {other:?}"))),
    }
}

/// reorder_segments — изменить порядок сегментов согласно списку id в edit["ids"].
fn op_reorder_segments(p: &mut Project, edit: &Value) -> PatchResult {
    let new_ids = ids(edit);
    if new_ids.is_empty() {
        return Err((400, "reorder_segments requires non-empty ids array".into()));
    }
    let mut map: std::collections::HashMap<String, dub_core::Segment> =
        p.segments.drain(..).map(|s| (s.id.clone(), s)).collect();
    for id in &new_ids {
        if let Some(s) = map.remove(id) {
            p.segments.push(s);
        }
    }
    for (_, s) in map {
        p.segments.push(s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn proj_with_seg() -> Project {
        let mut p = Project::default();
        p.segments.push(dub_core::Segment {
            id: "s0".into(),
            start: 0.0,
            end: 1.0,
            src_text: "hi".into(),
            ..Default::default()
        });
        p
    }

    #[test]
    fn segment_edits_text_and_marks_dirty() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"segment","id":"s0","tgt_text":"привет"})).unwrap();
        assert_eq!(p.segments[0].tgt_text, "привет");
        assert!(p.segments[0].dirty);
    }

    #[test]
    fn segment_unknown_id_404() {
        let mut p = proj_with_seg();
        let e = apply(&mut p, &json!({"op":"segment","id":"sX","tgt_text":"x"})).unwrap_err();
        assert_eq!(e.0, 404);
    }

    #[test]
    fn subpos_sets_locked() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"subpos","sub_y":720})).unwrap();
        assert_eq!(p.captions.sub_y, Some(720));
        assert!(p.captions.sub_y_locked);
    }

    #[test]
    fn mode_dub_and_unknown() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"mode","value":"dub"})).unwrap();
        assert_eq!(p.mode, "dub");
        assert!(p.segments[0].dirty);
        let e = apply(&mut p, &json!({"op":"mode","value":"nope"})).unwrap_err();
        assert_eq!(e.0, 400);
    }

    #[test]
    fn translate_sets_lang_and_dirty() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"translate","lang":"de"})).unwrap();
        assert_eq!(p.tgt_lang, "de");
        assert_eq!(p.subs.mode, "translate");
        assert!(p.audio.rewrite.is_none());
        assert!(p.segments[0].dirty);
    }

    #[test]
    fn translate_funny_sets_rewrite() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"translate","lang":"en","mode":"funny"})).unwrap();
        assert_eq!(p.audio.rewrite.as_deref(), Some("make it a funny, playful dub"));
    }

    #[test]
    fn translate_style_normalizes_and_marks_dirty() {
        let mut p = proj_with_seg();
        // переводы строк -> пробелы, обрезка краёв.
        apply(&mut p, &json!({"op":"translate_style","style":"  formal,\n very polite  "})).unwrap();
        assert_eq!(p.audio.translate_style, "formal, very polite");
        assert!(p.segments[0].dirty);
        // кап длины ~500 символов.
        let long = "a".repeat(700);
        apply(&mut p, &json!({"op":"translate_style","style":long})).unwrap();
        assert_eq!(p.audio.translate_style.chars().count(), 500);
        // пустой style снимает стиль.
        apply(&mut p, &json!({"op":"translate_style","style":"  "})).unwrap();
        assert_eq!(p.audio.translate_style, "");
    }

    #[test]
    fn rewrite_sets_instruction_and_dub() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"rewrite","instruction":"as a pirate"})).unwrap();
        assert_eq!(p.audio.rewrite.as_deref(), Some("as a pirate"));
        assert_eq!(p.mode, "dub");
        assert!(p.segments[0].dirty);
        let e = apply(&mut p, &json!({"op":"rewrite","instruction":"  "})).unwrap_err();
        assert_eq!(e.0, 400);
    }

    // ── PATCH-хвост (раунд 5) ────────────────────────────────────────────────

    #[test]
    fn caption_global_sets_substyle_and_plate_toggle() {
        let mut p = proj_with_seg();
        // тумблер подложки: plate=false -> в extra sub_style (map_sub_style читает).
        apply(&mut p, &json!({"op":"caption","color":"#FF0000","plate":false})).unwrap();
        let ss = p.captions.sub_style.as_ref().unwrap();
        assert_eq!(ss.color, "#FF0000");
        assert_eq!(ss.extra.get("plate").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn caption_per_segment_override() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"caption","seg_id":"s0","text":"свой текст","color":"#00FF00"})).unwrap();
        assert_eq!(p.captions.overrides.len(), 1);
        let o = &p.captions.overrides[0];
        assert_eq!(o.seg_id, "s0");
        assert_eq!(o.text.as_deref(), Some("свой текст"));
        assert_eq!(o.style.as_ref().unwrap().color, "#00FF00");
        // повторный caption на тот же seg_id ОБНОВЛЯЕТ, не добавляет.
        apply(&mut p, &json!({"op":"caption","seg_id":"s0","text":"новый"})).unwrap();
        assert_eq!(p.captions.overrides.len(), 1);
        assert_eq!(p.captions.overrides[0].text.as_deref(), Some("новый"));
    }

    #[test]
    fn del_segment_marks_first_dirty_and_404() {
        let mut p = proj_with_seg();
        p.segments.push(dub_core::Segment { id: "s1".into(), ..Default::default() });
        apply(&mut p, &json!({"op":"del_segment","id":"s1"})).unwrap();
        assert_eq!(p.segments.len(), 1);
        assert!(p.segments[0].dirty);
        let e = apply(&mut p, &json!({"op":"del_segment","id":"nope"})).unwrap_err();
        assert_eq!(e.0, 404);
    }

    #[test]
    fn blur_add_edit_del_cycle() {
        let mut p = proj_with_seg();
        p.meta.duration = 12.0;
        apply(&mut p, &json!({"op":"blur_add","x":10,"y":20,"w":100,"h":40})).unwrap();
        assert_eq!(p.captions.blur_boxes.len(), 1);
        assert_eq!(p.captions.blur_boxes[0].t1, 12.0); // дефолт весь ролик
        apply(&mut p, &json!({"op":"blur","idx":0,"x":15,"hidden":true})).unwrap();
        assert_eq!(p.captions.blur_boxes[0].x, 15);
        assert!(p.captions.blur_boxes[0].hidden);
        // out-of-range -> 404
        let e = apply(&mut p, &json!({"op":"blur","idx":9,"x":1})).unwrap_err();
        assert_eq!(e.0, 404);
        apply(&mut p, &json!({"op":"blur_del","idx":0})).unwrap();
        assert!(p.captions.blur_boxes.is_empty());
        // отсутствие обязательного поля -> 400
        let e = apply(&mut p, &json!({"op":"blur_add","x":1,"y":2})).unwrap_err();
        assert_eq!(e.0, 400);
    }

    #[test]
    fn title_add_edit_del_cycle() {
        let mut p = proj_with_seg();
        p.meta.duration = 8.0;
        apply(&mut p, &json!({"op":"title_add","text":"HELLO","x":50,"y":60,"w":300,"h":80})).unwrap();
        assert_eq!(p.captions.titles.len(), 1);
        let t = &p.captions.titles[0];
        assert_eq!(t.text, "HELLO");
        assert_eq!(t.tgt, "HELLO");
        assert_eq!(t.bbox.as_deref(), Some(&[50i64, 60, 300, 80][..]));
        assert_eq!(t.end, 8.0);
        apply(&mut p, &json!({"op":"title","idx":0,"tgt":"ПРИВЕТ","color":"#FF0000"})).unwrap();
        assert_eq!(p.captions.titles[0].tgt, "ПРИВЕТ");
        assert_eq!(p.captions.titles[0].color.as_deref(), Some("#FF0000"));
        apply(&mut p, &json!({"op":"title_del","idx":0})).unwrap();
        assert!(p.captions.titles.is_empty());
        let e = apply(&mut p, &json!({"op":"title_del","idx":0})).unwrap_err();
        assert_eq!(e.0, 404);
    }

    #[test]
    fn title_size_px_null_resets_to_auto() {
        // регресс: очистка поля px/контура/межстрочья у титра (явный JSON null) должна вернуть авто-подбор,
        // а не молча игнорироваться (i()/as_i64() на null давал None -> сброс не применялся).
        let mut p = Project::default();
        apply(&mut p, &json!({"op":"title_add","text":"T","x":0,"y":0,"w":100,"h":40})).unwrap();
        apply(&mut p, &json!({"op":"title","idx":0,"size_px":80,"outline_w":6,"lh":50})).unwrap();
        assert_eq!(p.captions.titles[0].size_px, Some(80));
        assert_eq!(p.captions.titles[0].outline_w, Some(6));
        assert_eq!(p.captions.titles[0].lh, Some(50));
        apply(&mut p, &json!({"op":"title","idx":0,"size_px":null,"outline_w":null,"lh":null})).unwrap();
        assert_eq!(p.captions.titles[0].size_px, None);   // null -> авто-фит
        assert_eq!(p.captions.titles[0].outline_w, None);
        assert_eq!(p.captions.titles[0].lh, None);
        apply(&mut p, &json!({"op":"title","idx":0,"size_px":42})).unwrap();
        apply(&mut p, &json!({"op":"title","idx":0,"color":"#fff"})).unwrap();   // отсутствие ключа не трогает size_px
        assert_eq!(p.captions.titles[0].size_px, Some(42));
    }

    #[test]
    fn preset_and_blur_enable() {
        let mut p = proj_with_seg();
        apply(&mut p, &json!({"op":"preset","name":"hormozi"})).unwrap();
        assert_eq!(p.captions.preset.name.as_deref(), Some("hormozi"));
        apply(&mut p, &json!({"op":"preset","name":"match"})).unwrap();
        // "match"/пусто хранится как есть в питоне (None only когда name отсутствует/пусто); тут name="match".
        apply(&mut p, &json!({"op":"preset"})).unwrap();
        assert!(p.captions.preset.name.is_none());
        apply(&mut p, &json!({"op":"blur_enable","on":false,"sigma":45})).unwrap();
        assert!(!p.render.blur);
        assert_eq!(p.render.blur_sigma, 45);
        apply(&mut p, &json!({"op":"sub_blur","on":true,"sigma":80})).unwrap();
        assert!(p.render.blur);
        assert_eq!(p.render.blur_sigma, 80);
    }

    #[test]
    fn keep_original_toggles_and_validates_container() {
        let mut p = proj_with_seg();
        // дефолты: выключено, mp4.
        assert!(!p.audio.keep_original_track);
        assert_eq!(p.audio.container, "mp4");
        // keep + mkv.
        apply(&mut p, &json!({"op":"keep_original","keep":true,"container":"mkv"})).unwrap();
        assert!(p.audio.keep_original_track);
        assert_eq!(p.audio.container, "mkv");
        // ре-TTS не требуется — dirty НЕ ставится.
        assert!(!p.segments[0].dirty);
        // keep без container — контейнер не трогается.
        apply(&mut p, &json!({"op":"keep_original","keep":false})).unwrap();
        assert!(!p.audio.keep_original_track);
        assert_eq!(p.audio.container, "mkv");
        // невалидный container -> 400.
        let e = apply(&mut p, &json!({"op":"keep_original","keep":true,"container":"avi"})).unwrap_err();
        assert_eq!(e.0, 400);
    }

    #[test]
    fn del_titles_and_del_blurs_high_to_low() {
        let mut p = proj_with_seg();
        for _ in 0..3 {
            p.captions.titles.push(dub_core::Title { text: "t".into(), ..Default::default() });
            p.captions.blur_boxes.push(dub_core::BlurBox::default());
        }
        apply(&mut p, &json!({"op":"del_titles","idxs":[0,2]})).unwrap();
        assert_eq!(p.captions.titles.len(), 1);
        apply(&mut p, &json!({"op":"del_blurs","idxs":[1]})).unwrap();
        assert_eq!(p.captions.blur_boxes.len(), 2);
    }

    #[test]
    fn stable_id_preservation_and_donor_stability_on_delete() {
        let mut p = Project::default();
        p.segments.push(dub_core::Segment { id: "s0".into(), start: 0.0, end: 1.0, src_text: "seg0".into(), ..Default::default() });
        p.segments.push(dub_core::Segment { id: "s1".into(), start: 1.0, end: 2.0, src_text: "seg1".into(), ..Default::default() });
        p.segments.push(dub_core::Segment { id: "s2".into(), start: 2.0, end: 3.0, src_text: "seg2".into(), voice: Some("donor:s1".into()), ..Default::default() });

        // Удаляем s0 (первый сегмент)
        apply(&mut p, &json!({"op":"del_segment","id":"s0"})).unwrap();

        // Проверяем: осталось 2 сегмента, но их ID s1 и s2 не изменились
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].id, "s1");
        assert_eq!(p.segments[1].id, "s2");

        // Проверяем: донор в s2 всё ещё указывает на стабильный id s1
        assert_eq!(p.segments[1].voice.as_deref(), Some("donor:s1"));
        assert_eq!(p.segments[0].id, "s1"); // Целевой сегмент донора на месте
    }

    #[test]
    fn regen_sets_regenerated_flag_and_clear_regen_resets_it() {
        let mut p = Project::default();
        p.segments.push(dub_core::Segment { id: "s0".into(), ..Default::default() });
        p.segments.push(dub_core::Segment { id: "s1".into(), ..Default::default() });

        // op regen на s0
        apply(&mut p, &json!({"op":"regen","id":"s0"})).unwrap();
        assert_eq!(p.segments[0].extra.get("regenerated").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(p.segments[1].extra.get("regenerated"), None);

        // op clear_regen
        apply(&mut p, &json!({"op":"clear_regen"})).unwrap();
        assert_eq!(p.segments[0].extra.get("regenerated"), None);

        // op regen_multi на s0 и s1
        apply(&mut p, &json!({"op":"regen_multi","ids":["s0","s1"]})).unwrap();
        assert_eq!(p.segments[0].extra.get("regenerated").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(p.segments[1].extra.get("regenerated").and_then(|v| v.as_bool()), Some(true));

        // op regen_all сбрасывает флаг со всех
        apply(&mut p, &json!({"op":"regen_all"})).unwrap();
        assert_eq!(p.segments[0].extra.get("regenerated"), None);
        assert_eq!(p.segments[1].extra.get("regenerated"), None);
    }
}


