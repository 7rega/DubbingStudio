//! Shared automatic/manual project alignment; preserves IDs, texts and speakers.
use dub_asr::forced::{self, Aligner, Input, Outcome};
use dub_core::Project;
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
        let mut result=snapshot;
        let cb=|v|progress(v);
        let summary=run(&mut result,&dir,&models,request.language.as_deref(),&cb)?;
        let _guard=crate::PROJECT_WRITE_LOCK.lock().map_err(|e|e.to_string())?;
        let current=Project::from_json(&std::fs::read_to_string(dir.join("project.json")).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        if fingerprint(&current)?!=expected {
            return Err("ALIGN_PROJECT_CHANGED: Проект изменён во время выравнивания. Повторите запуск.".into());
        }
        crate::save_project_unlocked(&dir,&result)?;
        Ok(json!({"project_id":pid,"summary":summary}))
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
    Ok(blake3::hash(project.to_json().map_err(|e|e.to_string())?.as_bytes()).to_hex().to_string())
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

fn hash_audio(path:&Path)->Result<String,String> {
    let mut f=std::fs::File::open(path).map_err(|e|e.to_string())?;
    let mut hash=blake3::Hasher::new();let mut buf=vec![0;1024*1024];
    loop {let n=f.read(&mut buf).map_err(|e|e.to_string())?;if n==0{break;}hash.update(&buf[..n]);}
    Ok(hash.finalize().to_hex().to_string())
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
    let source=["vocals16_clean.wav","stems/vocals.wav","vocals16.wav","ref_vocals16.wav","audio_hq.wav"]
        .iter().map(|p|work.join(p)).find(|p|p.is_file()).ok_or("ALIGN_AUDIO_MISSING: Вокальная дорожка отсутствует.")?;
    let inputs=anchors(project);
    if inputs.iter().any(|s|!s.start.is_finite()||!s.end.is_finite()) {return Err("ALIGN_INVALID_BOUNDS".into());}
    let audio_hash=hash_audio(&source)?;
    let speakers:Vec<_>=project.segments.iter().map(|s|&s.speaker).collect();
    let key=blake3::hash(serde_json::to_vec(&json!([forced::VERSION,forced::REVISION,audio_hash,lang,inputs,speakers])).map_err(|e|e.to_string())?.as_slice()).to_hex().to_string();
    let path=work.join("alignment-cache.json");
    let cached=std::fs::read(&path).ok().and_then(|bytes|serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|c|c.key==key && valid_outcomes(&inputs,&c.outcomes));
    let was_cached=cached.is_some();
    progress(json!({"stage":"aligning","msg":if was_cached{"Выравнивание: проверенный результат из кэша"}else{"Выравнивание слов по английскому вокалу"},"pct":0}));
    let outcomes=if let Some(c)=cached {c.outcomes} else {
        let (samples,_)=dub_asr::load_wav_16k_mono(&source).map_err(|e|e.to_string())?;
        if samples.iter().any(|x|!x.is_finite()){return Err("ALIGN_AUDIO_INVALID".into());}
        let mut aligner=Aligner::load(&models.join(forced::MODEL_DIR))?;
        aligner.align(&inputs,&samples,&|done,total|progress(json!({"stage":"aligning","msg":format!("Выравнивание: окно {done}/{total}"),"pct":done as f64/total.max(1) as f64*95.0})))?
    };
    if !valid_outcomes(&inputs,&outcomes){return Err("ALIGN_INVALID_RESULT".into());}
    let summary=apply(project,&inputs,&outcomes,was_cached)?;
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
    inputs.len()==outcomes.len() && inputs.iter().zip(outcomes).all(|(s,o)| {
        s.id==o.id && o.aligned.as_ref().map(|a| {
            a.start.is_finite()&&a.end.is_finite()&&a.start>=0.0&&a.end>a.start&&!a.words.is_empty()
                && a.words.iter().map(|w|w.word.as_str()).eq(s.text.split_whitespace())
                && a.words.iter().all(|w|w.start.is_finite()&&w.end.is_finite()&&w.score.is_finite()&&w.start>=a.start&&w.end<=a.end&&w.end>=w.start)
                && a.words.windows(2).all(|w|w[0].end<=w[1].start)
        }).unwrap_or(true)
    })
}

fn apply(project:&mut Project,inputs:&[Input],outcomes:&[Outcome],cached:bool)->Result<Summary,String> {
    let mut summary=Summary{cached,..Default::default()};
    let mut stored=Vec::new();
    for ((s,input),o) in project.segments.iter_mut().zip(inputs).zip(outcomes) {
        if let Some(a)=&o.aligned {
            let moved=(s.start-a.start).abs()>0.0005||(s.end-a.end).abs()>0.0005;
            if moved {s.start=a.start;s.end=a.end;s.dirty=true;summary.changed+=1;} else {summary.unchanged+=1;}
            s.extra.insert("words".into(),serde_json::to_value(&a.words).map_err(|e|e.to_string())?);
            s.extra.insert("alignment_review".into(),json!(a.review));
            if a.review{summary.review+=1;}
        } else {
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
        assert_eq!(apply(&mut p,&input,&out,false).unwrap().changed,1);
        let second=anchors(&p);assert_eq!(second[0].start,0.1);
        assert_eq!(apply(&mut p,&second,&out,true).unwrap().changed,0);
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
}
