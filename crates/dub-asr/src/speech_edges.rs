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
            let mut consensus=false;
            if candidates.len()==3 {
                candidates.sort_by(f64::total_cmp);
                if candidates[2]-candidates[0]<=0.030000001 {boundary=if start{edge.min(candidates[1])}else{edge.max(candidates[1])};consensus=true;}
            }
            // Sustained/shouted tails (approved variant B): when the quiet-support
            // consensus fails, walk the decaying envelope forward. A single short
            // dip may bridge into a following burst (drawn-out shout); stop where
            // that burst fades below 15% of its own peak for three frames.
            if !start && !consensus {
                let quiet=(threshold*0.35).max(0.0001);
                let i0=indexes.iter().copied().min_by(|&a,&b|((a as f64*0.005+0.005)-edge).abs().total_cmp(&(((b as f64*0.005+0.005)-edge).abs()))).unwrap_or(0);
                if self.rms.get(i0).copied().unwrap_or(0.0)>=quiet {
                    let limit=((edge+0.900)/0.005) as usize;
                    let hi_i=self.rms.len().min(((hi)/0.005) as usize);
                    let stop=limit.min(hi_i);
                    let mut i=i0; let mut last=i0; let mut a_end=edge; let mut b_end=edge;
                    let mut a_done=false; let mut bridged=false;
                    while i+1<stop {
                        let t=(i+1) as f64*0.005+0.005;
                        let cur=self.rms[i+1]; let prev=self.rms[i];
                        if cur<quiet {break;}
                        if cur>prev*1.25&&cur>quiet*2.0 {
                            if !a_done {a_end=i as f64*0.005+0.010;a_done=true;}
                            if !bridged {
                                if let Some((m,burst_peak))=self.burst_end(i+1,edge,stop) {
                                    if m-1>last&&burst_peak>=0.6*peak {bridged=true;last=m-1;b_end=last as f64*0.005+0.010;i=last;continue;}
                                }
                            }
                            break;
                        }
                        last=i+1;i+=1;
                        if !a_done {a_end=last as f64*0.005+0.010;}
                        b_end=last as f64*0.005+0.010;
                    }
                    let gain=|v:f64| if v-edge>=0.040 {v} else {boundary};
                    let walked=gain(b_end).max(gain(a_end));
                    boundary=boundary.max(walked);
                }
            }
        }
        round_ms(if start{(boundary-0.015).max(lo)}else{(boundary+0.020).min(hi)})
    }
    fn burst_end(&self,start:usize,edge:f64,stop:usize)->Option<(usize,f64)> {
        let mut peak=0.0f64; let mut low=0usize;
        for j in start..stop {
            let t=j as f64*0.005+0.005;
            if t>edge+0.900 {return None;}
            peak=peak.max(self.rms[j]);
            if peak>0.0&&self.rms[j]<peak*0.15 {
                low+=1;
                if low>=3 {return Some((j-2,peak));}
            } else {low=0;}
        }
        None
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
    #[test]
    fn sustained_shout_tail_is_not_cropped() {
        // Burst 0.50-0.90, decaying tail 0.90-1.00, short dip, then a loud sustained
        // shout 1.05-1.60 fading out: variant B must bridge the dip and keep the
        // shout instead of stopping at the first rise.
        let mut audio=vec![0.0;48000];
        let burst=|a:&mut [f32],from:usize,to:usize,amp:f64|{for (i,x) in a[from..to].iter_mut().enumerate(){*x=(amp*((i as f64)*0.7).sin()) as f32;}};
        burst(&mut audio,8000,14400,0.08);
        for i in 14400..16000 {audio[i]=(0.05*(1.0-(i-14400) as f64/1600.0)*((i as f64)*0.5).sin()) as f32;}
        for i in 16000..16800 {audio[i]=(0.006*((i as f64)*0.5).sin()) as f32;}
        burst(&mut audio,16800,25600,0.12);
        for i in 25600..28800 {audio[i]=(0.12*(1.0-(i-25600) as f64/3200.0)*((i as f64)*0.5).sin()) as f32;}
        let env=Envelope::new(&audio);
        let word=TimedWord{word:"Gryffindor!".into(),start:0.50,end:0.90,score:0.32};
        let end=env.expand(&word,false,0.0,3.0);
        assert!(end>1.55,"shout tail cropped at {end}");
        assert!(end<1.95,"tail overran into silence: {end}");
    }
}
