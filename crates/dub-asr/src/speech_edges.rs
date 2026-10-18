//! Local, bounded waveform expansion, matching experiment2 (10/5 ms RMS).
use crate::forced::{round_ms, TimedWord};
pub struct Envelope { rms:Vec<f64> }
impl Envelope {
    pub fn new(samples:&[f32])->Self {
        Self {rms:samples.windows(160).step_by(80).map(|f|(f.iter().map(|x|(*x as f64).powi(2)).sum::<f64>()/160.0).sqrt()).collect()}
    }
    /// Max RMS over [a,b] seconds (frame time = i*0.005+0.005). Empty range -> 0.
    pub fn max_rms(&self,a:f64,b:f64)->f64 {
        if !(b>a) {return 0.0;}
        let i0=(((a-0.005)/0.005).ceil().max(0.0)) as usize;
        let mut m=0.0f64;
        for i in i0..self.rms.len() {
            let t=i as f64*0.005+0.005;
            if t>b {break;}
            m=m.max(self.rms[i]);
        }
        m
    }
    /// If a word begins inside dead silence (e.g. Qwen3 snapping to snippet sample 0),
    /// advance start forward to acoustic onset, leaving a 20ms lead-in margin.
    pub fn trim_start_silence(&self, start: f64, end: f64) -> f64 {
        if self.rms.is_empty() || !(end > start) {
            return start;
        }
        let i0 = (((start - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());
        let i_limit = (((end - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());
        let mut peak = 0.0f64;
        for i in i0..i_limit {
            peak = peak.max(self.rms[i]);
        }
        let floor = (0.0020f64).max(peak * 0.08);
        let mut cur = i0;
        while cur < i_limit && self.rms[cur] < floor {
            cur += 1;
        }
        // Only trim if there is significant leading silence (at least 80 ms = 16 frames)
        if cur.saturating_sub(i0) >= 16 {
            let onset = (cur as f64 * 0.005 + 0.005 - 0.020).max(start);
            round_ms(onset)
        } else {
            start
        }
    }
    /// If the final word ends inside trailing ambient noise/silence (e.g. Qwen3 overshooting into room tone),
    /// pull back to acoustic offset, leaving a 30ms margin.
    pub fn trim_end_silence(&self, start: f64, end: f64) -> f64 {
        if self.rms.is_empty() || !(end > start) {
            return end;
        }
        let i_end = (((end - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len().saturating_sub(1));
        let i_start = (((start - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len().saturating_sub(1));

        let mut peak = 0.0f64;
        for i in i_start..=i_end {
            peak = peak.max(self.rms[i]);
        }
        let floor = (0.0020f64).max(peak * 0.08);

        let mut cur = i_end;
        while cur > i_start && self.rms[cur] < floor {
            cur -= 1;
        }

        // Only trim if trailing ambient noise/silence is at least 120 ms (24 frames)
        if i_end.saturating_sub(cur) >= 24 {
            let offset = (cur as f64 * 0.005 + 0.005 + 0.030).min(end).max(start);
            round_ms(offset)
        } else {
            end
        }
    }
    /// Controlled forward expansion for sustained vowels / shouts on the final word.
    /// Only expands if acoustic energy is actively vibrating at the word boundary (RMS >= min_vibrate),
    /// decays along the vowel envelope above speech floor (>= floor), and is capped at +250ms.
    pub fn expand_tail(&self, w: &TimedWord, hi: f64) -> f64 {
        if self.rms.is_empty() || !(hi > w.end) {
            return w.end;
        }
        let edge = w.end;
        let i_edge = (((edge - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());
        let edge_rms = self.rms.get(i_edge).copied().unwrap_or(0.0);

        let i_wstart = (((w.start - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());
        let mut word_peak = edge_rms;
        for i in i_wstart..=i_edge.min(self.rms.len().saturating_sub(1)) {
            word_peak = word_peak.max(self.rms[i]);
        }

        // Only expand if the voice is still actively vibrating / shouting at edge
        let min_vibrate = (0.008f64).max(word_peak * 0.20);
        if edge_rms < min_vibrate {
            return edge;
        }

        let floor = (0.006f64).max(word_peak * 0.15);
        let limit_t = hi.min(edge + 0.250);
        let stop = (((limit_t - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());

        let mut cur = i_edge;
        while cur + 1 < stop && self.rms[cur + 1] >= floor {
            cur += 1;
        }

        let expanded = cur as f64 * 0.005 + 0.005;
        if expanded - edge >= 0.030 {
            round_ms(expanded)
        } else {
            edge
        }
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

    /// Безопасное микро-расширение начала фразы наружу (Zero-Trim & Guarded Expand <= 20мс в тишину, до 180мс при наличии голоса).
    /// Никогда не срезает границу внутрь (zero trim), сохраняя 100% хронометража.
    /// Расширяет влево до 20 мс в свободную тишину (атака согласных),
    /// а при наличии активной вибрации/гортанного звука (RMS >= floor) сканирует влево до 180 мс.
    /// Оставляет защитный буфер >= 30 мс до предыдущей фразы.
    pub fn smart_trim_expand_start(&self, start: f64, end: f64, lo: f64) -> f64 {
        let safety_buffer: f64 = 0.030;
        let gap_left = start - lo;

        if gap_left <= safety_buffer {
            // Фразы идут впритык — блокируем расширение во избежание наложений
            return start;
        }

        let min_bound = (lo + safety_buffer).max(start - 0.180);
        let word_peak = self.max_rms(start, end);
        let floor = (0.0030f64).max(word_peak * 0.10);

        // Базовый безопасный отступ 20мс в тишину (для атаки согласных)
        let mut best_start = (start - 0.020).max(min_bound);

        // Если в свободной паузе перед словом фиксируется голос (рык, выкрик, гортанная атака),
        // продолжаем расширение влево до 180мс, пока RMS выше порога
        if !self.rms.is_empty() {
            let i_cur = (((best_start - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());
            let i_min = (((min_bound - 0.005).max(0.0) / 0.005).round() as usize).min(self.rms.len());

            let mut idx = i_cur;
            while idx > i_min && self.rms[idx - 1] >= floor {
                idx -= 1;
                best_start = (idx as f64 * 0.005 + 0.005).max(min_bound);
            }
        }

        round_ms(best_start)
    }

    /// Безопасное микро-расширение конца фразы наружу (Zero-Trim & Guarded Expand <= 30мс).
    /// Никогда не срезает границу внутрь (zero trim), не отнимая хронометраж у дубляжа.
    /// Расширяет вправо до 30 мс в свободную тишину, оставляя защитный буфер >= 30 мс до следующей фразы.
    pub fn smart_trim_expand_end(&self, _start: f64, end: f64, hi: f64) -> f64 {
        let safety_buffer: f64 = 0.030;
        let max_expand: f64 = 0.030;
        let gap_right = hi - end;

        if gap_right <= safety_buffer {
            // Следующая фраза идет впритык — блокируем расширение
            return end;
        }

        let avail = gap_right - safety_buffer;
        let expand = max_expand.min(avail);
        round_ms((end + expand).min(hi))
    }

    /// Двусторонний «умный» контроль обоих краёв фразы (Guarded Expand: -20мс слева / +30мс справа).
    pub fn smart_adjust_bounds(&self, start: f64, end: f64, lo: f64, hi: f64) -> (f64, f64) {
        let adj_start = self.smart_trim_expand_start(start, end, lo);
        let adj_end = self.smart_trim_expand_end(adj_start, end, hi);
        (adj_start, adj_end)
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
    #[test]
    fn trims_leading_dead_silence() {
        let mut audio = vec![0.0f32; 32000];
        for i in 16000..24000 {
            audio[i] = (0.05 * ((i as f64) * 0.5).sin()) as f32;
        }
        let env = Envelope::new(&audio);
        let trimmed = env.trim_start_silence(0.2, 1.5);
        assert!(trimmed >= 0.95 && trimmed <= 1.05, "expected onset near 1.0s, got {trimmed}");
    }

    #[test]
    fn smart_trim_expand_start_guarded_expand() {
        let audio = vec![0.0f32; 16000];
        let env = Envelope::new(&audio);
        // Free space to the left (lo = 0.0, start = 1.0) -> expands by 20ms to 0.98s
        let expanded = env.smart_trim_expand_start(1.0, 2.0, 0.0);
        assert!((expanded - 0.98).abs() < 0.001, "expected 0.98s, got {expanded}");

        // Tight space (lo = 0.98, start = 1.0, gap = 20ms <= 30ms buffer) -> locked at 1.0s
        let locked = env.smart_trim_expand_start(1.0, 2.0, 0.98);
        assert!((locked - 1.0).abs() < 0.001, "expected locked at 1.0s, got {locked}");
    }

    #[test]
    fn smart_trim_expand_end_guarded_expand() {
        let audio = vec![0.0f32; 16000];
        let env = Envelope::new(&audio);
        // Free space to the right (end = 1.0, hi = 2.0) -> expands by 30ms to 1.03s
        let expanded = env.smart_trim_expand_end(0.5, 1.0, 2.0);
        assert!((expanded - 1.03).abs() < 0.001, "expected 1.03s, got {expanded}");

        // Tight space (end = 1.0, hi = 1.02, gap = 20ms <= 30ms buffer) -> locked at 1.0s
        let locked = env.smart_trim_expand_end(0.5, 1.0, 1.02);
        assert!((locked - 1.0).abs() < 0.001, "expected locked at 1.0s, got {locked}");
    }

    #[test]
    fn smart_trim_expand_start_captures_growl() {
        let mut audio = vec![0.0f32; 32000];
        // Word from 1.0s to 1.5s (samples 16000..24000)
        for i in 16000..24000 {
            audio[i] = (0.10 * ((i as f64) * 0.5).sin()) as f32;
        }
        // Guttural growl from 0.88s to 1.0s (samples 14080..16000)
        for i in 14080..16000 {
            audio[i] = (0.05 * ((i as f64) * 0.5).sin()) as f32;
        }
        let env = Envelope::new(&audio);
        let expanded = env.smart_trim_expand_start(1.0, 1.5, 0.5);
        assert!(expanded <= 0.90, "expected start to expand into growl <= 0.90s, got {expanded}");
        assert!(expanded >= 0.86, "expected start to not overrun growl, got {expanded}");
    }
}
