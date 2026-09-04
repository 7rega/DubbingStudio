//! Мини WAV I/O для сборки таймлайна дубляжа (assemble.timeline): чтение WAV в mono f32 + запись
//! mono f32. Higgs отдаёт PCM f32 через audiocpp::encode_wav (PCM16 WAV) — hound читает оба формата,
//! стерео сводим в mono усреднением (как s.mean(axis=1) в питоне).

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::io::Read;
use std::path::Path;
use std::process::Stdio;

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

#[cfg(windows)]
const FFMPEG: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const FFMPEG: &str = "ffmpeg";

fn stream_pcm16le(video: &Path, mut visit: impl FnMut(i16)) -> Result<(), ()> {
    let mut child = crate::media::cmd_silent(FFMPEG)
        .args(["-v", "quiet", "-i"])
        .arg(video)
        .args(["-ac", "1", "-ar", "8000", "-f", "s16le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let mut stdout = child.stdout.take().ok_or(())?;
    let mut buf = [0u8; 32 * 1024];
    let mut carry = None;
    loop {
        let read = stdout.read(&mut buf).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        let mut bytes = &buf[..read];
        if let Some(lo) = carry.take() {
            let (&hi, rest) = bytes.split_first().ok_or(())?;
            visit(i16::from_le_bytes([lo, hi]));
            bytes = rest;
        }
        for sample in bytes.chunks_exact(2) {
            visit(i16::from_le_bytes([sample[0], sample[1]]));
        }
        carry = bytes.chunks_exact(2).remainder().first().copied();
    }
    drop(stdout);
    if child.wait().map_err(|_| ())?.success() {
        Ok(())
    } else {
        Err(())
    }
}

/// Даунсэмпл-пики аудио для WaveformTimeline. Декодирует PCM двумя потоковыми проходами: первый
/// определяет амплитуду и длину, второй заполняет пики. В памяти остаётся только N бакетов.
pub fn waveform_peaks(video: &Path, n: usize) -> Vec<f64> {
    let mut sample_count = 0usize;
    let mut max_amp = 1.0f32;
    if stream_pcm16le(video, |sample| {
        sample_count += 1;
        max_amp = max_amp.max((sample as f32).abs());
    })
    .is_err()
        || sample_count == 0
    {
        return Vec::new();
    }

    let buckets = n.min(sample_count).max(1);
    let mut maxima = vec![0.0f32; buckets];
    let mut sample = 0usize;
    if stream_pcm16le(video, |pcm| {
        let bucket = sample * buckets / sample_count;
        maxima[bucket] = maxima[bucket].max((pcm as f32).abs());
        sample += 1;
    })
    .is_err()
    {
        return Vec::new();
    }
    if sample != sample_count {
        return Vec::new();
    }

    maxima
        .into_iter()
        .map(|peak| ((peak / max_amp) as f64 * 1000.0).round() / 1000.0)
        .collect()
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

