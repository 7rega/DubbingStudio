//! English text-conditioned alignment. The model and time mapping are pinned
//! to the validated WhisperX reference; segment identity is never reconstructed.
use ort::{session::Session, value::TensorRef};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, path::Path};

pub const VERSION: &str = "wav2vec2-en-fp32-edges-v1";
pub const COMPONENT: &str = "alignment-en";
pub const MODEL_DIR: &str = "alignment/en";
pub const REVISION: &str = "a19f851b3d42865797e410752b4c570c871e4825";
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub hash: &'static str,
}
macro_rules! model_file {
    ($name:literal, $remote:literal, $size:literal, $hash:literal) => {
        ModelFile { name: $name, url: concat!("https://huggingface.co/Xenova/wav2vec2-base-960h/resolve/a19f851b3d42865797e410752b4c570c871e4825/", $remote), size: $size, hash: $hash }
    };
}
pub const FILES: &[ModelFile] = &[
    model_file!("model.onnx", "onnx/model.onnx", 377887594, "5659fcc79c33b1000eecf88f8f43bae2c7a9898f608f3c0295f015e1cd3f46d3"),
    model_file!("vocab.json", "vocab.json", 358, "795edde10fe7ae15e260d4f67fe453913ea7f67f69f857755c15ce839a7ea6e9"),
    model_file!("config.json", "config.json", 2094, "aeeb74ef2996494acdc86f224d4df1a2da22007ca2135ea6b3f94b0655491655"),
    model_file!("preprocessor_config.json", "preprocessor_config.json", 215, "288b3cfae2bedc4fc6de73bfd3fec4b3d29d5d5db5fbd1980884433cd3e60371"),
];

pub fn file_valid(path: &Path, spec: &ModelFile) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    if f.metadata().map(|m| m.len()).unwrap_or(0) != spec.size { return false; }
    let mut hash = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => { hash.update(&buf[..n]); }
            Err(_) => return false,
        }
    }
    hash.finalize().to_hex().as_str() == spec.hash
}

pub fn model_ready(dir: &Path) -> bool {
    FILES.iter().all(|s| std::fs::metadata(dir.join(s.name)).map(|m| m.len() == s.size).unwrap_or(false))
}

/// Hash once per file metadata revision, not on every capabilities poll.
pub fn verified_model_ready(dir: &Path) -> bool {
    type Signature = Vec<(u64, std::time::SystemTime)>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<std::path::PathBuf,(Signature,bool)>>> = std::sync::OnceLock::new();
    let signature: Option<Signature> = FILES.iter().map(|s| {
        let m=std::fs::metadata(dir.join(s.name)).ok()?;
        Some((m.len(),m.modified().ok()?))
    }).collect();
    let Some(signature)=signature else{return false;};
    let Ok(mut cache)=CACHE.get_or_init(Default::default).lock() else{return false;};
    if let Some((old,ready))=cache.get(dir){if old==&signature{return *ready;}}
    let ready=FILES.iter().all(|s|file_valid(&dir.join(s.name),s));
    cache.insert(dir.to_path_buf(),(signature,ready));ready
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
    fn skipped(id: &str, reason: &str) -> Self {
        Self { id: id.into(), aligned: None, reason: Some(reason.into()) }
    }
}

pub struct Aligner {
    session: Session,
    dictionary: HashMap<char, usize>,
    columns: Vec<usize>,
}
impl Aligner {
    pub fn load(dir: &Path) -> Result<Self, String> {
        for spec in FILES {
            if !file_valid(&dir.join(spec.name), spec) {
                return Err(format!("ALIGN_MODEL_MISSING: {}. Установите «Выравнивание по вокалу — английский» в меню компонентов.", spec.name));
            }
        }
        let vocab: HashMap<String, usize> = serde_json::from_slice(&std::fs::read(dir.join("vocab.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        // Torchaudio's reference head excludes HF's <s>, </s>, <unk> columns.
        let mut columns = vec![0usize];
        let mut chars: Vec<(char, usize)> = vocab.iter().filter_map(|(c,&i)| {
            let mut it = c.chars();
            let ch = it.next()?;
            (it.next().is_none()).then_some((ch.to_ascii_lowercase(),i))
        }).collect();
        chars.sort_by_key(|(_,i)| *i);
        let mut dictionary = HashMap::new();
        for (ch,index) in chars { dictionary.insert(ch,columns.len()); columns.push(index); }
        crate::ensure_ort_dylib();
        let session = Session::builder().map_err(|e| e.to_string())?
            .with_intra_threads(6).map_err(|e| e.to_string())?
            .commit_from_file(dir.join("model.onnx")).map_err(|e| format!("ALIGN_RUNTIME: {e}"))?;
        Ok(Self { session, dictionary, columns })
    }

    fn emissions(&mut self, wave: &[f32]) -> Result<Vec<Vec<f32>>, String> {
        let tensor = TensorRef::from_array_view(([1i64, wave.len() as i64], wave)).map_err(|e| e.to_string())?;
        let output = self.session.run(ort::inputs![tensor]).map_err(|e| format!("ALIGN_INFERENCE: {e}"))?;
        let (_,value) = output.iter().next().ok_or("ALIGN_INFERENCE: no output")?;
        let (shape,data) = value.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if shape.len()!=3 || shape[0]!=1 || shape[2]!=32 { return Err("ALIGN_INFERENCE: unexpected model shape".into()); }
        let mut rows = Vec::with_capacity(shape[1] as usize);
        for frame in data.chunks_exact(32) {
            let mut row: Vec<f32> = self.columns.iter().map(|&i| frame[i]).collect();
            if row.iter().any(|x| !x.is_finite()) { return Err("ALIGN_INFERENCE: non-finite emission".into()); }
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let sum: f32 = row.iter().map(|x| (*x-max).exp()).sum();
            let norm = max+sum.ln();
            for x in &mut row { *x -= norm; }
            let wildcard = row[1..].iter().copied().fold(f32::NEG_INFINITY, f32::max);
            row.push(wildcard);
            rows.push(row);
        }
        Ok(rows)
    }

    pub fn align(&mut self, inputs: &[Input], samples: &[f32], progress: &dyn Fn(usize,usize)) -> Result<Vec<Outcome>,String> {
        let duration = samples.len() as f64 / 16000.0;
        let mut result: Vec<Outcome> = inputs.iter().map(|s| Outcome::skipped(&s.id,"not_aligned")).collect();
        let valid: Vec<bool> = inputs.iter().enumerate().map(|(i,s)| {
            s.start.is_finite() && s.end.is_finite() && s.start>=0.0 && s.end>s.start && s.end<=duration+0.02
                && s.end-s.start<=28.0 && !s.text.trim().is_empty()
                && s.text.split_whitespace().all(|w| w.chars().any(|c| c.is_ascii_alphabetic()) && !w.chars().any(|c| c.is_numeric() || (c.is_alphabetic() && !c.is_ascii())))
                && (i==0 || inputs[i-1].end<=s.start) && (i+1==inputs.len() || s.end<=inputs[i+1].start)
        }).collect();
        for (i,ok) in valid.iter().enumerate() { if !ok { result[i].reason=Some("unsupported_text_bounds_or_overlap".into()); } }
        let groups = groups(inputs, &valid);
        for (gi,members) in groups.iter().enumerate() {
            let first=members[0]; let last=*members.last().unwrap();
            let mut context=members.clone();
            if first>0 && valid[first-1] && inputs[first].start-inputs[first-1].end<1.2 { context.insert(0,first-1); }
            if last+1<inputs.len() && valid[last+1] && inputs[last+1].start-inputs[last].end<1.2 { context.push(last+1); }
            let start=(inputs[context[0]].start-0.8).max(0.0);
            let end=(inputs[*context.last().unwrap()].end+0.8).min(duration);
            if end-start>60.0 { for &i in members { result[i].reason=Some("window_too_long".into()); } continue; }
            let text=context.iter().map(|&i| inputs[i].text.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" ");
            let chars: Vec<char> = text.chars().collect();
            let a=(start*16000.0) as usize; let b=((end*16000.0) as usize).min(samples.len());
            if b.saturating_sub(a)<400 { continue; }
            let emissions=self.emissions(&samples[a..b])?;
            let wildcard=emissions[0].len()-1;
            let tokens: Vec<usize> = chars.iter().map(|c| {
                let c=if *c==' ' { '|' } else if *c=='’' { '\'' } else { c.to_ascii_lowercase() };
                *self.dictionary.get(&c).unwrap_or(&wildcard)
            }).collect();
            let Some(spans)=ctc_path(&emissions,&tokens) else { for &i in members { result[i].reason=Some("ctc_path_failed".into()); } continue; };
            let mut words=Vec::new();
            let mut offset=0;
            for word in text.split_whitespace() {
                let len=word.chars().count();
                let indexes: Vec<usize>=(offset..offset+len).filter(|&i| chars[i].is_ascii_alphabetic()).collect();
                if indexes.is_empty() { return Err("ALIGN_TEXT: empty lexical word".into()); }
                let begin=spans[indexes[0]].0; let finish=spans[*indexes.last().unwrap()].1;
                let scale=(end-start)/emissions.len() as f64;
                words.push(TimedWord { word:word.into(), start:round_ms(start+begin as f64*scale), end:round_ms(start+finish as f64*scale), score:indexes.iter().map(|&j| spans[j].2).sum::<f32>()/indexes.len() as f32 });
                offset+=len+1;
            }
            let mut wi=0;
            for &i in &context {
                let count=inputs[i].text.split_whitespace().count();
                if members.contains(&i) {
                    let ws=words[wi..wi+count].to_vec();
                    let start=ws[0].start; let end=ws.last().unwrap().end;
                    if end>start && (start-inputs[i].start).abs()<=1.5 && (end-inputs[i].end).abs()<=1.5 {
                        result[i]=Outcome { id:inputs[i].id.clone(), reason:None, aligned:Some(Aligned { start,end,review:ws.iter().any(|w| w.score<0.3),words:ws }) };
                    } else { result[i].reason=Some("implausible_bounds".into()); }
                }
                wi+=count;
            }
            progress(gi+1,groups.len());
        }
        // Acoustic ownership uses the original lexical boundaries of ALL
        // neighbours, never the previously expanded subtitle frame.
        let raw=result.clone();
        let envelope=crate::speech_edges::Envelope::new(samples);
        for (i,out) in result.iter_mut().enumerate() {
            if let Some(aligned)=out.aligned.as_mut() {
                let left=if i==0 {0.0} else { raw[i-1].aligned.as_ref().map(|a| a.end).unwrap_or(inputs[i-1].end) };
                let right=if i+1==inputs.len() {duration} else {raw[i+1].aligned.as_ref().map(|a| a.start).unwrap_or(inputs[i+1].start)};
                if left>aligned.start || right<aligned.end { *out=Outcome::skipped(&out.id,"neighbour_conflict"); continue; }
                let lo=if i==0 {0.0} else {(left+aligned.start)/2.0};
                let hi=if i+1==inputs.len() {duration} else {(right+aligned.end)/2.0};
                aligned.start=envelope.expand(&aligned.words[0],true,lo,hi);
                aligned.end=envelope.expand(aligned.words.last().unwrap(),false,lo,hi);
            }
        }
        // Rejected candidates can expose an unchanged neighbour. Revalidate the
        // final set; never clip a spoken word to hide a cross-segment conflict.
        loop {
            let mut reject=Vec::new();
            for i in 1..inputs.len() {
                let prev=result[i-1].aligned.as_ref().map(|a| a.end).unwrap_or(inputs[i-1].end);
                let next=result[i].aligned.as_ref().map(|a| a.start).unwrap_or(inputs[i].start);
                if prev>next && inputs[i-1].end<=inputs[i].start {
                    if result[i-1].aligned.is_some(){reject.push(i-1);}
                    if result[i].aligned.is_some(){reject.push(i);}
                }
            }
            if reject.is_empty(){break;}
            for i in reject {result[i]=Outcome::skipped(&inputs[i].id,"neighbour_conflict");}
        }
        Ok(result)
    }
}

fn groups(inputs:&[Input], valid:&[bool])->Vec<Vec<usize>> {
    let mut groups=Vec::new(); let mut cur:Vec<usize>=Vec::new();
    for (i,s) in inputs.iter().enumerate() {
        if !cur.is_empty() && (!valid[i] || i!=cur[cur.len()-1]+1 || s.start-inputs[*cur.last().unwrap()].end>=1.2 || s.end-inputs[cur[0]].start>18.0) { groups.push(std::mem::take(&mut cur)); }
        if valid[i] {cur.push(i);}
    }
    if !cur.is_empty(){groups.push(cur);}
    groups
}

pub(crate) fn round_ms(t:f64)->f64 { (t*1000.0).round()/1000.0 }

// Same trellis/backtracking as the pinned WhisperX reference. Retain punctuation
// as wildcard context, but only alphabetic spans define exported word edges.
fn ctc_path(emission:&[Vec<f32>],tokens:&[usize])->Option<Vec<(usize,usize,f32)>> {
    let n=emission.len(); let m=tokens.len(); let stride=m+1;
    if m==0 || m>n || (n+1).checked_mul(stride)?>20_000_000 {return None;}
    let mut table=vec![f32::NEG_INFINITY;(n+1)*stride]; table[0]=0.0;
    let mut blank=0.0;
    for t in 1..=n {blank+=emission[t-1][0];table[t*stride]=if t>=n+1-m {f32::INFINITY}else{blank};}
    for t in 0..n {for j in 1..=m {
        table[(t+1)*stride+j]=(table[t*stride+j]+emission[t][0]).max(table[t*stride+j-1]+emission[t][tokens[j-1]]);
    }}
    let mut best=0; for t in 1..=n {if table[t*stride+m]>table[best*stride+m]{best=t;}}
    let mut j=m; let mut points=Vec::new();
    for t in (1..=best).rev() {
        let stay=table[(t-1)*stride+j]+emission[t-1][0];
        let change=table[(t-1)*stride+j-1]+emission[t-1][tokens[j-1]];
        let changed=change>stay;
        points.push((j-1,t-1,emission[t-1][if changed{tokens[j-1]}else{0}].exp()));
        if changed {j-=1;if j==0{break;}}
    }
    if j!=0{return None;}
    points.reverse();
    let mut spans=vec![(usize::MAX,0,0.0f32);m];let mut counts=vec![0usize;m];
    for (j,t,p) in points {spans[j].0=spans[j].0.min(t);spans[j].1=t+1;spans[j].2+=p;counts[j]+=1;}
    for (span,count) in spans.iter_mut().zip(counts) {if count==0{return None;}span.2/=count as f32;}
    Some(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ctc_keeps_token_order_and_handles_silence() {
        let e=vec![vec![-0.01,-9.0,-9.0],vec![-9.0,-0.01,-9.0],vec![-0.01,-9.0,-9.0],vec![-9.0,-9.0,-0.01],vec![-0.01,-9.0,-9.0]];
        let spans=ctc_path(&e,&[1,2]).unwrap();
        assert_eq!(spans.len(),2);assert!(spans[0].1<=spans[1].0);assert_eq!(spans[1].1,4);
        assert!(ctc_path(&e,&[1;6]).is_none());
    }
    #[test]
    fn groups_do_not_bridge_invalid_overlap() {
        let rows=(0..4).map(|i|Input{id:i.to_string(),text:"word".into(),start:i as f64,end:i as f64+0.5}).collect::<Vec<_>>();
        assert_eq!(groups(&rows,&[true,false,false,true]),vec![vec![0],vec![3]]);
    }
}
