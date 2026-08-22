//! Мини WAV I/O для сборки таймлайна дубляжа (assemble.timeline): чтение WAV в mono f32 + запись
//! mono f32. Higgs отдаёт PCM f32 через audiocpp::encode_wav (PCM16 WAV) — hound читает оба формата,
//! стерео сводим в mono усреднением (как s.mean(axis=1) в питоне).

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::Path;

/// Прочитать WAV -> (mono f32 сэмплы, sample_rate).
pub fn read_mono_f32(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut r = WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => r
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read f32: {e}"))?,
        SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("read int: {e}"))?
        }
    };
    let mono: Vec<f32> = if ch <= 1 {
        interleaved
    } else {
        interleaved
            .chunks(ch)
            .map(|fr| fr.iter().sum::<f32>() / ch as f32)
            .collect()
    };
    Ok((mono, spec.sample_rate))
}

/// Даунсэмпл-пики аудио для WaveformTimeline. Порт app.py._compute_peaks: ffmpeg -> s16le 8kHz mono,
/// N бакетов, в каждом max(|amp|)/max_amp, округление до 3 знаков. CPU, вне GPU-воркера. Сбой ffmpeg
/// или тишина -> пустой список (как питон — не отравляем кэш).
pub fn waveform_peaks(video: &Path, n: usize) -> Vec<f64> {
    #[cfg(windows)]
    const FFMPEG: &str = "ffmpeg.exe";
    #[cfg(not(windows))]
    const FFMPEG: &str = "ffmpeg";
    let out = crate::media::cmd_silent(FFMPEG)
        .args(["-v", "quiet", "-i"])
        .arg(video)
        .args(["-ac", "1", "-ar", "8000", "-f", "s16le", "-"])
        .output();
    let bytes = match out {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    // s16le -> i16 сэмплы.
    let samples: Vec<f32> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32)
        .collect();
    if samples.is_empty() {
        return Vec::new();
    }
    let max_amp = samples.iter().fold(1.0f32, |m, &s| m.max(s.abs()));
    let nb = n.min(samples.len()).max(1);
    // np.array_split: первые (len % nb) бакетов на 1 длиннее.
    let base = samples.len() / nb;
    let rem = samples.len() % nb;
    let mut peaks = Vec::with_capacity(nb);
    let mut i = 0;
    for b in 0..nb {
        let len = base + if b < rem { 1 } else { 0 };
        let slice = &samples[i..i + len];
        i += len;
        let mx = slice.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        peaks.push(((mx / max_amp) as f64 * 1000.0).round() / 1000.0);
    }
    peaks
}

/// Записать mono f32 -> WAV IEEE-float32.
pub fn write_mono_f32(path: &Path, data: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut w = WavWriter::create(path, spec).map_err(|e| format!("create {}: {e}", path.display()))?;
    for &s in data {
        w.write_sample(s).map_err(|e| format!("write: {e}"))?;
    }
    w.finalize().map_err(|e| format!("finalize: {e}"))?;
    Ok(())
}

/// Прочитать WAV -> (деинтерливнутые каналы [Vec<f32>; channels], sample_rate).
/// Поддерживает mono, stereo и multi-channel в 16/24/32-bit PCM и IEEE 32-bit float.
pub fn read_audio_f32(path: &Path) -> Result<(Vec<Vec<f32>>, u32), String> {
    let mut r = WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => r
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read f32: {e}"))?,
        SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("read int: {e}"))?
        }
    };
    if ch == 1 {
        Ok((vec![interleaved], spec.sample_rate))
    } else {
        let n_frames = interleaved.len() / ch;
        let mut channels = vec![Vec::with_capacity(n_frames); ch];
        for frame in interleaved.chunks(ch) {
            for (c, &s) in frame.iter().enumerate() {
                channels[c].push(s);
            }
        }
        Ok((channels, spec.sample_rate))
    }
}

/// Записать multi-channel f32 -> WAV IEEE-float32 (интерлив).
pub fn write_audio_f32(path: &Path, channels: &[Vec<f32>], sample_rate: u32) -> Result<(), String> {
    if channels.is_empty() {
        return Err("no channels to write".into());
    }
    let n_channels = channels.len() as u16;
    let n_frames = channels[0].len();
    let spec = WavSpec {
        channels: n_channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut w = WavWriter::create(path, spec).map_err(|e| format!("create {}: {e}", path.display()))?;
    for i in 0..n_frames {
        for ch in channels {
            let s = ch.get(i).copied().unwrap_or(0.0);
            w.write_sample(s).map_err(|e| format!("write: {e}"))?;
        }
    }
    w.finalize().map_err(|e| format!("finalize: {e}"))?;
    Ok(())
}

