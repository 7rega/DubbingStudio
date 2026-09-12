//! Акустический анализ фактуры и зрелости голоса (HNR, Spectral Tilt).
//!
//! Не требует сторонних зависимостей: чистая математика (FFT + нормализованная автокорреляция).
//! Анализ проводится ТОЛЬКО на voiced-фреймах (участках с реальной вибрацией связок),
//! исключая паузы, дыхание, фоновые шумы и глухие согласные.

/// Акустический профиль фактуры голоса.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoiceTexture {
    /// Гармоники к шуму в дБ (Harmonics-to-Noise Ratio).
    /// >12 дБ — чистый звонкий диктор; 7..12 дБ — нормальный разговорный; <7 дБ — хриплый, с песком, прокуренный.
    pub hnr: f32,
    /// Спектральный наклон в дБ (отношение энергии <1000 Гц к энергии >=1000 Гц).
    /// >12 дБ — глубокий, глухой, тяжелый бас зрелого/пожилого; <6 дБ — яркий, звонкий молодой голос.
    pub spectral_tilt: f32,
}

impl Default for VoiceTexture {
    fn default() -> Self {
        Self {
            hnr: 10.0,
            spectral_tilt: 8.0,
        }
    }
}

/// Анализ фактуры голоса (HNR и Spectral Tilt) по mono-сэмплам [-1, 1].
pub fn analyze_voice_texture(samples: &[f32], sr: u32) -> Option<VoiceTexture> {
    if sr == 0 || samples.is_empty() {
        return None;
    }

    let sr_f = sr as f64;
    let frame_len = (sr_f * 0.040).round() as usize; // 40мс
    let hop = (sr_f * 0.020).round() as usize;       // 20мс
    let min_lag = (sr_f / 400.0).floor() as usize;    // макс 400 Гц
    let max_lag = (sr_f / 60.0).ceil() as usize;      // мин 60 Гц

    if frame_len < 4 || min_lag < 1 || max_lag >= frame_len || samples.len() < frame_len {
        return None;
    }

    // Ближайшая степень двойки для FFT >= frame_len
    let mut fft_size = 512;
    while fft_size < frame_len {
        fft_size <<= 1;
    }

    // Предрасчёт окна Ханна
    let hann_window: Vec<f32> = (0..frame_len)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (frame_len - 1) as f32).cos()))
        .collect();

    let mut hnrs: Vec<f32> = Vec::new();
    let mut tilts: Vec<f32> = Vec::new();

    let mut start = 0;
    while start + frame_len <= samples.len() {
        let win = &samples[start..start + frame_len];
        start += hop.max(1);

        // 1. RMS-гейт тишины
        let r0: f64 = win.iter().map(|&x| (x as f64) * (x as f64)).sum();
        let rms = (r0 / frame_len as f64).sqrt();
        if rms < 0.01 || r0 <= 0.0 {
            continue;
        }

        // 2. Нормализованная автокорреляция для поиска пика периодичности (voiced-детектор)
        let hi = max_lag.min(frame_len - 1);
        let mut best_r = 0.0f64;

        for lag in min_lag..=hi {
            let mut num = 0.0f64;
            let mut e1 = 0.0f64;
            let mut e2 = 0.0f64;
            for i in lag..frame_len {
                let a = win[i] as f64;
                let b = win[i - lag] as f64;
                num += a * b;
                e1 += a * a;
                e2 += b * b;
            }
            let denom = (e1 * e2).sqrt();
            if denom > 0.0 {
                let r = num / denom;
                if r > best_r {
                    best_r = r;
                }
            }
        }

        // Если автокорреляция ниже 0.50 — это шум, вздох или глухой звук (не voiced)
        if best_r < 0.50 {
            continue;
        }

        // 3. Вычисляем HNR фрейма по формуле Бурсмы: 10 * log10(r / (1 - r))
        let r_clamped = (best_r as f32).min(0.999);
        let hnr_frame = 10.0 * (r_clamped / (1.0 - r_clamped).max(1e-5)).log10();
        hnrs.push(hnr_frame.clamp(0.0, 30.0));

        // 4. Вычисляем Spectral Tilt через FFT
        let mut re = vec![0.0f32; fft_size];
        let mut im = vec![0.0f32; fft_size];
        for i in 0..frame_len {
            re[i] = win[i] * hann_window[i];
        }

        fft_inplace(&mut re, &mut im);

        let half = fft_size / 2;
        let bin_hz = sr as f32 / fft_size as f32;
        let mut e_low = 0.0f32;
        let mut e_high = 0.0f32;

        for k in 0..=half {
            let freq = k as f32 * bin_hz;
            let power = re[k] * re[k] + im[k] * im[k];
            if freq < 1000.0 {
                e_low += power;
            } else {
                e_high += power;
            }
        }

        let tilt_frame = 10.0 * ((e_low + 1e-6) / (e_high + 1e-6)).log10();
        tilts.push(tilt_frame.clamp(-10.0, 35.0));
    }

    if hnrs.is_empty() || tilts.is_empty() {
        return None;
    }

    Some(VoiceTexture {
        hnr: median_f32(&mut hnrs),
        spectral_tilt: median_f32(&mut tilts),
    })
}

fn median_f32(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) * 0.5
    }
}

/// Радикс-2 FFT (итеративный in-place, O(N log N)).
fn fft_inplace(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Cooley-Tukey butterflies
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wl_re, wl_im) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut w_re, mut w_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let a = i + k;
                let b = i + k + len / 2;
                let t_re = re[b] * w_re - im[b] * w_im;
                let t_im = re[b] * w_im + im[b] * w_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let nw_re = w_re * wl_re - w_im * wl_im;
                let nw_im = w_re * wl_im + w_im * wl_re;
                w_re = nw_re;
                w_im = nw_im;
            }
            i += len;
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_sine_has_high_hnr() {
        let sr = 16_000;
        let duration_secs = 0.5;
        let n = (sr as f32 * duration_secs) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 150.0 * i as f32 / sr as f32).sin())
            .collect();

        let tex = analyze_voice_texture(&samples, sr).expect("voiced sine must produce texture");
        assert!(tex.hnr > 12.0, "Pure sine should have very high HNR, got {:.1}", tex.hnr);
        assert!(tex.spectral_tilt > 10.0, "150 Hz tone has energy <1000 Hz, tilt should be high, got {:.1}", tex.spectral_tilt);
    }

    #[test]
    fn test_silence_returns_none() {
        let sr = 16_000;
        let samples = vec![0.0f32; 16_000];
        assert!(analyze_voice_texture(&samples, sr).is_none());
    }

    #[test]
    fn test_high_tone_has_lower_tilt() {
        let sr = 16_000;
        let n = 8000;
        let bass: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                0.6 * (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
            })
            .collect();

        let bright: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                0.3 * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 1200.0 * t).sin()
                    + 0.4 * (2.0 * std::f32::consts::PI * 2400.0 * t).sin()
            })
            .collect();

        let tex_bass = analyze_voice_texture(&bass, sr).unwrap();
        let tex_bright = analyze_voice_texture(&bright, sr).unwrap();

        assert!(tex_bass.spectral_tilt > tex_bright.spectral_tilt,
            "Bass tilt ({:.1}) should be significantly higher than bright tilt ({:.1})",
            tex_bass.spectral_tilt, tex_bright.spectral_tilt
        );
    }
}
