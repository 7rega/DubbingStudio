//! Silence-proven automatic splits and listener-approved manual merges.
//! Speaker labels are not voice identity gates. Applying a proposal NEVER aligns
//! or splits again: the proposal revision and the entire selected chain are checked.
use dub_asr::{forced::TimedWord, speech_edges::Envelope};
use dub_core::{Project, Segment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, path::Path};

pub const SPLIT_GAP: f64 = 0.8;
pub const SPLIT_ABS: f64 = 0.02;
pub const SPLIT_RATIO: f64 = 0.3;
pub const SPLIT_MIN_PART: f64 = 0.5;
pub const MERGE_GAP: f64 = 0.15;
pub const MERGE_MAX_DUR: f64 = 15.0;

#[derive(Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub apply_merges: Vec<(String, String)>,
    pub language: Option<String>,
    pub revision: Option<String>,
}

#[derive(Default, Serialize)]
pub struct Summary {
    pub aligned_first: bool,
    pub split: usize,
    pub merged: usize,
    pub splits: Vec<SplitOp>,
    pub suggestions: Vec<MergeSuggestion>,
    pub unaligned: usize,
    pub pending_translation: usize,
    pub max_merge_duration: f64,
    pub revision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SplitOp {
    pub id: String,
    pub gap: f64,
    pub left: (f64, f64),
    pub right: (f64, f64),
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MergeSuggestion {
    pub a: String,
    pub b: String,
    pub boundary: f64,
    pub start: f64,
    pub end: f64,
    pub combined_dur: f64,
    pub combined_chars: usize,
}

pub async fn handle(
    axum::extract::State(st): axum::extract::State<crate::AppState>,
    axum::extract::Path(pid): axum::extract::Path<String>,
    axum::Json(request): axum::Json<Request>,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let dir = match st.proj_dir(&pid) { Ok(d) => d, Err(e) => return e };
    let snapshot = match st.load_project(&pid) { Ok(p) => p, Err(e) => return e };
    if snapshot.segments.is_empty() {
        return (StatusCode::CONFLICT, "REGROUP_EMPTY: Проект без реплик.").into_response();
    }
    let expected = match crate::alignment::fingerprint(&snapshot) {
        Ok(f) => f, Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    if let Err(e) = check_revision(&request.apply_merges, request.revision.as_deref(), &expected) {
        return (StatusCode::CONFLICT, e).into_response();
    }
    let models = st.models_root.clone();
    let job: crate::jobs::JobFn = Box::new(move |progress| {
        let mut result = snapshot.clone();
        let summary = run(&mut result, &dir, &models, request.language.as_deref(), &request.apply_merges, &|v| progress(v))?;
        let _guard = crate::PROJECT_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
        let current = Project::from_json(&std::fs::read_to_string(dir.join("project.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if crate::alignment::fingerprint(&current)? != expected {
            return Err("REGROUP_PROJECT_CHANGED: Проект изменён во время пересборки. Повторите запуск.".into());
        }
        crate::save_project_unlocked(&dir, &result)?;
        Ok(json!({"project_id":pid, "summary":summary, "before":snapshot, "project":result}))
    });
    Json(json!({"job_id":st.jobs.enqueue(job).await})).into_response()
}

fn check_revision(pairs: &[(String, String)], revision: Option<&str>, current: &str) -> Result<(), String> {
    if !pairs.is_empty() && revision != Some(current) {
        return Err("REGROUP_STALE: Предложения устарели. Повторите «Пересобрать» и прослушайте новые стыки.".into());
    }
    Ok(())
}

pub fn run(
    project: &mut Project, work: &Path, models: &Path, language: Option<&str>,
    pairs: &[(String, String)], progress: &dyn Fn(Value),
) -> Result<Summary, String> {
    validate_segments(&project.segments)?;
    // Transactional even for non-HTTP callers: an error leaves the input intact.
    let mut draft = project.clone();
    let mut summary = Summary { max_merge_duration: MERGE_MAX_DUR, ..Default::default() };
    if pairs.is_empty() {
        let source = crate::alignment::vocals_path(work)?;
        let hash = crate::alignment::hash_audio(&source)?;
        if draft.segments.iter().any(|s| !s.src_text.trim().is_empty() && !crate::alignment::alignment_is_current(s, &hash)) {
            progress(json!({"stage":"regroup","msg":"Пересборка: выравнивание актуального текста по вокалу","pct":0}));
            crate::alignment::run(&mut draft, work, models, language, progress)?;
            summary.aligned_first = true;
        }
        let env = load_envelope(&source)?;
        let (segments, splits) = split_segments(&draft.segments, &env);
        summary.split = splits.len();
        summary.splits = splits;
        if summary.split > 0 { replace_segments(&mut draft, segments); }
    } else {
        let (segments, merged) = apply_merges(draft.segments.clone(), pairs)?;
        summary.merged = merged;
        if merged > 0 { replace_segments(&mut draft, segments); }
    }
    finish_summary(&mut draft, &mut summary)?;
    progress(json!({"stage":"regroup","pct":100,"msg":format!(
        "Пересборка: разбито {}, склеено {}, предложений {}, без выравнивания {}",
        summary.split, summary.merged, summary.suggestions.len(), summary.unaligned)}));
    *project = draft;
    Ok(summary)
}

fn load_envelope(source: &Path) -> Result<Envelope, String> {
    let (samples, _) = dub_asr::load_wav_16k_mono(source).map_err(|e| e.to_string())?;
    if samples.is_empty() || samples.iter().any(|s| !s.is_finite()) { return Err("REGROUP_AUDIO_INVALID".into()); }
    Ok(Envelope::new(&samples))
}

fn finish_summary(project: &mut Project, summary: &mut Summary) -> Result<(), String> {
    summary.suggestions = merge_candidates(&project.segments);
    summary.unaligned = project.segments.iter().filter(|s| !s.src_text.trim().is_empty() && words(s).is_empty()).count();
    summary.pending_translation = project.segments.iter().filter(|s| !s.src_text.trim().is_empty() && s.tgt_text.trim().is_empty()).count();
    // fingerprint игнорирует extra["regroup_summary"], поэтому revision можно посчитать ДО вставки —
    // сохранённая в проект копия несёт то же значение, что и отданный API summary.
    summary.revision = crate::alignment::fingerprint(project)?;
    project.extra.insert("regroup_summary".into(), serde_json::to_value(&summary).map_err(|e| e.to_string())?);
    Ok(())
}

fn words(s: &Segment) -> Vec<TimedWord> { crate::alignment::current_words(s).unwrap_or_default() }
fn audio_hash(s: &Segment) -> String {
    s.extra.get("alignment_words").and_then(|v| v.get("audio_hash")).and_then(Value::as_str).unwrap_or_default().into()
}
fn join_words(ws: &[TimedWord]) -> String { ws.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" ") }
fn round3(t: f64) -> f64 { (t * 1000.0).round() / 1000.0 }
fn hidden(s: &Segment) -> bool { s.extra.get("hidden").and_then(Value::as_bool).unwrap_or(false) }

fn new_id(used: &mut HashSet<String>) -> String {
    loop {
        let id = format!("rg_{}", uuid::Uuid::new_v4().simple());
        if used.insert(id.clone()) { return id; }
    }
}

fn validate_segments(segments: &[Segment]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for s in segments {
        if s.id.is_empty() || !ids.insert(&s.id) || !s.start.is_finite() || !s.end.is_finite() || s.start < 0.0 || s.end <= s.start {
            return Err("REGROUP_INVALID_SEGMENTS: Проверьте ID и границы реплик.".into());
        }
    }
    if segments.windows(2).any(|s| s[0].start > s[1].start) {
        return Err("REGROUP_INVALID_ORDER: Реплики должны идти по времени.".into());
    }
    Ok(())
}

/// Greedy cuts use the start of the CURRENT part, so every output part is >=0.5s.
/// All eligible gaps are processed in one pass; a second pass is a no-op.
pub fn split_segments(segments: &[Segment], env: &Envelope) -> (Vec<Segment>, Vec<SplitOp>) {
    let mut out = Vec::with_capacity(segments.len());
    let mut ops = Vec::new();
    let mut used: HashSet<String> = segments.iter().map(|s| s.id.clone()).collect();
    for s in segments {
        let ws = words(s);
        let mut cuts = Vec::new();
        let mut from = 0;
        if !hidden(s) {
            for j in 0..ws.len().saturating_sub(1) {
                let gap = ws[j+1].start - ws[j].end;
                if gap <= SPLIT_GAP || ws[j].end - ws[from].start < SPLIT_MIN_PART
                    || ws.last().unwrap().end - ws[j+1].start < SPLIT_MIN_PART { continue; }
                let rms = env.max_rms(ws[j].end + 0.05, ws[j+1].start - 0.05);
                let peak = env.max_rms(ws[j].start, ws[j].end).min(env.max_rms(ws[j+1].start, ws[j+1].end));
                if peak <= 0.0 || rms >= SPLIT_ABS || rms >= peak * SPLIT_RATIO { continue; }
                let mid = (ws[j].end + ws[j+1].start) / 2.0;
                // Acoustic padding belongs to each new edge, bounded by silence ownership.
                let end = env.expand(&ws[j], false, s.start, mid).max(ws[j].end).min(mid);
                let start = env.expand(&ws[j+1], true, mid, s.end).min(ws[j+1].start).max(mid);
                cuts.push((j + 1, end, start, gap));
                from = j + 1;
            }
        }
        if cuts.is_empty() { out.push(s.clone()); continue; }
        let hash = audio_hash(s);
        let mut parts = Vec::new();
        let mut from = 0;
        let weights: Vec<usize> = (0..=cuts.len()).map(|k| {
            let to = cuts.get(k).map(|c| c.0).unwrap_or(ws.len());
            let from_k = if k == 0 { 0 } else { cuts[k - 1].0 };
            to.saturating_sub(from_k).max(1)
        }).collect();
        let tgt_parts = if s.tgt_text.trim().is_empty() {
            vec![String::new(); cuts.len() + 1]
        } else if s.tgt_text.trim() == s.src_text.trim() {
            (0..=cuts.len()).map(|k| {
                let to = cuts.get(k).map(|c| c.0).unwrap_or(ws.len());
                let from_k = if k == 0 { 0 } else { cuts[k - 1].0 };
                join_words(&ws[from_k..to])
            }).collect()
        } else {
            split_caption(&s.tgt_text, &weights)
        };
        for k in 0..=cuts.len() {
            let to = cuts.get(k).map(|c| c.0).unwrap_or(ws.len());
            let mut part = s.clone();
            part.id = new_id(&mut used);
            part.start = if k == 0 { s.start } else { cuts[k-1].2 };
            part.end = cuts.get(k).map(|c| c.1).unwrap_or(s.end);
            part.src_text = join_words(&ws[from..to]);
            part.tgt_text = tgt_parts.get(k).cloned().unwrap_or_else(|| part.src_text.clone());
            part.dirty = true;
            part.ckpt = None;
            part.extra.remove("regenerated");
            if part.tgt_text.trim().is_empty() {
                part.extra.insert("translation_pending".into(), json!(true));
            } else {
                part.extra.remove("translation_pending");
            }
            part.extra.insert("regroup_from".into(), json!([s.id]));
            crate::alignment::set_words(&mut part, &ws[from..to], &hash).expect("finite timed words");
            parts.push(part);
            from = to;
        }
        for (k, cut) in cuts.iter().enumerate() {
            ops.push(SplitOp { id: s.id.clone(), gap: round3(cut.3),
                left: (parts[k].start, parts[k].end), right: (parts[k+1].start, parts[k+1].end) });
        }
        out.extend(parts);
    }
    (out, ops)
}

pub fn merge_candidates(segments: &[Segment]) -> Vec<MergeSuggestion> {
    segments.windows(2).filter_map(|pair| {
        let (a, b) = (&pair[0], &pair[1]);
        let gap = b.start - a.end;
        let dur = b.end - a.start;
        let keep = |s: &Segment| s.extra.get("keep_original").and_then(Value::as_bool).unwrap_or(false);
        let lane = |s: &Segment| s.extra.get("lane").and_then(Value::as_i64).unwrap_or(0);
        if hidden(a) || hidden(b) || keep(a) != keep(b) || lane(a) != lane(b)
            || !(-0.05 < gap && gap < MERGE_GAP) || !dur.is_finite() || dur <= 0.0
            || dur > MERGE_MAX_DUR || b.end < a.end || b.start < a.start { return None; }
        Some(MergeSuggestion { a: a.id.clone(), b: b.id.clone(), boundary: round3(b.start),
            start: a.start, end: b.end, combined_dur: round3(dur),
            combined_chars: a.src_text.chars().count() + b.src_text.chars().count() + 1 })
    }).collect()
}

/// Validate all pairs and whole chains BEFORE building any output. Text length is
/// informational, not the obsolete 160-character gate; the source interval cap is 15s.
pub fn apply_merges(segments: Vec<Segment>, pairs: &[(String, String)]) -> Result<(Vec<Segment>, usize), String> {
    validate_segments(&segments)?;
    if pairs.is_empty() { return Ok((segments, 0)); }
    let candidates: HashSet<_> = merge_candidates(&segments).into_iter().map(|s| (s.a, s.b)).collect();
    let mut selected = HashSet::new();
    for pair in pairs {
        if !candidates.contains(pair) { return Err(format!("REGROUP_INVALID_PAIR: {} + {}. Обновите предложения.", pair.0, pair.1)); }
        selected.insert(pair.clone());
    }
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < segments.len() {
        let mut to = from + 1;
        while to < segments.len() && selected.contains(&(segments[to-1].id.clone(), segments[to].id.clone())) { to += 1; }
        if to > from + 1 && segments[to-1].end - segments[from].start > MERGE_MAX_DUR {
            return Err(format!("REGROUP_CHAIN_TOO_LONG: {} … {} превышает {MERGE_MAX_DUR} с. Снимите часть отметок.", segments[from].id, segments[to-1].id));
        }
        ranges.push(from..to);
        from = to;
    }
    let mut out = Vec::new();
    let mut merged = 0;
    let mut used = segments.iter().map(|s| s.id.clone()).collect();
    for range in ranges {
        let group = &segments[range];
        if group.len() == 1 { out.push(group[0].clone()); continue; }
        let mut m = group[0].clone();
        m.id = new_id(&mut used);
        m.end = group.last().unwrap().end;
        m.src_text = group.iter().map(|s| s.src_text.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        m.tgt_text = group.iter().map(|s| s.tgt_text.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        if group.iter().any(|s| !s.src_text.trim().is_empty() && s.tgt_text.trim().is_empty()) || m.tgt_text.trim().is_empty() {
            m.tgt_text.clear(); // a partial translation must not masquerade as a complete merged line
            m.extra.insert("translation_pending".into(), json!(true));
        } else {
            m.extra.remove("translation_pending");
        }
        m.dirty = true;
        m.ckpt = None;
        m.extra.remove("regenerated");
        m.extra.insert("regroup_from".into(), json!(group.iter().map(|s| &s.id).collect::<Vec<_>>()));
        let hash = audio_hash(&group[0]);
        let mut ws = Vec::new();
        let valid = group.iter().all(|s| !words(s).is_empty() && audio_hash(s) == hash);
        if valid { for s in group { ws.extend(words(s)); } }
        crate::alignment::invalidate_words(&mut m);
        if valid && ws.windows(2).all(|w| w[0].end <= w[1].start) {
            crate::alignment::set_words(&mut m, &ws, &hash)?;
            m.extra.insert("alignment_review".into(), json!(group.iter().any(|s| s.extra.get("alignment_review").and_then(Value::as_bool).unwrap_or(false))));
        }
        out.push(m);
        merged += 1;
    }
    Ok((out, merged))
}

/// Keep caption overrides and source donors attached across structural edits.
fn replace_segments(project: &mut Project, mut after: Vec<Segment>) {
    crate::segment_cache::preserve_donors(project, &mut after);
    let before_ids: HashSet<_> = project.segments.iter().map(|s| s.id.as_str()).collect();
    let mut replacements: HashMap<String, Vec<&Segment>> = HashMap::new();
    for s in &after {
        if before_ids.contains(s.id.as_str()) { continue; }
        if let Some(ids) = s.extra.get("regroup_from").and_then(Value::as_array) {
            for id in ids.iter().filter_map(Value::as_str) {
                replacements.entry(id.into()).or_default().push(s);
            }
        }
    }
    let old = &project.captions.overrides;
    let mut overrides = old.iter().filter(|o| !replacements.contains_key(&o.seg_id)).cloned().collect::<Vec<_>>();
    let mut done = HashSet::new();
    for source in &project.segments {
        let Some(parts) = replacements.get(&source.id) else { continue; };
        if parts.len() > 1 {
            for ov in old.iter().filter(|o| o.seg_id == source.id) {
                let texts = ov.text.as_ref().map(|t| split_caption(t, &parts.iter().map(|s| s.src_text.split_whitespace().count()).collect::<Vec<_>>()));
                for (i, part) in parts.iter().enumerate() {
                    let mut copy = ov.clone();
                    copy.seg_id = part.id.clone();
                    copy.text = texts.as_ref().map(|t| t[i].clone());
                    overrides.push(copy);
                }
            }
        } else if let Some(part) = parts.first() {
            if !done.insert(part.id.clone()) { continue; }
            let ids: Vec<&str> = part.extra["regroup_from"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
            if let Some(base) = ids.iter().find_map(|id| old.iter().find(|o| o.seg_id == *id)) {
                let mut copy = base.clone();
                copy.seg_id = part.id.clone();
                if ids.iter().any(|id| old.iter().any(|o| o.seg_id == *id && o.text.is_some())) {
                    copy.text = Some(ids.iter().filter_map(|id| {
                        old.iter().find(|o| o.seg_id == *id).and_then(|o| o.text.as_deref())
                            .or_else(|| project.segments.iter().find(|s| s.id == *id).map(|s| if s.tgt_text.is_empty() { s.src_text.as_str() } else { s.tgt_text.as_str() }))
                    }).collect::<Vec<_>>().join(" "));
                }
                overrides.push(copy);
            }
        }
    }
    project.captions.overrides = overrides;
    project.segments = after;
    project.audio.mix_dirty = true;
}

/// Preserve custom caption content; distribute on word boundaries, Unicode-safe.
fn split_caption(text: &str, weights: &[usize]) -> Vec<String> {
    let tokens = text.split_whitespace().collect::<Vec<_>>();
    let chars = text.chars().collect::<Vec<_>>();
    let use_chars = tokens.len() < weights.len();
    let len = if use_chars { chars.len() } else { tokens.len() };
    let total = weights.iter().sum::<usize>().max(1);
    let mut weight = 0;
    let mut from = 0;
    weights.iter().enumerate().map(|(i, w)| {
        weight += w;
        let to = if i + 1 == weights.len() { len } else { len * weight / total };
        let part = if use_chars { chars[from..to].iter().collect() } else { tokens[from..to].join(" ") };
        from = to;
        part
    }).collect()
}

/// Analyze calls this only AFTER alignment and BEFORE translation. Never merge.
pub fn auto_split(project: &mut Project, work: &Path, progress: &dyn Fn(Value)) -> Result<usize, String> {
    validate_segments(&project.segments)?;
    let source = crate::alignment::vocals_path(work)?;
    let hash = crate::alignment::hash_audio(&source)?;
    let env = load_envelope(&source)?;
    let mut draft = project.clone();
    for s in &mut draft.segments {
        if !crate::alignment::alignment_is_current(s, &hash) { crate::alignment::invalidate_words(s); }
    }
    let (segments, splits) = split_segments(&draft.segments, &env);
    let mut summary = Summary { split: splits.len(), splits, max_merge_duration: MERGE_MAX_DUR, ..Default::default() };
    if summary.split > 0 { replace_segments(&mut draft, segments); }
    finish_summary(&mut draft, &mut summary)?;
    progress(json!({"stage":"regroup","msg":format!("Пересборка: разбито {}", summary.split)}));
    *project = draft;
    Ok(summary.split)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(id: &str, intervals: &[(f64, f64)]) -> Segment {
        let ws: Vec<_> = intervals.iter().enumerate().map(|(i, &(start, end))| TimedWord { word: format!("word{i}"), start, end, score: 1.0 }).collect();
        let mut s = Segment { id: id.into(), start: ws[0].start, end: ws.last().unwrap().end,
            src_text: join_words(&ws), tgt_text: format!("translation {id}"), ..Default::default() };
        crate::alignment::set_words(&mut s, &ws, "audio").unwrap();
        s
    }

    fn envelope(intervals: &[(f64, f64)]) -> Envelope {
        let mut samples = vec![0.0; 16000 * 20];
        for &(a, b) in intervals {
            for i in (a * 16000.0) as usize..(b * 16000.0) as usize { samples[i] = 0.3 * (i as f32 * 0.05).sin(); }
        }
        Envelope::new(&samples)
    }

    #[test]
    fn all_silent_gaps_are_split_once_with_unique_ids_and_padded_edges() {
        let intervals = [(1.0, 2.0), (4.0, 5.0), (7.0, 8.0)];
        let env = envelope(&intervals);
        let input = vec![seg("s1", &intervals), seg("s11", &[(10.0, 11.0)])];
        let (once, ops) = split_segments(&input, &env);
        assert_eq!(ops.len(), 2);
        assert_eq!(once.len(), 4);
        assert_eq!(once[3].id, "s11");
        let keys: HashSet<_> = once.iter().map(|s| crate::segment_cache::key(&s.id)).collect();
        assert_eq!(keys.len(), once.len());
        assert!(once[0].end > 2.0 && once[1].start < 4.0);
        assert!(once[..3].iter().all(|s| !s.tgt_text.is_empty() && s.dirty && s.ckpt.is_none() && !words(s).is_empty()));
        let (twice, more) = split_segments(&once, &env);
        assert!(more.is_empty());
        assert_eq!(serde_json::to_value(once).unwrap(), serde_json::to_value(twice).unwrap());
    }

    #[test]
    fn split_preserves_tgt_text_for_untranslated_translated_and_empty() {
        let intervals = [(1.0, 2.0), (4.0, 5.0), (7.0, 8.0)];
        let env = envelope(&intervals);

        // Case 1: untranslated (tgt_text == src_text) -> each part gets its own matching src_text
        let mut s_untranslated = seg("u1", &intervals);
        s_untranslated.tgt_text = s_untranslated.src_text.clone();
        let (out_u, ops_u) = split_segments(&[s_untranslated], &env);
        assert_eq!(ops_u.len(), 2);
        assert_eq!(out_u.len(), 3);
        for part in &out_u {
            assert_eq!(part.tgt_text, part.src_text);
            assert!(!part.extra.contains_key("translation_pending"));
        }
        assert_eq!(out_u[0].tgt_text, "word0");
        assert_eq!(out_u[1].tgt_text, "word1");
        assert_eq!(out_u[2].tgt_text, "word2");

        // Case 2: translated (tgt_text != src_text) -> split proportionally via split_caption
        let mut s_translated = seg("t1", &intervals);
        s_translated.tgt_text = "перевод части один два три".into();
        let (out_t, ops_t) = split_segments(&[s_translated], &env);
        assert_eq!(ops_t.len(), 2);
        assert_eq!(out_t.len(), 3);
        assert_eq!(out_t.iter().map(|p| p.tgt_text.as_str()).collect::<Vec<_>>().join(" "), "перевод части один два три");
        for part in &out_t {
            assert!(!part.tgt_text.trim().is_empty());
            assert!(!part.extra.contains_key("translation_pending"));
        }

        // Case 3: empty tgt_text -> stays empty, sets translation_pending
        let mut s_empty = seg("e1", &intervals);
        s_empty.tgt_text = "".into();
        let (out_e, ops_e) = split_segments(&[s_empty], &env);
        assert_eq!(ops_e.len(), 2);
        assert_eq!(out_e.len(), 3);
        for part in &out_e {
            assert!(part.tgt_text.is_empty());
            assert_eq!(part.extra.get("translation_pending"), Some(&json!(true)));
        }
    }

    #[test]
    fn stale_text_and_continuous_speech_never_split() {
        let mut s = seg("s0", &[(1.0, 2.0), (4.0, 5.0)]);
        assert!(split_segments(&[s.clone()], &envelope(&[(1.0, 5.0)])).1.is_empty());
        s.src_text = "user corrected this".into();
        let (out, ops) = split_segments(&[s.clone()], &envelope(&[(1.0, 2.0), (4.0, 5.0)]));
        assert!(ops.is_empty());
        assert_eq!(out[0].src_text, s.src_text);
    }

    #[test]
    fn short_middle_part_is_not_created() {
        let intervals = [(1.0, 2.0), (4.0, 4.2), (7.0, 8.0)];
        let env = envelope(&intervals);
        let (out, _) = split_segments(&[seg("s0", &intervals)], &env);
        assert!(out.iter().all(|s| s.end - s.start >= SPLIT_MIN_PART));
        assert!(split_segments(&out, &env).1.is_empty());
    }

    #[test]
    fn merge_chains_respect_total_limit_and_keep_first_voice_despite_speaker_labels() {
        let mut a = seg("a", &[(0.0, 5.0)]);
        a.speaker = Some("wrong-label-a".into());
        a.voice = Some("actor".into());
        a.extra.insert("translation_pending".into(), json!(true));
        let mut b = seg("b", &[(5.0, 10.0)]);
        b.speaker = Some("wrong-label-b".into());
        let c = seg("c", &[(10.0, 15.0)]);
        let d = seg("d", &[(15.0, 16.0)]);
        let pairs = vec![("a".into(), "b".into()), ("b".into(), "c".into())];
        let (out, n) = apply_merges(vec![a.clone(), b.clone(), c.clone(), d.clone()], &pairs).unwrap();
        assert_eq!(n, 1);
        assert_eq!(out[0].end, 15.0);
        assert_eq!(out[0].voice, a.voice);
        assert_eq!(out[0].tgt_text, "translation a translation b translation c");
        assert!(!out[0].extra.contains_key("translation_pending"));
        assert_eq!(words(&out[0]).len(), 3);
        let mut too_long = pairs;
        too_long.push(("c".into(), "d".into()));
        assert!(apply_merges(vec![a, b, c, d], &too_long).unwrap_err().contains("CHAIN_TOO_LONG"));
    }

    #[test]
    fn stale_or_nonadjacent_pairs_are_errors_not_silent_noops() {
        let segments = vec![seg("a", &[(0.0, 1.0)]), seg("b", &[(1.0, 2.0)]), seg("c", &[(2.0, 3.0)])];
        assert!(apply_merges(segments.clone(), &[("a".into(), "c".into())]).is_err());
        assert!(apply_merges(segments, &[("missing".into(), "b".into())]).is_err());
        assert!(check_revision(&[("a".into(), "b".into())], Some("old"), "new").is_err());
    }

    #[test]
    fn merge_application_neither_loads_models_nor_splits_again() {
        let mut p = Project::default();
        p.segments = vec![seg("a", &[(1.0, 2.0), (4.0, 5.0)]), seg("b", &[(5.0, 6.0)])];
        let summary = run(&mut p, Path::new("missing"), Path::new("missing"), None,
            &[("a".into(), "b".into())], &|_| {}).unwrap();
        assert_eq!((summary.split, summary.merged), (0, 1));
        assert_eq!(p.segments.len(), 1);
        assert_eq!(summary.revision, crate::alignment::fingerprint(&p).unwrap());
        // revision сохраняется в extra["regroup_summary"] и переживает перезагрузку проекта
        let persisted = p.extra.get("regroup_summary").unwrap();
        assert_eq!(persisted.get("revision").and_then(Value::as_str), Some(summary.revision.as_str()));
    }

    #[test]
    fn caption_text_and_donor_survive_split_merge_and_snapshot_restore() {
        let mut p = Project::default();
        p.segments = vec![seg("s0", &[(1.0, 2.0), (4.0, 5.0)]), seg("s1", &[(6.0, 7.0)])];
        p.segments[1].voice = Some("clone:s0".into());
        p.captions.overrides.push(dub_core::CaptionOverride { seg_id: "s0".into(), text: Some("Мой исправленный текст здесь".into()), x: Some(30), ..Default::default() });
        let snapshot = p.to_json().unwrap();
        let (after, _) = split_segments(&p.segments, &envelope(&[(1.0, 2.0), (4.0, 5.0)]));
        replace_segments(&mut p, after);
        assert_eq!(p.captions.overrides.len(), 2);
        assert!(p.captions.overrides.iter().all(|o| o.x == Some(30) && p.segments.iter().any(|s| s.id == o.seg_id)));
        assert_eq!(p.captions.overrides.iter().filter_map(|o| o.text.as_deref()).collect::<Vec<_>>().join(" "), "Мой исправленный текст здесь");
        let donor = crate::segment_cache::donor(&p, &p.segments[2]).unwrap();
        assert_eq!((donor.start, donor.end), (1.0, 5.0));
        let restored = Project::from_json(&snapshot).unwrap();
        assert_eq!(restored.segments[0].id, "s0");
        assert_eq!(restored.captions.overrides[0].seg_id, "s0");
    }

    #[test]
    fn merge_preserves_caption_overrides_and_partial_translation_stays_pending() {
        let mut p = Project::default();
        p.segments = vec![seg("a", &[(0.0, 1.0)]), seg("b", &[(1.0, 2.0)])];
        p.segments[1].tgt_text.clear();
        p.captions.overrides.push(dub_core::CaptionOverride { seg_id: "b".into(), text: Some("custom caption".into()), ..Default::default() });
        let (after, _) = apply_merges(p.segments.clone(), &[("a".into(), "b".into())]).unwrap();
        replace_segments(&mut p, after);
        assert_eq!(p.captions.overrides[0].seg_id, p.segments[0].id);
        assert_eq!(p.captions.overrides[0].text.as_deref(), Some("translation a custom caption"));
        assert!(p.segments[0].tgt_text.is_empty());
        assert_eq!(p.segments[0].extra["translation_pending"], json!(true));
        let cjk = split_caption("日本語字幕", &[1, 1]);
        assert!(cjk.iter().all(|s| !s.is_empty()));
        assert_eq!(cjk.concat(), "日本語字幕");
    }

    #[test]
    fn hidden_keep_original_and_parallel_lanes_are_not_silently_lost() {
        let a = seg("a", &[(0.0, 1.0)]);
        for (field, value) in [("hidden", json!(true)), ("keep_original", json!(true)), ("lane", json!(1))] {
            let mut b = seg("b", &[(1.0, 2.0)]);
            b.extra.insert(field.into(), value);
            assert!(merge_candidates(&[a.clone(), b]).is_empty());
        }
    }

    #[test]
    fn failed_chain_leaves_project_unchanged() {
        let mut p = Project::default();
        p.segments = vec![seg("a", &[(0.0, 6.0)]), seg("b", &[(6.0, 12.0)]), seg("c", &[(12.0, 18.0)])];
        let before = p.to_json().unwrap();
        assert!(run(&mut p, Path::new("missing"), Path::new("missing"), None,
            &[("a".into(), "b".into()), ("b".into(), "c".into())], &|_| {}).is_err());
        assert_eq!(p.to_json().unwrap(), before);
    }

    #[test]
    fn new_edges_keep_quiet_speech_just_outside_ctc_words() {
        let env = envelope(&[(1.0, 2.04), (3.97, 5.0)]);
        let (out, ops) = split_segments(&[seg("s0", &[(1.0, 2.0), (4.0, 5.0)])], &env);
        assert_eq!(ops.len(), 1);
        assert!(out[0].end >= 2.04, "cropped consonant at {}", out[0].end);
        assert!(out[1].start <= 3.97, "cropped onset at {}", out[1].start);
        assert!(out[0].end < out[1].start);
    }

    /// Runs the actual Rust pipeline, on a disposable copy, never on a user's project.
    #[test]
    #[ignore = "requires REGROUP_PROJECT_DIR and ALIGN_MODELS_ROOT, plus ONNX Runtime"]
    fn real_project_is_idempotent_and_preserves_words() {
        let source = std::path::PathBuf::from(std::env::var("REGROUP_PROJECT_DIR").expect("REGROUP_PROJECT_DIR"));
        let models = std::path::PathBuf::from(std::env::var("ALIGN_MODELS_ROOT").expect("ALIGN_MODELS_ROOT"));
        let mut project = Project::from_json(&std::fs::read_to_string(source.join("project.json")).unwrap()).unwrap();
        let original_words = project.segments.iter().flat_map(|s| s.src_text.split_whitespace().map(str::to_owned)).collect::<Vec<_>>();
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let scratch = Scratch(std::env::temp_dir().join(format!("dub-regroup-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&scratch.0).unwrap();
        std::fs::copy(crate::alignment::vocals_path(&source).unwrap(), scratch.0.join("vocals16_clean.wav")).unwrap();
        let first = run(&mut project, &scratch.0, &models, Some("en"), &[], &|v| eprintln!("{v}")).unwrap();
        let after_first = serde_json::to_value(&project.segments).unwrap();
        let second = run(&mut project, &scratch.0, &models, Some("en"), &[], &|v| eprintln!("{v}")).unwrap();
        assert_eq!((second.split, second.merged), (0, 0));
        assert_eq!(serde_json::to_value(&project.segments).unwrap(), after_first);
        assert_eq!(project.segments.iter().flat_map(|s| s.src_text.split_whitespace().map(str::to_owned)).collect::<Vec<_>>(), original_words);
        let keys: HashSet<_> = project.segments.iter().map(crate::segment_cache::audio_key).collect();
        assert_eq!(keys.len(), project.segments.len());
        assert_eq!(first.merged, 0, "automatic regroup must never merge");
        assert!(project.segments.iter().all(|s| s.start >= 0.0 && s.end > s.start));
    }
}
