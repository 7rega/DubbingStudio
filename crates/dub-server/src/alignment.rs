//! Shared automatic/manual project alignment; preserves IDs, texts and speakers.
use dub_asr::forced::{self, Aligner, Input, Outcome, TimedWord};
use dub_core::{Project, Segment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::Read, path::Path};

#[derive(Default, Deserialize)]
pub struct Request { pub language: Option<String> }

pub async fn handle(
    axum::extract::State(st): axum::extract::State<crate::AppState>,
    axum::extract::Path(pid): axum::extract::Path<String>,
    axum::Json(request): axum::Json<Request>,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let dir=match st.proj_dir(&pid){Ok(d)=>d,Err(e)=>return e};
    let snapshot=match st.load_project(&pid){Ok(p)=>p,Err(e)=>return e};
    if !snapshot.segments.is_empty() {
        if let Err(e)=preflight(&st.models_root).and_then(|_|language(&snapshot,request.language.as_deref()).map(|_|())) {
            return (StatusCode::CONFLICT,e).into_response();
        }
    }
    let expected=match fingerprint(&snapshot){Ok(f)=>f,Err(e)=>return (StatusCode::INTERNAL_SERVER_ERROR,e).into_response()};
    let models=st.models_root.clone();
    let job:crate::jobs::JobFn=Box::new(move |progress| {
        let mut result=snapshot.clone();
        let cb=|v|progress(v);
        let summary=run(&mut result,&dir,&models,request.language.as_deref(),&cb)?;
        let _guard=crate::PROJECT_WRITE_LOCK.lock().map_err(|e|e.to_string())?;
        let current=Project::from_json(&std::fs::read_to_string(dir.join("project.json")).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        if fingerprint(&current)?!=expected {
            return Err("ALIGN_PROJECT_CHANGED: Проект изменён во время выравнивания. Повторите запуск.".into());
        }
        crate::save_project_unlocked(&dir,&result)?;
        Ok(json!({"project_id":pid,"summary":summary,"before":snapshot,"project":result}))
    });
    Json(json!({"job_id":st.jobs.enqueue(job).await})).into_response()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub changed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub review: usize,
    pub cached: bool,
    pub details: Vec<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Anchor {
    input: Input,
    output_start: f64,
    output_end: f64,
}
#[derive(Serialize, Deserialize)]
struct State { anchors: Vec<Anchor> }
#[derive(Serialize, Deserialize)]
struct Cache { key: String, outcomes: Vec<Outcome> }

pub fn fingerprint(project: &Project) -> Result<String,String> {
    // «regroup_summary» хранит собственный revision (= fingerprint этого же проекта) — исключаем его из
    // отпечатка: иначе значение не может быть устойчивым (self-reference) и в project.json хранится пустым.
    let mut stable = project.clone();
    stable.extra.remove("regroup_summary");
    Ok(blake3::hash(stable.to_json().map_err(|e|e.to_string())?.as_bytes()).to_hex().to_string())
}

pub fn language(project:&Project, requested:Option<&str>) -> Result<String,String> {
    let explicit=requested.filter(|l| !l.is_empty() && *l!="auto");
    let lang=explicit.or_else(|| project.meta.extra.get("detected_src_lang").and_then(Value::as_str))
        .or_else(||project.meta.extra.get("src_lang").and_then(Value::as_str)).unwrap_or("auto");
    match lang {
        "en"|"English"|"english" => Ok("en".into()),
        "auto"|"" => Err("ALIGN_LANGUAGE_REQUIRED: Укажите язык оригинала: для первой версии поддерживается английский.".into()),
        _ => Err(format!("ALIGN_LANGUAGE_UNSUPPORTED: {lang}. Установленная модель выравнивания поддерживает английский оригинал.")),
    }
}

pub fn preflight(models:&Path) -> Result<(),String> {
    if !forced::verified_model_ready(&models.join(forced::MODEL_DIR)) {
        return Err("ALIGN_MODEL_MISSING: Установите «Выравнивание по вокалу — английский» в меню моделей и компонентов (нужен ONNX Runtime).".into());
    }
    Ok(())
}

fn anchors(project:&Project)->Vec<Input> {
    let previous=project.extra.get("alignment_state").and_then(|v|serde_json::from_value::<State>(v.clone()).ok());
    project.segments.iter().map(|s| {
        if let Some(old)=previous.as_ref().and_then(|state|state.anchors.iter().find(|a|a.input.id==s.id && a.input.text==s.src_text
            && (a.output_start-s.start).abs()<0.0005 && (a.output_end-s.end).abs()<0.0005)) {
            return old.input.clone();
        }
        Input{id:s.id.clone(),text:s.src_text.clone(),start:s.start,end:s.end}
    }).collect()
}

pub fn hash_audio(path:&Path)->Result<String,String> {
    let mut f=std::fs::File::open(path).map_err(|e|e.to_string())?;
    let mut hash=blake3::Hasher::new();let mut buf=vec![0;1024*1024];
    loop {let n=f.read(&mut buf).map_err(|e|e.to_string())?;if n==0{break;}hash.update(&buf[..n]);}
    Ok(hash.finalize().to_hex().to_string())
}

pub fn vocals_path(work: &Path) -> Result<std::path::PathBuf, String> {
    ["vocals16_clean.wav", "stems/vocals.wav", "vocals16.wav", "ref_vocals16.wav", "audio_hq.wav"]
        .iter().map(|p| work.join(p)).find(|p| p.is_file())
        .ok_or_else(|| "ALIGN_AUDIO_MISSING: Вокальная дорожка отсутствует.".into())
}

fn word_signature(s: &Segment) -> String {
    blake3::hash(json!([s.src_text, s.start, s.end, s.extra.get("words")]).to_string().as_bytes())
        .to_hex().to_string()
}

/// ASR words and stale/edited forced-alignment words are never split candidates.
pub fn current_words(s: &Segment) -> Option<Vec<TimedWord>> {
    let stamp = s.extra.get("alignment_words")?;
    if stamp.get("version")?.as_str()? != forced::VERSION
        || stamp.get("signature")?.as_str()? != word_signature(s) { return None; }
    let words: Vec<TimedWord> = serde_json::from_value(s.extra.get("words")?.clone()).ok()?;
    if valid_words(&s.src_text, s.start, s.end, &words) { Some(words) } else { None }
}

pub fn words_match_audio(s: &Segment, hash: &str) -> bool {
    current_words(s).is_some()
        && s.extra.get("alignment_words").and_then(|v| v.get("audio_hash")).and_then(Value::as_str) == Some(hash)
}

/// Remember a skip for this exact input too. Otherwise a single unsupported line
/// would realign every already split neighbour on each click and cause drift.
pub fn alignment_is_current(s: &Segment, hash: &str) -> bool {
    words_match_audio(s, hash) || s.extra.get("alignment_attempt").is_some_and(|stamp| {
        stamp.get("version").and_then(Value::as_str) == Some(forced::VERSION)
            && stamp.get("audio_hash").and_then(Value::as_str) == Some(hash)
            && stamp.get("signature").and_then(Value::as_str) == Some(word_signature(s).as_str())
    })
}

pub fn set_words(s: &mut Segment, words: &[TimedWord], audio_hash: &str) -> Result<(), String> {
    s.extra.remove("alignment_attempt");
    s.extra.insert("words".into(), serde_json::to_value(words).map_err(|e| e.to_string())?);
    s.extra.insert("alignment_words".into(), json!({
        "version": forced::VERSION, "audio_hash": audio_hash, "signature": word_signature(s),
    }));
    Ok(())
}

pub fn invalidate_words(s: &mut Segment) {
    for key in ["words", "alignment_words", "alignment_review", "alignment_attempt"] { s.extra.remove(key); }
}

fn valid_words(text: &str, start: f64, end: f64, words: &[TimedWord]) -> bool {
    start.is_finite() && end.is_finite() && start >= 0.0 && end > start && !words.is_empty()
        && words.iter().map(|w| w.word.as_str()).eq(text.split_whitespace())
        && words.iter().all(|w| w.start.is_finite() && w.end.is_finite()
            && w.score.is_finite() && (0.0..=1.0).contains(&w.score)
            && w.start >= start && w.end <= end && w.end >= w.start)
        && words.windows(2).all(|w| w[0].end <= w[1].start)
}

/// Computes everything before mutating project. Cache failure never prevents a
/// successful alignment, and a corrupt cache is treated as a miss.
pub fn run(project:&mut Project, work:&Path, models:&Path, requested:Option<&str>, progress:&dyn Fn(Value))->Result<Summary,String> {
    if project.segments.is_empty(){return Ok(Summary::default());}
    let lang=language(project,requested)?;
    if project.segments.iter().all(|s|s.src_text.trim().is_empty()) {
        return Err("ALIGN_SOURCE_TEXT_REQUIRED: Для выравнивания нужен текст оригинала, а не только перевод.".into());
    }
    preflight(models)?;
    let source = vocals_path(work)?;
    let inputs=anchors(project);
    if inputs.iter().any(|s|!s.start.is_finite()||!s.end.is_finite()) {return Err("ALIGN_INVALID_BOUNDS".into());}
    let audio_hash=hash_audio(&source)?;
    let (samples,_) = dub_asr::load_wav_16k_mono(&source).map_err(|e|e.to_string())?;
    if samples.is_empty() || samples.iter().any(|x| !x.is_finite()) { return Err("ALIGN_AUDIO_INVALID".into()); }
    let duration = samples.len() as f64 / 16000.0;
    let speakers:Vec<_>=project.segments.iter().map(|s|&s.speaker).collect();
    let key=blake3::hash(serde_json::to_vec(&json!([forced::VERSION,forced::REVISION,audio_hash,lang,inputs,speakers])).map_err(|e|e.to_string())?.as_slice()).to_hex().to_string();
    let path=work.join("alignment-cache.json");
    let cached=std::fs::read(&path).ok().and_then(|bytes|serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|c|c.key==key && valid_outcomes(&inputs,&c.outcomes)
            && c.outcomes.iter().all(|o| o.aligned.as_ref().is_none_or(|a| a.end <= duration + 0.001)));
    let was_cached=cached.is_some();
    progress(json!({"stage":"aligning","msg":if was_cached{"Выравнивание: проверенный результат из кэша"}else{"Выравнивание слов по английскому вокалу"},"pct":0}));
    let outcomes=if let Some(c)=cached {c.outcomes} else {
        let mut aligner=Aligner::load(&models.join(forced::MODEL_DIR))?;
        aligner.align(&inputs,&samples,&|done,total|progress(json!({"stage":"aligning","msg":format!("Выравнивание: окно {done}/{total}"),"pct":done as f64/total.max(1) as f64*95.0})))?
    };
    if !valid_outcomes(&inputs,&outcomes){return Err("ALIGN_INVALID_RESULT".into());}
    if outcomes.iter().any(|o| o.aligned.as_ref().is_some_and(|a| a.end > duration + 0.001)) { return Err("ALIGN_INVALID_RESULT".into()); }
    let summary=apply(project,&inputs,&outcomes,was_cached,&audio_hash)?;
    project.meta.extra.insert("src_lang".into(),json!(lang));
    project.extra.insert("alignment_summary".into(),serde_json::to_value(&summary).map_err(|e|e.to_string())?);
    // The caller serializes/commits the project only after this function returns.
    if let Ok(bytes)=serde_json::to_vec(&Cache{key,outcomes}) {
        let tmp=work.join("alignment-cache.json.tmp");
        if std::fs::write(&tmp,bytes).is_ok(){let _=std::fs::rename(tmp,path);}
    }
    progress(json!({"stage":"aligning","pct":100,"msg":format!("Выравнивание: изменено {}, без изменений {}, пропущено {}",summary.changed,summary.unchanged,summary.skipped)}));
    Ok(summary)
}

fn valid_outcomes(inputs:&[Input], outcomes:&[Outcome])->bool {
    let unique: std::collections::HashSet<_> = inputs.iter().map(|s| &s.id).collect();
    inputs.len()==outcomes.len() && unique.len() == inputs.len() && inputs.iter().zip(outcomes).all(|(s,o)| {
        s.id==o.id && o.aligned.as_ref().map(|a| {
            valid_words(&s.text, a.start, a.end, &a.words)
        }).unwrap_or(true)
    })
        && (1..inputs.len()).all(|i| {
            let left = outcomes[i-1].aligned.as_ref().map(|a| a.end).unwrap_or(inputs[i-1].end);
            let right = outcomes[i].aligned.as_ref().map(|a| a.start).unwrap_or(inputs[i].start);
            inputs[i-1].end > inputs[i].start || left <= right
        })
}

fn apply(project:&mut Project,inputs:&[Input],outcomes:&[Outcome],cached:bool,audio_hash:&str)->Result<Summary,String> {
    let mut summary=Summary{cached,..Default::default()};
    let mut stored=Vec::new();
    for ((s,input),o) in project.segments.iter_mut().zip(inputs).zip(outcomes) {
        if let Some(a)=&o.aligned {
            let moved=(s.start-a.start).abs()>0.0005||(s.end-a.end).abs()>0.0005;
            if moved {
                s.start=a.start;s.end=a.end;
                crate::segment_cache::invalidate_audio(s);
                summary.changed+=1;
            } else {summary.unchanged+=1;}
            set_words(s, &a.words, audio_hash)?;
            s.extra.insert("alignment_review".into(),json!(a.review));
            if a.review{summary.review+=1;}
        } else {
            invalidate_words(s);
            s.extra.insert("alignment_attempt".into(), json!({
                "version": forced::VERSION, "audio_hash": audio_hash, "signature": word_signature(s), "reason": o.reason,
            }));
            summary.skipped+=1;
            summary.details.push(json!({"id":s.id,"reason":o.reason}));
        }
        stored.push(Anchor{input:input.clone(),output_start:s.start,output_end:s.end});
    }
    if summary.changed>0 {project.audio.mix_dirty=true;}
    project.extra.insert("alignment_state".into(),serde_json::to_value(State{anchors:stored}).map_err(|e|e.to_string())?);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dub_asr::forced::{Aligned,TimedWord};
    #[test]
    fn preserves_identity_translation_speaker_and_is_idempotent() {
        let mut p=Project::default();
        p.segments.push(dub_core::Segment{id:"keep-id".into(),src_text:"Hello.".into(),tgt_text:"Привет.".into(),speaker:Some("7".into()),voice:Some("actor".into()),start:0.1,end:1.0,..Default::default()});
        let input=anchors(&p);
        let out=vec![Outcome{id:"keep-id".into(),reason:None,aligned:Some(Aligned{start:0.3,end:1.1,review:false,words:vec![TimedWord{word:"Hello.".into(),start:0.32,end:1.08,score:0.9}]})}];
        assert!(valid_outcomes(&input,&out));
        assert_eq!(apply(&mut p,&input,&out,false,"audio").unwrap().changed,1);
        let second=anchors(&p);assert_eq!(second[0].start,0.1);
        assert_eq!(apply(&mut p,&second,&out,true,"audio").unwrap().changed,0);
        assert_eq!(p.segments.len(),1);assert_eq!(p.segments[0].tgt_text,"Привет.");
        assert_eq!(p.segments[0].speaker.as_deref(),Some("7"));assert_eq!(p.segments[0].voice.as_deref(),Some("actor"));
        p.segments[0].start=0.4;assert_eq!(anchors(&p)[0].start,0.4);
    }
    #[test]
    fn auto_language_never_assumes_english() {
        let mut p=Project::default();assert!(language(&p,None).is_err());
        p.meta.extra.insert("detected_src_lang".into(),json!("ja"));assert!(language(&p,None).is_err());
        assert_eq!(language(&p,Some("en")).unwrap(),"en");
    }

    #[test]
    fn word_stamp_survives_disk_roundtrip_but_not_edits_or_audio_replacement() {
        let mut p = Project::default();
        let mut s = Segment { id: "s0".into(), src_text: "Hello world".into(), start: 0.123, end: 2.987, ..Default::default() };
        let words = vec![TimedWord { word: "Hello".into(), start: 0.321, end: 1.111, score: 0.31234568 },
            TimedWord { word: "world".into(), start: 2.222, end: 2.876, score: 0.9876543 }];
        set_words(&mut s, &words, "audio-a").unwrap();
        p.segments.push(s);
        let p = Project::from_json(&p.to_json_pretty().unwrap()).unwrap();
        assert!(words_match_audio(&p.segments[0], "audio-a"));
        assert!(!words_match_audio(&p.segments[0], "audio-b"));
        let mut changed = p.segments[0].clone();
        changed.src_text = "Edited words".into();
        assert!(current_words(&changed).is_none());
        let mut changed = p.segments[0].clone();
        changed.start += 0.01;
        assert!(current_words(&changed).is_none());
        let mut translated = p.segments[0].clone();
        translated.tgt_text = "Привет, мир".into();
        translated.speaker = Some("corrected speaker label".into());
        assert!(current_words(&translated).is_some());
    }

    #[test]
    fn skipped_alignment_removes_old_word_timestamps() {
        let mut p = Project::default();
        let mut s = Segment { id: "s0".into(), src_text: "old".into(), start: 0.1, end: 1.0, ..Default::default() };
        set_words(&mut s, &[TimedWord { word: "old".into(), start: 0.2, end: 0.9, score: 0.9 }], "audio").unwrap();
        s.src_text = "123".into();
        p.segments.push(s);
        let inputs = anchors(&p);
        let outcomes = vec![Outcome { id: "s0".into(), aligned: None, reason: Some("unsupported_text".into()) }];
        apply(&mut p, &inputs, &outcomes, false, "audio").unwrap();
        assert!(!p.segments[0].extra.contains_key("words"));
        assert!(current_words(&p.segments[0]).is_none());
        assert!(alignment_is_current(&p.segments[0], "audio"));
        assert!(!alignment_is_current(&p.segments[0], "new-audio"));
        assert_eq!(p.segments[0].src_text, "123");
    }

    #[test]
    fn rejects_cached_cross_segment_overlap_and_duplicate_ids() {
        let inputs = vec![Input { id: "a".into(), text: "one".into(), start: 0.0, end: 1.0 },
            Input { id: "b".into(), text: "two".into(), start: 1.0, end: 2.0 }];
        let outcomes = inputs.iter().enumerate().map(|(i, s)| {
            let start = if i == 0 { 0.0 } else { 0.8 };
            Outcome { id: s.id.clone(), reason: None, aligned: Some(Aligned {
                start, end: s.end, review: false,
                words: vec![TimedWord { word: s.text.clone(), start, end: s.end, score: 1.0 }],
            }) }
        }).collect::<Vec<_>>();
        assert!(!valid_outcomes(&inputs, &outcomes));
        let mut duplicated = inputs.clone();
        duplicated[1].id = "a".into();
        let skipped = duplicated.iter().map(|s| Outcome { id: s.id.clone(), reason: None, aligned: None }).collect::<Vec<_>>();
        assert!(!valid_outcomes(&duplicated, &skipped));
    }
}
