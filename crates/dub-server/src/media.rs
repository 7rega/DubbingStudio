//! ffmpeg/ffprobe-обёртки для analyze. Порт нужных кусков dubengine/media.py: probe (длительность,
//! видеопоток, fps, кодек) и extract_audio -> wav 16k mono. Тяжёлого ничего: только вызовы бинарей.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
const FFMPEG: &str = "ffmpeg.exe";
#[cfg(windows)]
const FFPROBE: &str = "ffprobe.exe";
#[cfg(not(windows))]
const FFMPEG: &str = "ffmpeg";
#[cfg(not(windows))]
const FFPROBE: &str = "ffprobe";

pub fn cmd_silent<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Сводка probe: длительность/размер/fps/кодек первого видеопотока. Зеркало api._meta().
#[derive(Debug, Clone, Default)]
pub struct MediaMeta {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub src_codec: String,
}

fn parse_fps(r: &str) -> f64 {
    // r_frame_rate вида "30000/1001".
    let mut it = r.split('/');
    match (it.next(), it.next()) {
        (Some(n), Some(d)) => {
            let n: f64 = n.parse().unwrap_or(0.0);
            let d: f64 = d.parse().unwrap_or(0.0);
            if d != 0.0 {
                n / d
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// ffprobe -show_format -show_streams (json) -> MediaMeta. Ошибка если нет видеопотока/длительности.
pub fn probe(input: &Path) -> Result<MediaMeta, String> {
    let out = cmd_silent(FFPROBE)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(input)
        .output()
        .map_err(|e| format!("ffprobe запуск не удался: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe вернул код {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe json: {e}"))?;
    let streams = v
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or("ffprobe: нет streams")?;
    // Видеопоток ОПЦИОНАЛЕН: чистый аудио-вход (WAV/mp3/…) поддерживается в аудио-режиме (без видео).
    // Нет видео -> width/height/fps=0 (сигнал audio-only), кодек берём из аудиопотока.
    let vstream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("video"));
    let astream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("audio"));
    if vstream.is_none() && astream.is_none() {
        return Err("во входе нет ни видео-, ни аудиопотока".to_string());
    }
    let duration = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            // некоторые WAV не имеют format.duration -> берём из аудиопотока
            astream
                .and_then(|s| s.get("duration"))
                .and_then(|d| d.as_str())
                .and_then(|d| d.parse::<f64>().ok())
        })
        .ok_or("не удалось определить длительность")?;
    let width = vstream.and_then(|s| s.get("width")).and_then(|w| w.as_i64()).unwrap_or(0);
    let height = vstream.and_then(|s| s.get("height")).and_then(|h| h.as_i64()).unwrap_or(0);
    let fps = vstream
        .and_then(|s| s.get("r_frame_rate"))
        .and_then(|r| r.as_str())
        .map(parse_fps)
        .unwrap_or(0.0);
    let src_codec = vstream
        .or(astream)
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Ok(MediaMeta {
        duration,
        width,
        height,
        fps,
        src_codec,
    })
}

/// Извлечь аудиодорожку в WAV 16 кГц mono (pcm_s16le) — вход ASR. Порт media.to_16k_mono/extract_audio
/// (объединённо: сразу 16k/mono, т.к. дальше в порту нет separation-стадии). Если у видео нет аудио —
/// ffmpeg вернёт ошибку, которую пробрасываем.
pub fn extract_wav_16k_mono(input: &Path, out_wav: &Path) -> Result<(), String> {
    let status = cmd_silent(FFMPEG)
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args([
            "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-f", "wav",
        ])
        .arg(out_wav)
        .output()
        .map_err(|e| format!("ffmpeg запуск не удался: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "ffmpeg extract_audio код {:?}: {}",
            status.status.code(),
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    if !out_wav.is_file() {
        return Err("ffmpeg не создал wav".into());
    }
    Ok(())
}

// ─── Рендер-хелперы (порт media.py: extract_audio/duration/time_stretch/mix/mux/trim) ─────────

fn run_ff(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = cmd_silent(FFMPEG)
        .args(args)
        .output()
        .map_err(|e| format!("ffmpeg запуск: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.chars().rev().take(1500).collect::<String>().chars().rev().collect();
        return Err(format!("ffmpeg код {:?}:\n{tail}", out.status.code()));
    }
    Ok(())
}

/// run_ff с ЖЁСТКИМ таймаутом — для дорогих графов (mix_env volume:eval=frame на длинном файле мог
/// зависнуть навечно и заблокировать единственный воркер джоб, как burn #105). Drain-потоки с дедлайном
/// (паттерн dub-captions::burn::output_with_timeout).
fn run_ff_timeout(args: &[&std::ffi::OsStr], secs: u64) -> Result<(), String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd_silent(FFMPEG)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg запуск: {e}"))?;
    let mut se = child.stderr.take().expect("piped stderr");
    let th_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("ffmpeg не завершился за {secs}с — убит (зависание)"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(250)),
            Err(e) => return Err(format!("ffmpeg wait: {e}")),
        }
    };
    if !status.success() {
        let err = th_err.join().unwrap_or_default();
        let s = String::from_utf8_lossy(&err);
        let tail: String = s.chars().rev().take(1500).collect::<String>().chars().rev().collect();
        return Err(format!("ffmpeg код {:?}:\n{tail}", status.code()));
    }
    Ok(())
}

use std::ffi::OsStr;

/// Извлечь аудио в WAV sr/ac (порт media.extract_audio). Для сепарации: sr=44100, ac=2.
pub fn extract_audio(video: &Path, out_wav: &Path, sr: u32, ac: u32) -> Result<(), String> {
    let sr = sr.to_string();
    let ac = ac.to_string();
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), video.as_os_str(),
        OsStr::new("-vn"), OsStr::new("-ac"), OsStr::new(&ac),
        OsStr::new("-ar"), OsStr::new(&sr), out_wav.as_os_str(),
    ])
}

/// WAV/медиа -> 16k mono (порт media.to_16k_mono).
pub fn to_16k_mono(src: &Path, dst: &Path) -> Result<(), String> {
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-vn"), OsStr::new("-ac"), OsStr::new("1"),
        OsStr::new("-ar"), OsStr::new("16000"), dst.as_os_str(),
    ])
}

/// Длительность файла в секундах (ffprobe format.duration). Порт media.duration.
pub fn duration(path: &Path) -> Result<f64, String> {
    let out = cmd_silent(FFPROBE)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe запуск: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffprobe duration код {:?}", out.status.code()));
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|e| format!("duration parse: {e}"))
}

/// atempo-цепочка для factor вне [0.5,2.0] (порт media._atempo_chain).
fn atempo_chain(factor: f64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut f = factor;
    while f > 2.0 {
        parts.push("atempo=2.0".into());
        f /= 2.0;
    }
    while f < 0.5 {
        parts.push("atempo=0.5".into());
        f /= 0.5;
    }
    parts.push(format!("atempo={:.6}", f));
    parts.join(",")
}

/// factor>1 ускоряет (укорачивает); <1 замедляет. Порт media.time_stretch.
pub fn time_stretch(src: &Path, dst: &Path, factor: f64) -> Result<(), String> {
    let chain = atempo_chain(factor);
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-filter:a"), OsStr::new(&chain), dst.as_os_str(),
    ])
}

/// Умное сжатие межсловных пауз (squeeze_internal_pauses):
/// Находит промежутки тишины (< -35 dB) между словами длительностью > min_pause_ms (70 мс)
/// и сжимает их до target_max_pause_ms (40 мс) с применением 5 мс сглаживающего кроссфейда.
/// Не затрагивает сами слова и гласные, сохраняя 100% тембр и натуральность речи.
pub fn squeeze_internal_pauses(
    samples: &[f32],
    sr: u32,
    target_max_pause_ms: f64,
) -> Vec<f32> {
    if samples.is_empty() || sr == 0 {
        return samples.to_vec();
    }
    let win_len = ((sr as f64 * 0.010).round() as usize).max(1); // 10 мс окно
    let n_wins = samples.len() / win_len;
    if n_wins < 3 {
        return samples.to_vec();
    }

    // Порог тишины: -35 dB -> amp ~ 0.01778
    let silence_threshold = 0.01778f32;

    // Рассчитываем RMS для каждого 10 мс окна
    let mut is_speech: Vec<bool> = Vec::with_capacity(n_wins);
    for w in 0..n_wins {
        let start = w * win_len;
        let end = start + win_len;
        let slice = &samples[start..end];
        let sum_sq: f32 = slice.iter().map(|&s| s * s).sum();
        let rms = (sum_sq / win_len as f32).sqrt();
        is_speech.push(rms >= silence_threshold);
    }

    // Находим первое и последнее речевые окна
    let first_speech = match is_speech.iter().position(|&s| s) {
        Some(pos) => pos,
        None => return samples.to_vec(), // всё аудио — тишина, ничего не трогаем
    };
    let last_speech = match is_speech.iter().rposition(|&s| s) {
        Some(pos) => pos,
        None => return samples.to_vec(),
    };

    let min_pause_wins = ((0.070f64 / 0.010f64).round() as usize).max(1); // 70 мс = 7 окон
    let target_pause_samples = ((sr as f64 * target_max_pause_ms / 1000.0).round() as usize).max(1);
    let xfade_samples = ((sr as f64 * 0.005).round() as usize).min(target_pause_samples / 2).max(1); // 5 мс

    let mut out: Vec<f32> = Vec::with_capacity(samples.len());

    // Ведущая тишина (до first_speech) добавляется как есть
    let leading_end = first_speech * win_len;
    out.extend_from_slice(&samples[..leading_end]);

    // Проходим по сегментам от first_speech до last_speech
    let mut cur_win = first_speech;
    while cur_win <= last_speech {
        if is_speech[cur_win] {
            // Накапливаем связный кусок речи
            let spk_start = cur_win * win_len;
            while cur_win <= last_speech && is_speech[cur_win] {
                cur_win += 1;
            }
            let spk_end = if cur_win <= last_speech {
                cur_win * win_len
            } else {
                (last_speech + 1) * win_len
            };
            out.extend_from_slice(&samples[spk_start..spk_end]);
        } else {
            // Внутренняя пауза
            let pause_start_win = cur_win;
            while cur_win <= last_speech && !is_speech[cur_win] {
                cur_win += 1;
            }
            let pause_len_wins = cur_win - pause_start_win;
            let pause_raw_start = pause_start_win * win_len;
            let pause_raw_end = cur_win * win_len;
            let pause_raw_len = pause_raw_end - pause_raw_start;

            if pause_len_wins >= min_pause_wins && pause_raw_len > target_pause_samples + xfade_samples {
                // Сжимаем паузу до target_pause_samples с 5мс кроссфейдом между началом и концом тишины
                let half_target = target_pause_samples / 2;
                let part1_end = pause_raw_start + half_target;
                let part2_start = pause_raw_end.saturating_sub(half_target + xfade_samples);

                // Добавляем первую часть сжатой паузы (за вычетом зоны кроссфейда)
                let p1_clean_end = part1_end.saturating_sub(xfade_samples);
                out.extend_from_slice(&samples[pause_raw_start..p1_clean_end]);

                // Кроссфейд стыка (xfade_samples)
                for i in 0..xfade_samples {
                    let alpha = i as f32 / xfade_samples as f32;
                    let s1 = samples.get(p1_clean_end + i).copied().unwrap_or(0.0);
                    let s2 = samples.get(part2_start + i).copied().unwrap_or(0.0);
                    out.push((1.0 - alpha) * s1 + alpha * s2);
                }

                // Добавляем вторую часть сжатой паузы
                let p2_clean_start = part2_start + xfade_samples;
                if p2_clean_start < pause_raw_end {
                    out.extend_from_slice(&samples[p2_clean_start..pause_raw_end]);
                }
            } else {
                // Короткая пауза (<70 мс) — оставляем без изменений
                out.extend_from_slice(&samples[pause_raw_start..pause_raw_end]);
            }
        }
    }

    // Хвостовая часть (после last_speech) добавляется как есть
    let tail_start = (last_speech + 1) * win_len;
    if tail_start < samples.len() {
        out.extend_from_slice(&samples[tail_start..]);
    }

    out
}

/// Прочитать WAV -> сжать межсловные паузы -> записать в dst_wav -> вернуть (dst_wav, duration_s).
pub fn squeeze_internal_pauses_wav(
    src_wav: &Path,
    dst_wav: &Path,
    target_max_pause_ms: f64,
) -> Result<(PathBuf, f64), String> {
    let (samples, sr) = crate::wavio::read_mono_f32(src_wav)?;
    let squeezed = squeeze_internal_pauses(&samples, sr, target_max_pause_ms);
    crate::wavio::write_mono_f32(dst_wav, &squeezed, sr)?;
    let dur = squeezed.len() as f64 / sr as f64;
    Ok((dst_wav.to_path_buf(), dur))
}

/// Свести дубль-вокал поверх фона. МУЗЫКУ НЕ ГЛУШИМ (прямой приказ юзера, многократно): вокал уже вырезан
/// сепарацией, поэтому инструментал = чистый реальный фон и звучит в ПОЛНЫЙ уровень (1.0) — дублированный
/// голос заменяет вырезанный вокал, фон остаётся как в оригинале. amix normalize=0 НЕ делит входы пополам
/// (питон-дефолт=1 занизил бы фон ~в 0.5). aformat=cl=stereo снимает нестандартный mono layout hound-дубляжа
/// (уровень не меняет). Пики/итоговую громкость держит финальный loudnorm на смиксованной дорожке.
pub fn mix(voice: &Path, music: &Path, out: &Path) -> Result<(), String> {
    let fc = "[0:a]aformat=channel_layouts=stereo[v];\
              [1:a]aformat=channel_layouts=stereo[m];\
              [v][m]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0[a]";
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), voice.as_os_str(), OsStr::new("-i"), music.as_os_str(),
        OsStr::new("-filter_complex"), OsStr::new(fc), OsStr::new("-map"), OsStr::new("[a]"),
        OsStr::new("-c:a"), OsStr::new("aac"), OsStr::new("-b:a"), OsStr::new("192k"), out.as_os_str(),
    ])
}

/// Сведение диалог+фон с САЙДЧЕЙН-ДАКИНГОМ — проф. практика дубляжа: компрессор приглушает фон
/// ТОЛЬКО пока звучит голос (attack 150мс / release 600мс — плавные подныривания), в паузах фон
/// живёт ровно 1:1 — атмосфера НЕ гробится статичным резом (требование юзера: «дубляж громче фона,
/// но фон не гробить»). Замер: фон мультика -17.1 LUFS ≈ уровню голоса — без дакинга речь тонет.
/// threshold 0.02 / ratio 8 ≈ -6..-9 дБ фону под фразой. Порядок sidechaincompress: [фон][ключ-голос].
pub fn mix_ducked(voice: &Path, music: &Path, out: &Path) -> Result<(), String> {
    let fc = "[0:a]aformat=channel_layouts=stereo,asplit=2[v][vkey];\
              [1:a]aformat=channel_layouts=stereo[m];\
              [m][vkey]sidechaincompress=threshold=0.02:ratio=8:attack=150:release=600:makeup=1[bg];\
              [v][bg]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0[a]";
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), voice.as_os_str(), OsStr::new("-i"), music.as_os_str(),
        OsStr::new("-filter_complex"), OsStr::new(fc), OsStr::new("-map"), OsStr::new("[a]"),
        OsStr::new("-c:a"), OsStr::new("aac"), OsStr::new("-b:a"), OsStr::new("192k"), out.as_os_str(),
    ])
}

/// Свести три аудиопотока в один стерео-микс (для профессионального закадра):
/// track1 (дубляж 100%) + track2 (инструментал/музыка 100%) + track3 (приглушенный оригинальный вокал).
#[allow(dead_code)]
pub fn mix3(track1: &Path, track2: &Path, track3: &Path, out: &Path) -> Result<(), String> {
    let fc = "[0:a]aformat=channel_layouts=stereo[a0];\
              [1:a]aformat=channel_layouts=stereo[a1];\
              [2:a]aformat=channel_layouts=stereo[a2];\
              [a0][a1][a2]amix=inputs=3:duration=longest:dropout_transition=0:normalize=0[a]";
    run_ff(&[
        OsStr::new("-y"),
        OsStr::new("-i"), track1.as_os_str(),
        OsStr::new("-i"), track2.as_os_str(),
        OsStr::new("-i"), track3.as_os_str(),
        OsStr::new("-filter_complex"), OsStr::new(fc),
        OsStr::new("-map"), OsStr::new("[a]"),
        OsStr::new("-c:a"), OsStr::new("aac"),
        OsStr::new("-b:a"), OsStr::new("256k"),
        out.as_os_str(),
    ])
}

/// Наложить детерминированную огибающую дакинга на аудиофайл (напр. изолированный вокал):
/// 1.0 в паузах, 10^(duck_db/20) во время речевых блоков с плавными рампами.
#[allow(dead_code)]
pub fn duck_envelope_file(src_audio: &Path, blocks: &[SpeechBlock], duck_db: f64, out: &Path) -> Result<(), String> {
    let (channels, sr) = crate::wavio::read_audio_f32(src_audio)?;
    if channels.is_empty() {
        return Err("no audio channels".into());
    }
    let n_frames = channels[0].len();
    let params = DuckEnvelopeParams {
        duck_db,
        ..DuckEnvelopeParams::default()
    };
    let envelope = generate_duck_envelope(n_frames, sr, blocks, &params);
    let mut ducked = channels.clone();
    for ch in ducked.iter_mut() {
        for (i, sample) in ch.iter_mut().enumerate() {
            *sample *= envelope[i];
        }
    }
    crate::wavio::write_audio_f32(out, &ducked, sr)
}

/// Акустическое согласование пространства (Scene Spatial Reverb):
/// Насыщает сухой студийный голос тонкими ранними отражениями (Early Reflections, decay ~0.18с, wet -22 dB).
/// Устраняет эффект «голоса из радиобудки» и естественно сажает диктора в видеоряд.
pub fn apply_spatial_reverb(src_wav: &Path, dst_wav: &Path) -> Result<(), String> {
    let af = "aformat=channel_layouts=stereo,aecho=0.92:0.88:20|42:0.18|0.12";
    run_ff(&[
        OsStr::new("-y"),
        OsStr::new("-i"), src_wav.as_os_str(),
        OsStr::new("-af"), OsStr::new(af),
        dst_wav.as_os_str(),
    ])
}

/// Гибридный 3-трековый микшинг для профессионального UN Voice-Over:
/// Адаптирован под архитектуру sample-accurate ducking: vocal используется как sidechain/control,
/// master audio остаётся в audio_hq, сведение выполняется в float32 без промежуточного AAC.
#[allow(dead_code)]
pub fn mix_voiceover_hybrid(
    voice: &Path,
    audio_hq: &Path,
    _vocals: &Path,
    blocks: &[SpeechBlock],
    _inst_duck_db: f64,
    voc_duck_db: f64,
    out: &Path,
) -> Result<(), String> {
    mix_voiceover_file(voice, audio_hq, blocks, voc_duck_db, out, "speech_aware").map(|_| ())
}

pub use crate::duck::{
    apply_peak_control, generate_duck_envelope, mix_voiceover_sample_accurate,
    DiagnosticMetrics, DuckEnvelopeParams, SpeechBlock,
};

/// Высококачественный ресемплинг аудио через FFmpeg soxr (32-bit float):
/// aresample=target_sr:resampler=soxr:precision=28
pub fn resample_soxr(src: &Path, dst: &Path, target_sr: u32) -> Result<(), String> {
    let target_sr_s = target_sr.to_string();
    let af = format!("aresample={target_sr_s}:resampler=soxr:precision=28");
    run_ff(&[
        OsStr::new("-y"),
        OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-af"), OsStr::new(&af),
        OsStr::new("-c:a"), OsStr::new("pcm_f32le"),
        dst.as_os_str(),
    ])
}

/// Свести Voiceover в 32-bit float WAV с sample-accurate дакингом.
pub fn mix_voiceover_file(
    voice_wav: &Path,
    audio_hq_wav: &Path,
    blocks: &[SpeechBlock],
    duck_db: f64,
    out_wav: &Path,
    duck_mode: &str,
) -> Result<DiagnosticMetrics, String> {
    let (orig_ch, orig_sr) = crate::wavio::read_audio_f32(audio_hq_wav)?;
    let (tts_ch, tts_sr) = crate::wavio::read_audio_f32(voice_wav)?;

    // Если частота дискретизации не совпадает, передискретизируем TTS через soxr
    let tts_resampled_ch = if tts_sr != orig_sr {
        let temp_resampled = out_wav.with_extension("temp_tts_resampled.wav");
        resample_soxr(voice_wav, &temp_resampled, orig_sr)?;
        let (ch, _) = crate::wavio::read_audio_f32(&temp_resampled)?;
        let _ = std::fs::remove_file(&temp_resampled);
        ch
    } else {
        tts_ch
    };

    let params = DuckEnvelopeParams {
        duck_db,
        ..DuckEnvelopeParams::default()
    };

    let (mut mixed, metrics) = mix_voiceover_sample_accurate(
        &orig_ch,
        &tts_resampled_ch,
        orig_sr,
        blocks,
        &params,
        duck_mode,
    )?;

    // Стадия Headroom / True-Peak контроля (-1.0 dBFS ceiling)
    apply_peak_control(&mut mixed, -1.0);

    crate::wavio::write_audio_f32(out_wav, &mixed, orig_sr)?;
    Ok(metrics)
}


// Параметры детерминированной огибающей дакинга.
// ВАЖНО: в ДУБЛЯЖЕ фон = сепарированный инструментал (= M&E, оригинального голоса там НЕТ). Проф.практика
// дубляжа (Netflix/Deepdub M&E): M&E микшируется на ПОЛНОМ уровне с новым дубляжом — его НЕ душат, дубляж
// просто занимает место оригинального диалога. Поэтому дакинг мягкий (−3 дБ, лёгкий провал для разборчивости
// синтет-голоса), фон остаётся слышен. Env DUB_DUCK_DB (dB, 0 = совсем не душить). Прошлые −12 дБ «срезали
// весь фон» (подкаст/закадр-техника, ошибочно на дубляж). [[reference_parakeet_rs_ort_gotchas]] — «фон не глушить».
const DUCK_GAIN: f64 = 0.708; // −3 дБ под речью (дефолт); env DUB_DUCK_DB override
const DUCK_PREROLL: f64 = 0.08; // fade-down стартует за 0.08с ДО блока
const DUCK_FADE_DOWN: f64 = 0.10; // длина спуска
const DUCK_HOLD: f64 = 0.30; // держим приглушение 0.30с ПОСЛЕ конца блока
const DUCK_FADE_UP: f64 = 0.40; // длина подъёма

/// Глубина дакинга фона (линейный gain 0..1) под речью в ДУБЛЯЖЕ. Env DUB_DUCK_DB (дБ; 0 = без дакинга,
/// отрицательное = приглушать). Дефолт DUCK_GAIN (−3 дБ). Клэмп в [0.02, 1.0].
fn duck_gain() -> f64 {
    match std::env::var("DUB_DUCK_DB").ok().and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(db) => 10f64.powf(db / 20.0).clamp(0.02, 1.0),
        None => DUCK_GAIN,
    }
}

/// Собрать выражение gain(t) для ffmpeg-фильтра `volume` из речевых блоков: 1.0 вне блоков, DUCK_GAIN
/// внутри, линейные фейды на краях (down за DUCK_PREROLL до start, up через DUCK_HOLD после end).
/// Детерминированно и с точными dB — компрессор (sidechaincompress) реагировал на мгновенную амплитуду
/// TTS и давал «качели» на микропаузах; здесь огибающая задана таймингами, а не сигналом.
/// Форма: gain(t) = 1 − (1−g)·Σ trap_i(t), где trap_i — трапеция блока i (clip(min(рампа-вниз,
/// рампа-вверх),0,1)). Блоки разведены паузой ≥1.6с > preroll+fade+hold+fade — трапеции не пересекаются,
/// сумма ≤ 1. Длина выражения O(N) по блокам (вложенные if давали O(2^N) — дубль prev в обеих ветках).
fn duck_volume_expr(blocks: &[SpeechBlock], g: f64) -> String {
    if blocks.is_empty() {
        return "1".into();
    }
    // Трапеция блока: (t-ds)/fd растёт 0->1 на спуске, (ue-t)/fu убывает 1->0 на подъёме; между ними
    // обе ≥1 -> clip даёт полку 1 (полное приглушение). Вне [ds,ue] одна из рамп ≤0 -> clip даёт 0.
    let traps: Vec<String> = blocks
        .iter()
        .map(|b| {
            let ds = (b.start - DUCK_PREROLL).max(0.0); // старт спуска
            let us = b.end + DUCK_HOLD; // старт подъёма (после hold)
            let ue = us + DUCK_FADE_UP; // конец подъёма = снова 1.0
            format!(
                "clip(min((t-{ds:.3})/{fd:.3},({ue:.3}-t)/{fu:.3}),0,1)",
                fd = DUCK_FADE_DOWN,
                fu = DUCK_FADE_UP
            )
        })
        .collect();
    // clip суммы в [0,1] (#116, находка [0]): при большом tempo-fit границы блоков делятся, а фейд-константы
    // нет — соседние трапеции могут пересечься, sum>1 дало бы gain<g и даже <0 (инверсия фазы). clip держит
    // gain в [g,1].
    format!("1-{d:.4}*clip({sum},0,1)", d = 1.0 - g, sum = traps.join("+"))
}

/// Сведение диалог+фон с ДЕТЕРМИНИРОВАННОЙ ОГИБАЮЩЕЙ дакинга (#106). Фон приглушается по кусочно-линейной
/// огибающей громкости, построенной из ТОЧНЫХ таймингов речевых блоков (а не по мгновенной амплитуде
/// TTS, как sidechaincompress — тот давал «качели» на микропаузах внутри фраз). volume с выражением от t
/// (eval=frame). На длинных видео сотни блоков -> выражение большое: filtergraph пишем в файл через
/// `-filter_complex_script` (лимит CreateProcess 32767, паттерн из dub-captions/burn.rs). Порядок как в
/// mix: голос полным уровнем + фон по огибающей, amix normalize=0. Пусто блоков -> фон ровно 1.0.
pub fn mix_env(voice: &Path, music: &Path, blocks: &[SpeechBlock], out: &Path) -> Result<(), String> {
    // Дубляж: глубина дакинга фона = duck_gain() (−3 дБ дефолт, env DUB_DUCK_DB).
    mix_env_g(voice, music, blocks, duck_gain(), out)
}

/// Динамический дакинг с ЗАДАННОЙ глубиной (дБ) — для ЗАКАДРА (UN-style voice-over): оригинал звучит
/// ПОЛНЫМ между репликами перевода (слышно исходного спикера/эмоцию) и приглушается на `duck_db` ПОД
/// переводом, восстанавливаясь после. `bed` — весь оригинал, `blocks` — тайминги переведённой речи.
/// Прежде оригинал давился ПЛОСКО на всю дорожку (−12 дБ навсегда, в т.ч. в паузах) — не по практике.
pub fn mix_env_db(voice: &Path, bed: &Path, blocks: &[SpeechBlock], duck_db: f64, out: &Path) -> Result<(), String> {
    let g = 10f64.powf(duck_db / 20.0).clamp(0.02, 1.0);
    mix_env_g(voice, bed, blocks, g, out)
}

fn mix_env_g(voice: &Path, music: &Path, blocks: &[SpeechBlock], g: f64, out: &Path) -> Result<(), String> {
    let vol = duck_volume_expr(blocks, g);
    let fc = format!(
        "[0:a]aformat=channel_layouts=stereo[v];\
         [1:a]aformat=channel_layouts=stereo,volume='{vol}':eval=frame[bg];\
         [v][bg]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0[a]"
    );
    // Таймаут пропорционален длине музыки (eval=frame дорог на многочасовом): max(600с, 2×длит.).
    let secs = (duration(music).unwrap_or(0.0) * 2.0).max(600.0) as u64;
    run_ff_timeout(&[
        OsStr::new("-y"), OsStr::new("-i"), voice.as_os_str(), OsStr::new("-i"), music.as_os_str(),
        OsStr::new("-filter_complex"), OsStr::new(&fc),
        OsStr::new("-map"), OsStr::new("[a]"),
        OsStr::new("-c:a"), OsStr::new("aac"), OsStr::new("-b:a"), OsStr::new("192k"), out.as_os_str(),
    ], secs)
}

/// Финальная нормализация программы по EBU R128 (ffmpeg loudnorm): интегральная громкость к I LUFS
/// + true-peak лимитер к TP dBTP. Решение юзера (best-practice, НЕ питон): ставится последним шагом на
/// смиксованную дорожку — держит целевую громкость соцсетей и ловит пики.
/// Явно фиксируем sample_rates=44100 и -ar 44100 (фильтр loudnorm внутри апсемплит до 192к, без явного
/// флага AAC-энкодер выбирал 96 кГц).
pub fn loudnorm(src: &Path, dst: &Path, i: f64, tp: f64, lra: f64) -> Result<(), String> {
    let af = format!("aformat=channel_layouts=stereo:sample_rates=44100,loudnorm=I={i}:TP={tp}:LRA={lra},aformat=sample_rates=44100");
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-af"), OsStr::new(&af),
        OsStr::new("-ar"), OsStr::new("44100"),
        OsStr::new("-c:a"), OsStr::new("aac"), OsStr::new("-b:a"), OsStr::new("192k"), dst.as_os_str(),
    ])
}

/// Усилить всю дорожку на `gain_db` dB (монтажный гейн, наша opt-in фича). Перекодирование в aac 44.1k.
pub fn gain(src: &Path, dst: &Path, gain_db: f64) -> Result<(), String> {
    let af = format!("aformat=channel_layouts=stereo:sample_rates=44100,volume={gain_db}dB");
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-af"), OsStr::new(&af),
        OsStr::new("-ar"), OsStr::new("44100"),
        OsStr::new("-c:a"), OsStr::new("aac"), OsStr::new("-b:a"), OsStr::new("192k"), dst.as_os_str(),
    ])
}

/// Смуксить видео (copy) + аудио. БЕЗ -shortest (выход по длиннейшему потоку). Порт media.mux.
/// Если входное аудио уже в формате AAC (.m4a), поток копируется без перекодирования (-c:a copy).
pub fn mux(video: &Path, audio: &Path, out: &Path) -> Result<(), String> {
    let is_m4a = audio
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("m4a") || e.eq_ignore_ascii_case("aac"))
        .unwrap_or(false);

    if is_m4a {
        run_ff(&[
            OsStr::new("-y"), OsStr::new("-i"), video.as_os_str(), OsStr::new("-i"), audio.as_os_str(),
            OsStr::new("-map"), OsStr::new("0:v:0"), OsStr::new("-map"), OsStr::new("1:a:0?"),
            OsStr::new("-c:v"), OsStr::new("copy"),
            OsStr::new("-c:a"), OsStr::new("copy"),
            OsStr::new("-movflags"), OsStr::new("+faststart"), out.as_os_str(),
        ])
    } else {
        run_ff(&[
            OsStr::new("-y"), OsStr::new("-i"), video.as_os_str(), OsStr::new("-i"), audio.as_os_str(),
            OsStr::new("-map"), OsStr::new("0:v:0"), OsStr::new("-map"), OsStr::new("1:a:0?"),
            OsStr::new("-af"), OsStr::new("aformat=channel_layouts=stereo:sample_rates=44100"),
            OsStr::new("-ar"), OsStr::new("44100"),
            OsStr::new("-c:v"), OsStr::new("copy"),
            OsStr::new("-c:a"), OsStr::new("aac"),
            OsStr::new("-b:a"), OsStr::new("192k"),
            OsStr::new("-movflags"), OsStr::new("+faststart"), out.as_os_str(),
        ])
    }
}

/// Смуксить видео (copy) + ОРИГИНАЛЬНУЮ аудиодорожку БЕЗ перекодирования (`-c:a copy`). Для режимов, где
/// звук не трогаем (субтитры/транскрипт): сохраняем оригинал байт-в-байт — каналы (5.1/стерео), частоту,
/// битрейт, кодек. НИКАКОГО aformat/downmix (он рушил 6ch→stereo) и никакого переэнкода (терял качество).
pub fn mux_keep_audio(video: &Path, source_with_audio: &Path, out: &Path) -> Result<(), String> {
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), video.as_os_str(), OsStr::new("-i"), source_with_audio.as_os_str(),
        OsStr::new("-map"), OsStr::new("0:v:0"), OsStr::new("-map"), OsStr::new("1:a:0?"),
        OsStr::new("-c:v"), OsStr::new("copy"), OsStr::new("-c:a"), OsStr::new("copy"),
        OsStr::new("-movflags"), OsStr::new("+faststart"), out.as_os_str(),
    ])
}

/// ISO 639-1 (2-буквенный код Whisper/UI) -> ISO 639-2/B (3-буквенный код дорожки контейнера, `language`
/// в metadata:s:a). Покрывает весь WHISPER_LANGS (99 языков) + алиасы. Незнакомый код -> "und" (undefined).
pub fn iso639_1_to_2(code: &str) -> &'static str {
    match code.trim().to_lowercase().as_str() {
        "en" => "eng", "zh" => "zho", "de" => "deu", "es" => "spa", "ru" => "rus",
        "ko" => "kor", "fr" => "fra", "ja" => "jpn", "pt" => "por", "tr" => "tur",
        "pl" => "pol", "ca" => "cat", "nl" => "nld", "ar" => "ara", "sv" => "swe",
        "it" => "ita", "id" => "ind", "hi" => "hin", "fi" => "fin", "vi" => "vie",
        "he" => "heb", "uk" => "ukr", "el" => "ell", "ms" => "msa", "cs" => "ces",
        "ro" => "ron", "da" => "dan", "hu" => "hun", "ta" => "tam", "no" => "nor",
        "th" => "tha", "ur" => "urd", "hr" => "hrv", "bg" => "bul", "lt" => "lit",
        "la" => "lat", "mi" => "mri", "ml" => "mal", "cy" => "cym", "sk" => "slk",
        "te" => "tel", "fa" => "fas", "lv" => "lav", "bn" => "ben", "sr" => "srp",
        "az" => "aze", "sl" => "slv", "kn" => "kan", "et" => "est", "mk" => "mkd",
        "br" => "bre", "eu" => "eus", "is" => "isl", "hy" => "hye", "ne" => "nep",
        "mn" => "mon", "bs" => "bos", "kk" => "kaz", "sq" => "sqi", "sw" => "swa",
        "gl" => "glg", "mr" => "mar", "pa" => "pan", "si" => "sin", "km" => "khm",
        "sn" => "sna", "yo" => "yor", "so" => "som", "af" => "afr", "oc" => "oci",
        "ka" => "kat", "be" => "bel", "tg" => "tgk", "sd" => "snd", "gu" => "guj",
        "am" => "amh", "yi" => "yid", "lo" => "lao", "uz" => "uzb", "fo" => "fao",
        "ht" => "hat", "ps" => "pus", "tk" => "tuk", "nn" => "nno", "mt" => "mlt",
        "sa" => "san", "lb" => "ltz", "my" => "mya", "bo" => "bod", "tl" => "tgl",
        "mg" => "mlg", "as" => "asm", "tt" => "tat", "haw" => "haw", "ln" => "lin",
        "ha" => "hau", "ba" => "bak", "jw" => "jav", "su" => "sun", "yue" => "yue",
        _ => "und",
    }
}

/// Смуксить видео (copy) + ДВЕ звуковые дорожки: дубляж (default, 1-я) + оригинал (2-я). MP4/MKV по
/// расширению `out`. Дубляж перекодируется в AAC 256k (сгенерированный микс), а оригинал КОПИРУЕТСЯ без
/// перекода (`-c:a copy`) — сохраняем каналы (5.1)/частоту/битрейт/кодек как есть, никакой деградации.
/// БЕЗ -shortest. Метки языка (ISO 639-2) и человекочитаемые title'ы кладутся в metadata дорожек;
/// disposition:a:0 default помечает дубляж дорожкой по умолчанию, оригинал — недефолтной. +faststart на mp4.
#[allow(clippy::too_many_arguments)]
pub fn mux_multitrack(
    video: &Path,
    dub_audio: &Path,
    orig_source: &Path,
    out: &Path,
    dub_lang: &str,
    orig_lang: &str,
    dub_title: &str,
    orig_title: &str,
) -> Result<(), String> {
    let dl = format!("language={dub_lang}");
    let dt = format!("title={dub_title}");
    let ol = format!("language={orig_lang}");
    let ot = format!("title={orig_title}");
    let is_mp4 = out
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("mp4"))
        .unwrap_or(true);
    let is_m4a = dub_audio
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("m4a") || e.eq_ignore_ascii_case("aac"))
        .unwrap_or(false);

    let mut args: Vec<&OsStr> = vec![
        OsStr::new("-y"),
        OsStr::new("-i"), video.as_os_str(),
        OsStr::new("-i"), dub_audio.as_os_str(),
        OsStr::new("-i"), orig_source.as_os_str(),
    ];

    if is_m4a {
        args.extend([
            OsStr::new("-map"), OsStr::new("0:v:0"),
            OsStr::new("-map"), OsStr::new("1:a:0"),
            OsStr::new("-map"), OsStr::new("2:a:0"),
            OsStr::new("-c:v"), OsStr::new("copy"),
            OsStr::new("-c:a:0"), OsStr::new("copy"),
            OsStr::new("-metadata:s:a:0"), OsStr::new(&dl),
            OsStr::new("-metadata:s:a:0"), OsStr::new(&dt),
            OsStr::new("-c:a:1"), OsStr::new("copy"),
            OsStr::new("-metadata:s:a:1"), OsStr::new(&ol),
            OsStr::new("-metadata:s:a:1"), OsStr::new(&ot),
            OsStr::new("-disposition:a:0"), OsStr::new("default"),
            OsStr::new("-disposition:a:1"), OsStr::new("0"),
        ]);
    } else {
        args.extend([
            OsStr::new("-filter_complex"), OsStr::new("[1:a]aformat=channel_layouts=stereo:sample_rates=44100[dub]"),
            OsStr::new("-map"), OsStr::new("0:v:0"),
            OsStr::new("-map"), OsStr::new("[dub]"),
            OsStr::new("-map"), OsStr::new("2:a:0"),
            OsStr::new("-c:v"), OsStr::new("copy"),
            OsStr::new("-c:a:0"), OsStr::new("aac"), OsStr::new("-b:a:0"), OsStr::new("192k"),
            OsStr::new("-ar:a:0"), OsStr::new("44100"),
            OsStr::new("-metadata:s:a:0"), OsStr::new(&dl),
            OsStr::new("-metadata:s:a:0"), OsStr::new(&dt),
            OsStr::new("-c:a:1"), OsStr::new("copy"),
            OsStr::new("-metadata:s:a:1"), OsStr::new(&ol),
            OsStr::new("-metadata:s:a:1"), OsStr::new(&ot),
            OsStr::new("-disposition:a:0"), OsStr::new("default"),
            OsStr::new("-disposition:a:1"), OsStr::new("0"),
        ]);
    }

    if is_mp4 {
        args.push(OsStr::new("-movflags"));
        args.push(OsStr::new("+faststart"));
    }
    args.push(out.as_os_str());
    run_ff(&args)
}

/// Ремукс лёгкого playable output.mp4 из мультитрек-mkv (#116): видео + ПЕРВАЯ (дубляж) дорожка, copy
/// без перекодирования (доли секунды) + faststart. Плеер редактора (WebView2 не играет Matroska) тянет
/// этот mp4, а «Сохранить» отдаёт полный mkv.
pub fn remux_playable_mp4(mkv: &Path, out_mp4: &Path) -> Result<(), String> {
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), mkv.as_os_str(),
        OsStr::new("-map"), OsStr::new("0:v:0"), OsStr::new("-map"), OsStr::new("0:a:0"),
        OsStr::new("-c"), OsStr::new("copy"),
        OsStr::new("-movflags"), OsStr::new("+faststart"), out_mp4.as_os_str(),
    ])
}

/// Есть ли в файле аудиопоток (ffprobe). Для мультитрек-mux: нет аудио в источнике -> вторую дорожку
/// не добавляем (иначе -disposition:a:1 по несуществующему потоку валит ffmpeg). Ошибка probe -> false.
pub fn has_audio(input: &Path) -> bool {
    cmd_silent(FFPROBE)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(input)
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Сконвертировать аудио в PCM 16-bit WAV (стерео). Финальный формат аудио-режима (вход без видео):
/// пачка WAV -> пачка озвученных WAV. Лоссовость только от исходного mix (aac), сам WAV без потерь.
pub fn to_wav(src: &Path, dst: &Path) -> Result<(), String> {
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-af"), OsStr::new("aformat=channel_layouts=stereo"),
        OsStr::new("-c:a"), OsStr::new("pcm_s16le"), dst.as_os_str(),
    ])
}

/// Вырезать [start,end] в mono @ sr Гц. Порт media.trim(..., sr=16000): реф-клипы 16к, keep-сплайс 24к.
pub fn trim(src: &Path, dst: &Path, start: f64, end: f64, sr: u32) -> Result<(), String> {
    let ss = format!("{:.3}", start);
    let to = format!("{:.3}", end);
    let ar = sr.to_string();
    run_ff(&[
        OsStr::new("-y"), OsStr::new("-ss"), OsStr::new(&ss), OsStr::new("-to"), OsStr::new(&to),
        OsStr::new("-i"), src.as_os_str(), OsStr::new("-ac"), OsStr::new("1"),
        OsStr::new("-ar"), OsStr::new(&ar), dst.as_os_str(),
    ])
}

// ─── Оконная нарезка для полнометражного пайплайна (#79) ──────────────────────────────────────────
// Новые хелперы (не трогают существующие): вырезать ОДНО окно вокала в отдельный WAV (RAM O(окна)),
// либо нарезать весь вокал segment-muxer'ом одним проходом. `-reset_timestamps 1` даёт локальный t=0 в
// каждом окне (чистый клип для BSRoformer/Sortformer/ASR) — обратный сдвиг window_offset делает
// вызывающий при сшивке диаризации/ASR (dub_asr::Window::offset). Хардненные флаги для длинных входов
// (BORROWINGS #19): +discardcorrupt / avoid_negative_ts / max_muxing_queue_size — против timestamp-drift
// и 'Too many packets buffered' на часовых файлах.

/// Вырезать окно [start,end) вокала в отдельный WAV @ sr Гц (mono). Локальный t=0 (accurate seek:
/// -ss ПОСЛЕ -i для точного реза по сэмплу). Для стадийной обработки одного окна — RAM O(окна).
// allow(dead_code): вызывается оркестратором оконного пайплайна (интеграция #79 идёт отдельно).
#[allow(dead_code)]
pub fn slice_window(src: &Path, dst: &Path, start: f64, end: f64, sr: u32) -> Result<(), String> {
    let ss = format!("{:.3}", start);
    let to = format!("{:.3}", end);
    let ar = sr.to_string();
    run_ff(&[
        OsStr::new("-y"),
        OsStr::new("-fflags"), OsStr::new("+discardcorrupt"),
        OsStr::new("-i"), src.as_os_str(),
        // accurate seek внутри уже открытого потока (после -i) — точная граница окна
        OsStr::new("-ss"), OsStr::new(&ss), OsStr::new("-to"), OsStr::new(&to),
        OsStr::new("-ac"), OsStr::new("1"), OsStr::new("-ar"), OsStr::new(&ar),
        OsStr::new("-avoid_negative_ts"), OsStr::new("make_zero"),
        OsStr::new("-c:a"), OsStr::new("pcm_s16le"),
        dst.as_os_str(),
    ])
}

/// Нарезать весь вокал на окна фикс. длины `win_sec` segment-muxer'ом за ОДИН проход (RAM O(окна),
/// не O(файла)). Файлы пишутся по шаблону `pattern` с `%03d` (например `win_%03d.wav`).
/// `-reset_timestamps 1` -> каждый сегмент стартует с t=0; window_offset = idx*win_sec прибавляет
/// вызывающий при ре-базинге. Дешёвый фолбэк к min-cut нарезке, когда важна только RAM-локальность.
// allow(dead_code): вызывается оркестратором оконного пайплайна (интеграция #79 идёт отдельно).
#[allow(dead_code)]
pub fn segment_wav(src: &Path, out_dir: &Path, pattern: &str, win_sec: f64, sr: u32) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;
    let seg_time = format!("{:.3}", win_sec.max(0.1));
    let ar = sr.to_string();
    let out_tpl = out_dir.join(pattern);
    run_ff(&[
        OsStr::new("-y"),
        OsStr::new("-fflags"), OsStr::new("+discardcorrupt"),
        OsStr::new("-i"), src.as_os_str(),
        OsStr::new("-ac"), OsStr::new("1"), OsStr::new("-ar"), OsStr::new(&ar),
        OsStr::new("-c:a"), OsStr::new("pcm_s16le"),
        OsStr::new("-f"), OsStr::new("segment"),
        OsStr::new("-segment_time"), OsStr::new(&seg_time),
        OsStr::new("-reset_timestamps"), OsStr::new("1"),
        OsStr::new("-max_muxing_queue_size"), OsStr::new("2048"),
        out_tpl.as_os_str(),
    ])
}

#[cfg(test)]
mod env_tests {
    use super::*;

    /// Референс-огибающая: та же кусочно-линейная функция, что генерит duck_volume_expr, но на Rust —
    /// проверяем ключевые точки (вне блока 1.0, в центре блока DUCK_GAIN, середины фейдов).
    // Референс gain(t): та же формула, что в duck_volume_expr — СУММА трапеций с clip суммы в [0,1]
    // (не return по первой трапеции), чтобы тест ловил пересечение блоков.
    fn gain_at(t: f64, blocks: &[SpeechBlock]) -> f64 {
        let g = DUCK_GAIN;
        let mut sum = 0.0f64;
        for b in blocks {
            let ds = (b.start - DUCK_PREROLL).max(0.0);
            let us = b.end + DUCK_HOLD;
            let ue = us + DUCK_FADE_UP;
            let trap = ((t - ds) / DUCK_FADE_DOWN).min((ue - t) / DUCK_FADE_UP).clamp(0.0, 1.0);
            sum += trap;
        }
        1.0 - (1.0 - g) * sum.clamp(0.0, 1.0)
    }

    #[test]
    fn envelope_key_points() {
        let blocks = [SpeechBlock { start: 5.0, end: 8.0 }, SpeechBlock { start: 20.0, end: 22.0 }];
        // вне любого блока -> 1.0
        assert!((gain_at(0.0, &blocks) - 1.0).abs() < 1e-9);
        assert!((gain_at(15.0, &blocks) - 1.0).abs() < 1e-9);
        // центр блока -> полное приглушение
        assert!((gain_at(6.5, &blocks) - DUCK_GAIN).abs() < 1e-9);
        // до начала спуска (за preroll) ещё 1.0
        assert!((gain_at(5.0 - DUCK_PREROLL - 0.01, &blocks) - 1.0).abs() < 1e-9);
        // середина fade-down: между 1.0 и g
        let mid_down = gain_at(5.0 - DUCK_PREROLL + DUCK_FADE_DOWN / 2.0, &blocks);
        assert!(mid_down < 1.0 && mid_down > DUCK_GAIN);
        // hold после конца блока ещё приглушено
        assert!((gain_at(8.0 + DUCK_HOLD - 0.01, &blocks) - DUCK_GAIN).abs() < 1e-9);
    }

    #[test]
    fn empty_blocks_is_flat_one() {
        assert_eq!(duck_volume_expr(&[], DUCK_GAIN), "1");
    }

    #[test]
    fn expr_mentions_each_block_boundaries() {
        let blocks = [SpeechBlock { start: 1.0, end: 2.0 }];
        let e = duck_volume_expr(&blocks, DUCK_GAIN);
        // спуск стартует за preroll (0.920), глубина 1-g присутствует
        assert!(e.contains("0.920"), "{e}");
        assert!(e.contains(&format!("{:.4}", 1.0 - DUCK_GAIN)), "{e}");
    }

    #[test]
    fn expr_length_linear_in_blocks() {
        // Длина выражения растёт линейно по блокам (вложенные if давали O(2^N) и взрывали память).
        let blocks: Vec<SpeechBlock> = (0..300)
            .map(|i| SpeechBlock { start: i as f64 * 10.0, end: i as f64 * 10.0 + 5.0 })
            .collect();
        let e = duck_volume_expr(&blocks, DUCK_GAIN);
        assert!(e.len() < 60 * 300 + 64, "len={}", e.len());
    }

    #[test]
    fn expr_has_clip_wrapper() {
        // clip(...,0,1) держит sum трапеций в [0,1] -> gain не ниже g и не отрицательный (#116).
        assert!(duck_volume_expr(&[SpeechBlock { start: 1.0, end: 2.0 }], DUCK_GAIN).contains("clip("));
    }

    #[test]
    fn overlapping_blocks_gain_never_below_g() {
        // Два блока ВПЛОТНУЮ (пауза 0.05с < суммы фейдов) — трапеции пересекаются, sum>1. Без clip gain
        // ушёл бы ниже g и в минус (инверсия фазы). С clip — держится в [g,1] на всей шкале.
        let blocks = [SpeechBlock { start: 1.0, end: 2.0 }, SpeechBlock { start: 2.05, end: 3.0 }];
        let mut t = 0.0;
        while t < 4.0 {
            let gv = gain_at(t, &blocks);
            assert!(gv >= DUCK_GAIN - 1e-9 && gv <= 1.0 + 1e-9, "t={t} gain={gv}");
            t += 0.01;
        }
        // в зоне стыка приглушение полное (обе трапеции=1, clip=1 -> g)
        assert!((gain_at(2.02, &blocks) - DUCK_GAIN).abs() < 1e-9);
    }
}

#[cfg(test)]
mod iso639_tests {
    use super::iso639_1_to_2;

    #[test]
    fn known_codes_map_to_iso639_2() {
        assert_eq!(iso639_1_to_2("ru"), "rus");
        assert_eq!(iso639_1_to_2("en"), "eng");
        assert_eq!(iso639_1_to_2("de"), "deu");
        assert_eq!(iso639_1_to_2("ja"), "jpn");
        assert_eq!(iso639_1_to_2("zh"), "zho");
        assert_eq!(iso639_1_to_2("uk"), "ukr");
        // 3-буквенные входы Whisper (haw/yue) тоже покрыты
        assert_eq!(iso639_1_to_2("haw"), "haw");
        assert_eq!(iso639_1_to_2("yue"), "yue");
    }

    #[test]
    fn case_and_whitespace_insensitive() {
        assert_eq!(iso639_1_to_2("RU"), "rus");
        assert_eq!(iso639_1_to_2("  Fr  "), "fra");
    }

    #[test]
    fn unknown_or_auto_is_und() {
        assert_eq!(iso639_1_to_2("auto"), "und");
        assert_eq!(iso639_1_to_2(""), "und");
        assert_eq!(iso639_1_to_2("xx"), "und");
    }

    #[test]
    fn every_whisper_code_has_mapping() {
        // Незнакомый код -> "und". Все коды из WHISPER_LANGS должны маппиться в НЕ-"und".
        for (code, _) in dub_translate::WHISPER_LANGS {
            assert_ne!(iso639_1_to_2(code), "und", "no ISO 639-2 mapping for {code}");
        }
    }
}

#[cfg(test)]
mod pause_squeeze_tests {
    use super::squeeze_internal_pauses;

    #[test]
    fn squeezes_internal_long_pauses_only() {
        let sr = 1000u32; // 1000 samples/sec -> 10 samples per 10ms window
        // Строим сигнал:
        // 0.0 .. 0.1с (100 samples) -> звук (амплитуда 0.5)
        // 0.1 .. 0.4с (300 samples = 300 мс) -> длинная пауза (тишина 0.0)
        // 0.4 .. 0.5с (100 samples) -> звук (амплитуда 0.5)
        let mut samples = Vec::new();
        samples.extend(vec![0.5f32; 100]); // word 1: 100ms
        samples.extend(vec![0.0f32; 300]); // pause: 300ms (>70ms)
        samples.extend(vec![0.5f32; 100]); // word 2: 100ms

        assert_eq!(samples.len(), 500);

        // Сжимаем паузу до 40 мс (40 samples при sr=1000)
        let squeezed = squeeze_internal_pauses(&samples, sr, 40.0);

        // Ожидаем ~240 samples (100 + 40 + 100) вместо 500
        assert!(squeezed.len() < 300, "len was {}", squeezed.len());
        assert!(squeezed.len() >= 220, "len was {}", squeezed.len());
    }

    #[test]
    fn keeps_short_pauses_untouched() {
        let sr = 1000u32;
        // Короткая пауза 50 мс (< 70 мс)
        let mut samples = Vec::new();
        samples.extend(vec![0.5f32; 100]); // word 1: 100ms
        samples.extend(vec![0.0f32; 50]);  // pause: 50ms (<70ms)
        samples.extend(vec![0.5f32; 100]); // word 2: 100ms

        let squeezed = squeeze_internal_pauses(&samples, sr, 40.0);
        assert_eq!(squeezed.len(), samples.len());
    }

    #[test]
    fn empty_or_pure_silence_returns_as_is() {
        let empty = squeeze_internal_pauses(&[], 16000, 40.0);
        assert!(empty.is_empty());

        let silence = vec![0.0f32; 1000];
        let res = squeeze_internal_pauses(&silence, 1000, 40.0);
        assert_eq!(res.len(), 1000);
    }
}

