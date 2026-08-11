//! Мини WAV I/O (через hound): чтение в интерливнутый f32 и запись f32. Движок BSRoformer выдаёт
//! WAV IEEE-float32; входной микс из ffmpeg может быть pcm16 или float — hound читает оба формата.

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

/// Интерливнутый PCM во float32 + метаданные.
pub struct Audio {
    pub sample_rate: u32,
    pub channels: u16,
    pub data: Vec<f32>,
}

/// Прочитать WAV в интерливнутый f32 (нормализуя int-семплы в [-1,1]).
pub fn read_f32(path: &std::path::Path) -> Result<Audio, String> {
    let mut r = WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = r.spec();
    let data: Vec<f32> = match spec.sample_format {
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
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        data,
    })
}

/// Записать интерливнутый f32 в WAV IEEE-float32 (тот же формат, что отдаёт движок).
pub fn write_f32(path: &std::path::Path, a: &Audio) -> Result<(), String> {
    let spec = WavSpec {
        channels: a.channels.max(1),
        sample_rate: a.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut w = WavWriter::create(path, spec).map_err(|e| format!("create {}: {e}", path.display()))?;
    for &s in &a.data {
        w.write_sample(s).map_err(|e| format!("write: {e}"))?;
    }
    w.finalize().map_err(|e| format!("finalize: {e}"))?;
    Ok(())
}

/// Метаданные WAV без чтения данных: (sample_rate, channels, длительность в СЕКУНДАХ).
pub fn probe(path: &std::path::Path) -> Result<(u32, u16, f64), String> {
    let r = WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = r.spec();
    let dur = r.duration() as f64 / spec.sample_rate as f64; // duration() = фреймы (пер-канал)
    Ok((spec.sample_rate, spec.channels, dur))
}

/// Прочитать ДИАПАЗОН фреймов [from_frame, from_frame+n_frames) в интерливнутый f32 — окно длинного
/// файла без загрузки целиком (4ч 44.1к stereo f32 = ~5ГБ, целиком нельзя). hound::seek — по фреймам.
pub fn read_f32_range(path: &std::path::Path, from_frame: u32, n_frames: u32) -> Result<Audio, String> {
    let mut r = WavReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let spec = r.spec();
    r.seek(from_frame).map_err(|e| format!("seek {from_frame}: {e}"))?;
    let want = n_frames as usize * spec.channels as usize;
    let data: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => r
            .samples::<f32>()
            .take(want)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read f32: {e}"))?,
        SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .take(want)
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("read int: {e}"))?
        }
    };
    Ok(Audio { sample_rate: spec.sample_rate, channels: spec.channels, data })
}

/// Потоковый писатель WAV f32: держит файл открытым, дописывает чанки — итоговые stems длинного
/// файла собираются без буферизации всего трека в RAM.
pub struct StreamWriter {
    inner: WavWriter<std::io::BufWriter<std::fs::File>>,
}

impl StreamWriter {
    pub fn create(path: &std::path::Path, sample_rate: u32, channels: u16) -> Result<Self, String> {
        let spec = WavSpec { channels: channels.max(1), sample_rate, bits_per_sample: 32, sample_format: SampleFormat::Float };
        Ok(Self { inner: WavWriter::create(path, spec).map_err(|e| format!("create {}: {e}", path.display()))? })
    }
    pub fn write(&mut self, chunk: &[f32]) -> Result<(), String> {
        for &s in chunk {
            self.inner.write_sample(s).map_err(|e| format!("write: {e}"))?;
        }
        Ok(())
    }
    pub fn finalize(self) -> Result<(), String> {
        self.inner.finalize().map_err(|e| format!("finalize: {e}"))
    }
}
