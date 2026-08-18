//! Спектральная инверсная маска для формирования инструментала без фантомного вокала.
//!
//! Формула (режим Spectral Mask):
//!   gamma = 1.015
//!   S_mix = STFT(mix)
//!   S_voc = STFT(vocals)
//!   mask  = max(0, 1 - gamma * |S_voc| / max(|S_mix|, eps))
//!   S_inst = S_mix * mask (с сохранением оригинальной фазы S_mix)
//!   instrumental = iSTFT(S_inst) с нормализацией OLA по w^2.

use realfft::RealFftPlanner;
use std::f32::consts::PI;
use crate::wav::Audio;

pub const DEFAULT_GAMMA: f32 = 1.015;
pub const N_FFT: usize = 2048;
pub const HOP_LENGTH: usize = 441;
pub const WIN_LENGTH: usize = 2048;

/// Сгенерировать периодическое окно Ханна длины `size`.
pub fn hann_window(size: usize) -> Vec<f32> {
    let mut w = vec![0.0f32; size];
    for (i, v) in w.iter_mut().enumerate() {
        *v = 0.5 * (1.0 - (2.0 * PI * i as f32 / size as f32).cos());
    }
    w
}

/// Reflect-паддинг одномерного сигнала слева и справа на `pad` сэмплов.
fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = vec![0.0f32; n + 2 * pad];
    if n == 0 {
        return out;
    }
    if n == 1 {
        out.fill(x[0]);
        return out;
    }
    // Left reflect: x[pad - i]
    for i in 0..pad {
        let src = if i < pad && pad - i < n { pad - i } else { 1 };
        out[i] = x[src.min(n - 1)];
    }
    // Center
    out[pad..pad + n].copy_from_slice(x);
    // Right reflect: x[n - 2 - i]
    for i in 0..pad {
        let src = if n >= 2 + i { n - 2 - i } else { 0 };
        out[pad + n + i] = x[src.min(n - 1)];
    }
    out
}

/// Выполнить спектральное маскирование для одного аудиоканала.
fn spectral_invert_mask_channel(
    mix: &[f32],
    voc: &[f32],
    gamma: f32,
    planner: &mut RealFftPlanner<f32>,
) -> Vec<f32> {
    let n_samples = mix.len().min(voc.len());
    if n_samples == 0 {
        return Vec::new();
    }

    let pad = N_FFT / 2;
    let padded_mix = reflect_pad(&mix[..n_samples], pad);
    let padded_voc = reflect_pad(&voc[..n_samples], pad);
    let padded_len = padded_mix.len();

    let n_frames = if padded_len >= N_FFT {
        1 + (padded_len - N_FFT) / HOP_LENGTH
    } else {
        0
    };

    if n_frames == 0 {
        // Fallback: простое вычитание, если файл короче одного фрейма STFT
        return mix.iter().zip(voc.iter()).map(|(m, v)| *m - *v).collect();
    }

    let r2c = planner.plan_fft_forward(N_FFT);
    let c2r = planner.plan_fft_inverse(N_FFT);

    let window = hann_window(WIN_LENGTH);
    let mut frame_mix = vec![0.0f32; N_FFT];
    let mut frame_voc = vec![0.0f32; N_FFT];
    let mut frame_out = vec![0.0f32; N_FFT];

    let n_bins = N_FFT / 2 + 1;
    let mut spec_mix = vec![realfft::num_complex::Complex::<f32>::new(0.0, 0.0); n_bins];
    let mut spec_voc = vec![realfft::num_complex::Complex::<f32>::new(0.0, 0.0); n_bins];
    let mut spec_inst = vec![realfft::num_complex::Complex::<f32>::new(0.0, 0.0); n_bins];

    let mut scratch_fwd = vec![realfft::num_complex::Complex::<f32>::new(0.0, 0.0); r2c.get_scratch_len()];
    let mut scratch_inv = vec![realfft::num_complex::Complex::<f32>::new(0.0, 0.0); c2r.get_scratch_len()];

    let expected_len = N_FFT + (n_frames - 1) * HOP_LENGTH;
    let mut out_accum = vec![0.0f32; expected_len];
    let mut w_sum = vec![0.0f32; expected_len];

    let inv_n = 1.0f32 / N_FFT as f32;
    let eps = 1e-12f32;

    for f in 0..n_frames {
        let start = f * HOP_LENGTH;

        // Взвешивание окном
        for i in 0..N_FFT {
            frame_mix[i] = padded_mix[start + i] * window[i];
            frame_voc[i] = padded_voc[start + i] * window[i];
        }

        // RFFT
        let _ = r2c.process_with_scratch(&mut frame_mix, &mut spec_mix, &mut scratch_fwd);
        let _ = r2c.process_with_scratch(&mut frame_voc, &mut spec_voc, &mut scratch_fwd);

        // Расчёт спектральной маски: mask = max(0, 1 - gamma * |S_voc| / max(|S_mix|, eps))
        for k in 0..n_bins {
            let sm = spec_mix[k];
            let sv = spec_voc[k];

            let mag_m = (sm.re * sm.re + sm.im * sm.im).sqrt();
            let mag_v = (sv.re * sv.re + sv.im * sv.im).sqrt();

            let denom = if mag_m > eps { mag_m } else { eps };
            let gain = (1.0 - gamma * (mag_v / denom)).max(0.0);

            // S_inst = S_mix * gain (сохранение оригинальной фазы S_mix)
            spec_inst[k] = sm * gain;
        }

        // IRFFT
        let _ = c2r.process_with_scratch(&mut spec_inst, &mut frame_out, &mut scratch_inv);

        // Масштабирование IFFT (1/N) и Overlap-Add с окном синтеза
        for i in 0..N_FFT {
            let s = frame_out[i] * inv_n;
            out_accum[start + i] += s * window[i];
            w_sum[start + i] += window[i] * window[i];
        }
    }

    // Нормализация по сумме квадратов окон (w^2)
    for i in 0..expected_len {
        if w_sum[i] > 1e-8 {
            out_accum[i] /= w_sum[i];
        }
    }

    // Извлечение исходного фрагмента без center-padding
    let mut res = vec![0.0f32; n_samples];
    for (i, dst) in res.iter_mut().enumerate() {
        if pad + i < expected_len {
            *dst = out_accum[pad + i];
        }
    }

    res
}

/// Реконструкция инструментала через спектральную инверсную маску (для всех каналов).
pub fn spectral_invert_mask(mix: &Audio, voc: &Audio, gamma: f32) -> Audio {
    let ch = mix.channels.max(1) as usize;
    let n_mix = mix.data.len();
    let n_voc = voc.data.len();
    let total_samples = n_mix.min(n_voc);
    let n_frames_per_ch = total_samples / ch;

    let mut planner = RealFftPlanner::<f32>::new();
    let mut ch_outs = Vec::with_capacity(ch);

    for c in 0..ch {
        // Деинтерливинг канала c
        let mut ch_mix = Vec::with_capacity(n_frames_per_ch);
        let mut ch_voc = Vec::with_capacity(n_frames_per_ch);
        for i in 0..n_frames_per_ch {
            ch_mix.push(mix.data[i * ch + c]);
            ch_voc.push(voc.data[i * ch + c]);
        }

        let ch_res = spectral_invert_mask_channel(&ch_mix, &ch_voc, gamma, &mut planner);
        ch_outs.push(ch_res);
    }

    // Интерливинг обратно в Audio
    let mut interleaved = Vec::with_capacity(n_frames_per_ch * ch);
    for i in 0..n_frames_per_ch {
        for c in 0..ch {
            let s = if i < ch_outs[c].len() { ch_outs[c][i] } else { 0.0 };
            interleaved.push(s);
        }
    }

    Audio {
        sample_rate: mix.sample_rate,
        channels: mix.channels,
        data: interleaved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hann_window_symmetry() {
        let w = hann_window(2048);
        assert_eq!(w.len(), 2048);
        assert!((w[0] - 0.0).abs() < 1e-6);
        assert!((w[1024] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_pure_silence_vocal_leaves_mix_intact() {
        let sr = 44100;
        let ch = 2;
        let n = sr as usize * 2; // 2 seconds
        let mut mix_data = Vec::with_capacity(n * ch);
        let voc_data = vec![0.0f32; n * ch]; // vocal is silent

        // 440 Hz sine wave
        for i in 0..n {
            let val = (2.0 * PI * 440.0 * i as f32 / sr as f32).sin() * 0.5;
            mix_data.push(val);
            mix_data.push(val * 0.8);
        }

        let mix = Audio { sample_rate: sr, channels: ch as u16, data: mix_data.clone() };
        let voc = Audio { sample_rate: sr, channels: ch as u16, data: voc_data };

        let inst = spectral_invert_mask(&mix, &voc, 1.015);
        assert_eq!(inst.data.len(), mix.data.len());

        // Check error is minimal (< -40 dB)
        let mut diff_sum = 0.0f32;
        let mut mix_sum = 0.0f32;
        // Skip first and last 2048 samples (edge effects of STFT padding)
        let skip = 2048 * ch;
        for i in skip..(inst.data.len() - skip) {
            diff_sum += (inst.data[i] - mix.data[i]).powi(2);
            mix_sum += mix.data[i].powi(2);
        }
        let snr = 10.0 * (mix_sum / diff_sum.max(1e-12)).log10();
        assert!(snr > 35.0, "SNR for silent vocal should be > 35 dB, got {snr} dB");
    }
}
