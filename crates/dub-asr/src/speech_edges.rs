//! Local, bounded waveform expansion, matching experiment2 (10/5 ms RMS).
use crate::forced::{round_ms, TimedWord};
pub(crate) struct Envelope { rms:Vec<f64> }
impl Envelope {
    pub fn new(samples:&[f32])->Self {
        Self {rms:samples.windows(160).step_by(80).map(|f|(f.iter().map(|x|(*x as f64).powi(2)).sum::<f64>()/160.0).sqrt()).collect()}
    }
    pub fn expand(&self,w:&TimedWord,start:bool,lo:f64,hi:f64)->f64 {
        let edge=if start{w.start}else{w.end};
        let first=((edge-0.46).max(0.0)/0.005) as usize;
        let last=(((edge+0.46)/0.005) as usize+1).min(self.rms.len());
        let indexes:Vec<usize>=(first.min(last)..last).filter(|&i|{let t=i as f64*0.005+0.005;t>=edge-0.45&&t<=edge+0.45}).collect();
        let energy:Vec<f64>=indexes.iter().map(|&i|self.rms[i]).collect();
        let times:Vec<f64>=indexes.iter().map(|&i|i as f64*0.005+0.005).collect();
        let anchors:Vec<usize>=(0..times.len()).filter(|&i|if start{times[i]>=edge&&times[i]<=w.end.min(edge+0.12)}else{times[i]>=w.start.max(edge-0.12)&&times[i]<=edge}).collect();
        let mut boundary=edge;
        if let Some(&anchor)=anchors.iter().max_by(|&&a,&&b|energy[a].total_cmp(&energy[b])) {
            let word:Vec<f64>=times.iter().zip(&energy).filter(|(t,_)|**t>=w.start&&**t<=w.end).map(|(_,e)|*e).collect();
            let peak=percentile(&word,0.85);let floor=percentile(&energy,0.10);
            let threshold=0.0001f64.max(peak*0.035).max((floor*3.0).min(peak*0.1));
            let mut candidates=Vec::new();
            for multiplier in [0.7,1.0,1.4] {
                let active=closing(&energy.iter().map(|e|*e>threshold*multiplier).collect::<Vec<_>>());
                if !active[anchor]{continue;}
                let mut a=anchor;while a>0&&active[a-1]{a-=1;}
                let mut b=anchor+1;while b<active.len()&&active[b]{b+=1;}
                if b-a<4{continue;}
                if start {
                    let t=times[a]-0.005;
                    if a>=5&&!active[a-5..a].iter().any(|x|*x)&&t>=edge-0.18&&t<=edge+0.02 {candidates.push(t.max(lo));}
                } else {
                    let t=times[b-1]+0.005;
                    if b+5<=active.len()&&!active[b..b+5].iter().any(|x|*x)&&t>=edge-0.02&&t<=edge+0.25 {candidates.push(t.min(hi));}
                }
            }
            if candidates.len()==3 {
                candidates.sort_by(f64::total_cmp);
                if candidates[2]-candidates[0]<=0.030000001 {boundary=if start{edge.min(candidates[1])}else{edge.max(candidates[1])};}
            }
        }
        round_ms(if start{(boundary-0.015).max(lo)}else{(boundary+0.020).min(hi)})
    }
}
fn percentile(values:&[f64],p:f64)->f64 {
    if values.is_empty(){return 0.0;}
    let mut values=values.to_vec();values.sort_by(f64::total_cmp);
    let at=p*(values.len()-1) as f64;let a=at.floor() as usize;let b=at.ceil() as usize;
    values[a]+(values[b]-values[a])*(at-a as f64)
}
// scipy.ndimage.binary_closing with an even four-sample structure and origin=0.
fn closing(mask:&[bool])->Vec<bool> {
    let get=|a:&[bool],i:isize|i>=0&&(i as usize)<a.len()&&a[i as usize];
    let dilated:Vec<bool>=(0..mask.len()).map(|i|(-1..=2).any(|d|get(mask,i as isize+d))).collect();
    (0..mask.len()).map(|i|(-2..=1).all(|d|get(&dilated,i as isize+d))).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_word_retains_coverage_and_respects_neighbours() {
        let env=Envelope::new(&vec![0.0;32000]);let word=TimedWord{word:"test".into(),start:0.5,end:1.0,score:1.0};
        assert_eq!(env.expand(&word,true,0.49,1.01),0.49);
        assert_eq!(env.expand(&word,false,0.49,1.01),1.01);
    }
    #[test]
    fn preserves_quiet_consonant_without_pitch_requirement() {
        let mut audio=vec![0.0;32000];for (i,x) in audio[7000..18000].iter_mut().enumerate(){*x=if i%2==0{0.03}else{-0.03};}
        let env=Envelope::new(&audio);let word=TimedWord{word:"shh".into(),start:0.50,end:1.0,score:0.9};
        assert!(env.expand(&word,true,0.0,2.0)<0.48);
        assert!(env.expand(&word,false,0.0,2.0)>1.1);
    }
}
