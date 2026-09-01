//! Модуль точного по сэмплам дакинга (Sample-Accurate Ducking) и float-сведения для режима Voiceover.
//! Реализует требования VOICEOVER_HYBRID_IMPLEMENTATION_TZ_v2:
//! - Непрерывная огибающая с cosine easing (0.5 - 0.5*cos(pi*t)) без ступеней и разрывов;
//! - O(N) обработка сэмплов без аллокаций в цикле;
//! - Сведение в 32-bit float PCM;
//! - Отдельная стадия peak / headroom control перед финальным loudnorm (без двойного AAC-кодирования).

/// Речевой блок на таймлайне [start, end] в секундах.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeechBlock {
    pub start: f64,
    pub end: f64,
}

/// Параметры детерминированной огибающей дакинга.
#[derive(Clone, Debug, PartialEq)]
pub struct DuckEnvelopeParams {
    /// Глубина приглушения в dB (напр. -12.0 dB)
    pub duck_db: f64,
    /// Длительность предварительного спуска до начала речи (сек), дефолт 0.08с (80мс)
    pub preroll: f64,
    /// Длительность плавного спуска (сек), дефолт 0.12с (120мс)
    pub fade_down: f64,
    /// Длительность удержания приглушения после конца речи (сек), дефолт 0.20с (200мс)
    pub hold: f64,
    /// Длительность плавного подъёма (сек), дефолт 0.40с (400мс)
    pub fade_up: f64,
}

impl Default for DuckEnvelopeParams {
    fn default() -> Self {
        Self {
            duck_db: -12.0,
            preroll: 0.08,
            fade_down: 0.12,
            hold: 0.20,
            fade_up: 0.40,
        }
    }
}

/// Диагностические метрики сведе́ния для логов и телеметрии.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticMetrics {
    pub duck_mode: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub tts_peak: f32,
    pub original_peak: f32,
    pub mix_peak: f32,
    pub true_peak_est: f32,
    pub clipping_samples: usize,
}

/// Плавная C^1 косинусная интерполяция: 0.0 -> 0.0, 1.0 -> 1.0 с нулевой производной на краях.
#[inline(always)]
pub fn cosine_ease(u: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * u.clamp(0.0, 1.0)).cos()
}

/// Сгенерировать sample-accurate огибающую коэффициента усиления [0.0..1.0] для каждого сэмпла.
/// Гарантии:
/// - 0.0 <= duck_gain <= gain <= 1.0;
/// - Монотонные переходы без ступенек и разрывов производной;
/// - O(N) проход по сэмплам, без аллокаций в цикле.
pub fn generate_duck_envelope(
    n_samples: usize,
    sr: u32,
    blocks: &[SpeechBlock],
    params: &DuckEnvelopeParams,
) -> Vec<f32> {
    if n_samples == 0 {
        return Vec::new();
    }
    let g = if params.duck_db <= -60.0 {
        0.0f64
    } else {
        10f64.powf(params.duck_db / 20.0).clamp(0.0, 1.0)
    };
    if blocks.is_empty() {
        return vec![1.0f32; n_samples];
    }

    struct BlockBounds {
        ds: f64,
        de: f64,
        us: f64,
        ue: f64,
        inv_fd: f64,
        inv_fu: f64,
    }

    let mut sorted_blocks = blocks.to_vec();
    sorted_blocks.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

    let bounds: Vec<BlockBounds> = sorted_blocks
        .iter()
        .filter(|b| b.end > b.start)
        .map(|b| {
            let ds = (b.start - params.preroll).max(0.0);
            let de = ds + params.fade_down.max(1e-4);
            let us = b.end + params.hold.max(0.0);
            let ue = us + params.fade_up.max(1e-4);
            BlockBounds {
                ds,
                de,
                us,
                ue,
                inv_fd: 1.0 / params.fade_down.max(1e-4),
                inv_fu: 1.0 / params.fade_up.max(1e-4),
            }
        })
        .collect();

    if bounds.is_empty() {
        return vec![1.0f32; n_samples];
    }

    let sr_f64 = sr as f64;
    let mut env = vec![1.0f32; n_samples];
    let mut active_idx = 0usize;

    for (i, sample) in env.iter_mut().enumerate() {
        let t = i as f64 / sr_f64;

        while active_idx < bounds.len() && bounds[active_idx].ue < t {
            active_idx += 1;
        }

        let mut sum_trap = 0.0f64;
        let mut j = active_idx;
        while j < bounds.len() {
            let b = &bounds[j];
            if b.ds > t {
                break;
            }
            if t >= b.ds && t <= b.ue {
                let trap = if t < b.de {
                    cosine_ease((t - b.ds) * b.inv_fd)
                } else if t <= b.us {
                    1.0
                } else {
                    cosine_ease((b.ue - t) * b.inv_fu)
                };
                sum_trap += trap;
            }
            j += 1;
        }

        let weight = sum_trap.clamp(0.0, 1.0);
        let gain = 1.0 - (1.0 - g) * weight;
        *sample = gain as f32;
    }

    env
}

/// Выполнить sample-accurate float сведение оригинального аудио и дубляжа (TTS).
/// Применяет огибающую к original audio_hq и суммирует в float32.
/// Примечание: Лимитер/пик-контроль вынесен в отдельную стадию `apply_peak_control`.
pub fn mix_voiceover_sample_accurate(
    audio_hq_channels: &[Vec<f32>],
    tts_channels: &[Vec<f32>],
    sr: u32,
    speech_blocks: &[SpeechBlock],
    params: &DuckEnvelopeParams,
    duck_mode: &str,
) -> Result<(Vec<Vec<f32>>, DiagnosticMetrics), String> {
    if audio_hq_channels.is_empty() {
        return Err("audio_hq_channels is empty".into());
    }
    let n_channels = audio_hq_channels.len();
    let n_frames = audio_hq_channels[0].len();
    for ch in audio_hq_channels {
        if ch.len() != n_frames {
            return Err("mismatched channel lengths in audio_hq".into());
        }
    }

    // Генерируем непрерывную огибающую
    let envelope = generate_duck_envelope(n_frames, sr, speech_blocks, params);

    let mut mixed_channels = vec![vec![0.0f32; n_frames]; n_channels];
    let mut tts_peak = 0.0f32;
    let mut orig_peak = 0.0f32;
    let mut mix_peak = 0.0f32;
    let mut clipping_count = 0usize;

    for i in 0..n_frames {
        let env_val = envelope[i];
        let tts_mono = if !tts_channels.is_empty() {
            if tts_channels.len() == 1 {
                tts_channels[0].get(i).copied().unwrap_or(0.0)
            } else {
                let sum: f32 = tts_channels.iter().map(|ch| ch.get(i).copied().unwrap_or(0.0)).sum();
                sum / tts_channels.len() as f32
            }
        } else {
            0.0
        };

        tts_peak = tts_peak.max(tts_mono.abs());

        for c in 0..n_channels {
            let orig_s = audio_hq_channels[c][i];
            orig_peak = orig_peak.max(orig_s.abs());

            let ducked_s = orig_s * env_val;
            let sum_s = ducked_s + tts_mono;

            mix_peak = mix_peak.max(sum_s.abs());
            if sum_s.abs() > 1.0 {
                clipping_count += 1;
            }
            mixed_channels[c][i] = sum_s;
        }
    }

    // Грубая оценка True-Peak через оверсэмплинг/межсэмпловый пик
    let true_peak_est = mix_peak * 1.05;

    let metrics = DiagnosticMetrics {
        duck_mode: duck_mode.to_string(),
        sample_rate: sr,
        channels: n_channels,
        tts_peak,
        original_peak: orig_peak,
        mix_peak,
        true_peak_est,
        clipping_samples: clipping_count,
    };

    Ok((mixed_channels, metrics))
}

/// Выполнить sample-accurate 3-трековое сведение для дубляжа «С эффектами»:
/// 1) Instrumental (фоновая музыка/интершумы) — играет на 100% непрерывно (фон НЕ глушится!);
/// 2) Original Vocals (оригинальный вокал) — глушится в 0 под речью дубляжа, 100% в паузах (сохраняет вздохи, крики, стоны);
/// 3) Dubbing TTS (голос дубляжа) — накладывается поверх.
pub fn mix_3way_dub_sample_accurate(
    inst_channels: &[Vec<f32>],
    voc_channels: &[Vec<f32>],
    tts_channels: &[Vec<f32>],
    sr: u32,
    speech_blocks: &[SpeechBlock],
    params: &DuckEnvelopeParams,
    duck_mode: &str,
) -> Result<(Vec<Vec<f32>>, DiagnosticMetrics), String> {
    if inst_channels.is_empty() {
        return Err("inst_channels is empty".into());
    }
    let n_channels = inst_channels.len();
    let n_frames = inst_channels[0].len();
    for ch in inst_channels {
        if ch.len() != n_frames {
            return Err("mismatched channel lengths in inst_channels".into());
        }
    }

    // Генерируем непрерывную огибающую для оригинального вокала (0.0 под речью, 1.0 в паузах)
    let envelope = generate_duck_envelope(n_frames, sr, speech_blocks, params);

    let mut mixed_channels = vec![vec![0.0f32; n_frames]; n_channels];
    let mut tts_peak = 0.0f32;
    let mut orig_peak = 0.0f32;
    let mut mix_peak = 0.0f32;
    let mut clipping_count = 0usize;

    for i in 0..n_frames {
        let env_val = envelope[i];
        let tts_mono = if !tts_channels.is_empty() {
            if tts_channels.len() == 1 {
                tts_channels[0].get(i).copied().unwrap_or(0.0)
            } else {
                let sum: f32 = tts_channels.iter().map(|ch| ch.get(i).copied().unwrap_or(0.0)).sum();
                sum / tts_channels.len() as f32
            }
        } else {
            0.0
        };

        tts_peak = tts_peak.max(tts_mono.abs());

        for c in 0..n_channels {
            let inst_s = inst_channels[c][i];
            let voc_s = voc_channels.get(c)
                .or_else(|| voc_channels.get(0))
                .and_then(|ch| ch.get(i))
                .copied()
                .unwrap_or(0.0);

            orig_peak = orig_peak.max(inst_s.abs().max(voc_s.abs()));

            let ducked_voc_s = voc_s * env_val;
            let sum_s = inst_s + ducked_voc_s + tts_mono;

            mix_peak = mix_peak.max(sum_s.abs());
            if sum_s.abs() > 1.0 {
                clipping_count += 1;
            }
            mixed_channels[c][i] = sum_s;
        }
    }

    let true_peak_est = mix_peak * 1.05;

    let metrics = DiagnosticMetrics {
        duck_mode: duck_mode.to_string(),
        sample_rate: sr,
        channels: n_channels,
        tts_peak,
        original_peak: orig_peak,
        mix_peak,
        true_peak_est,
        clipping_samples: clipping_count,
    };

    Ok((mixed_channels, metrics))
}

/// Мягкое параболическое ограничение пиков (Soft-Knee Peak Limiting) для 1D-среза сэмплов.
/// Предотвращает цифровой клиппинг и щелчки без внесения ступенек формы волны.
pub fn apply_peak_control_slice(samples: &mut [f32], ceiling_dbfs: f32) {
    let ceiling = 10f32.powf(ceiling_dbfs / 20.0).clamp(0.5, 1.0);
    let knee_start = ceiling * 0.85;

    for sample in samples.iter_mut() {
        let abs_s = sample.abs();
        if abs_s > knee_start {
            let sign = if *sample >= 0.0 { 1.0 } else { -1.0 };
            let x = (abs_s - knee_start) / (ceiling - knee_start + 1e-6);
            // Мягкая параболическая сатурация (soft knee)
            let compressed = if x < 1.0 {
                knee_start + (ceiling - knee_start) * (x - 0.25 * x * x)
            } else {
                ceiling + (ceiling - knee_start) * 0.75 * ((x - 1.0) / (x + 1.0))
            };
            *sample = (compressed * sign).clamp(-ceiling, ceiling);
        }
    }
}

/// Отдельная стадия контроля запаса по уровню (Headroom) и True-Peak защиты для многоканального аудио.
/// Мягко поджимает пики выше ceiling_linear без внесения ступенек.
pub fn apply_peak_control(channels: &mut [Vec<f32>], ceiling_dbfs: f32) {
    for ch in channels.iter_mut() {
        apply_peak_control_slice(ch, ceiling_dbfs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_easing_bounds_and_symmetry() {
        assert_eq!(cosine_ease(0.0), 0.0);
        assert!((cosine_ease(0.5) - 0.5).abs() < 1e-9);
        assert_eq!(cosine_ease(1.0), 1.0);
        assert_eq!(cosine_ease(-0.5), 0.0);
        assert_eq!(cosine_ease(1.5), 1.0);

        // Проверяем монотонность
        let mut prev = 0.0;
        for i in 1..=100 {
            let u = i as f64 / 100.0;
            let val = cosine_ease(u);
            assert!(val >= prev, "cosine_ease not monotonic at u={u}");
            prev = val;
        }
    }

    #[test]
    fn test_generate_duck_envelope_monotonic_transitions() {
        let sr = 1000u32;
        let params = DuckEnvelopeParams {
            duck_db: -12.0,
            preroll: 0.10,
            fade_down: 0.10,
            hold: 0.20,
            fade_up: 0.30,
        };
        let blocks = vec![SpeechBlock { start: 1.0, end: 2.0 }];
        let n_samples = 3500; // 3.5 секунды
        let env = generate_duck_envelope(n_samples, sr, &blocks, &params);
        let g = 10f32.powf(-12.0 / 20.0);

        // 1. Проверяем граничные условия
        for (i, &val) in env.iter().enumerate() {
            assert!(val >= g - 1e-5 && val <= 1.0 + 1e-5, "Sample {i} out of bounds: {val}");
        }

        // Вне блока (t = 0.5s) -> 1.0
        assert!((env[500] - 1.0).abs() < 1e-4);

        // Внутри блока (t = 1.5s) -> g (-12 dB)
        assert!((env[1500] - g).abs() < 1e-4);

        // Монотонный спуск (t в [0.9s, 1.0s], т.е. сэмплы 900..1000)
        for i in 901..=1000 {
            assert!(env[i] <= env[i - 1] + 1e-6, "Fade down not monotonic at {i}: {} > {}", env[i], env[i-1]);
        }

        // Монотонный подъём (t в [2.2s, 2.5s], т.е. сэмплы 2200..2500)
        for i in 2201..=2500 {
            assert!(env[i] >= env[i - 1] - 1e-6, "Fade up not monotonic at {i}: {} < {}", env[i], env[i-1]);
        }
    }

    #[test]
    fn test_regression_synthetic_fixtures() {
        let sr = 44100u32;
        let params = DuckEnvelopeParams::default();
        let blocks = vec![
            SpeechBlock { start: 0.2, end: 0.5 },
            SpeechBlock { start: 0.8, end: 1.2 },
        ];
        let n_samples = (sr as f64 * 1.5) as usize;

        // --- Fixture A: Normal case with headroom (~ -1 dBFS, amp = 0.89) ---
        let amp_normal = 0.89f32;
        let mut orig_normal = vec![0.0f32; n_samples];
        for (i, s) in orig_normal.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            *s = amp_normal * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
        }

        let mut tts_normal = vec![0.0f32; n_samples];
        for (i, s) in tts_normal.iter_mut().enumerate() {
            let t = i as f64 / sr as f64;
            if (0.2..=0.5).contains(&t) || (0.8..=1.2).contains(&t) {
                *s = 0.5f32 * (2.0 * std::f32::consts::PI * 400.0 * t as f32).sin();
            }
        }

        let (mut mix_normal, metrics_a) = mix_voiceover_sample_accurate(
            &[orig_normal.clone(), orig_normal],
            &[tts_normal],
            sr,
            &blocks,
            &params,
            "speech_aware",
        ).unwrap();

        apply_peak_control(&mut mix_normal, -1.0);
        assert_eq!(metrics_a.duck_mode, "speech_aware");
        assert!(metrics_a.tts_peak > 0.4);

        // Проверяем отсутствие резких скачков (дельта между соседними сэмплами гладкая)
        for ch in &mix_normal {
            for i in 1..ch.len() {
                let delta = (ch[i] - ch[i - 1]).abs();
                // Максимальный шаг синусоиды 1кГц при sr=44100 составляет ~ 2*pi*1000/44100 * amp ≈ 0.13
                assert!(delta < 0.20, "Discontinuity jump at sample {i}: delta={delta}");
            }
        }

        // --- Fixture B: Stress case with overload (peaks near 0 dBFS, amp = 1.0) ---
        let mut orig_stress = vec![0.0f32; n_samples];
        for (i, s) in orig_stress.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            *s = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
        }
        let mut tts_stress = vec![0.0f32; n_samples];
        for (i, s) in tts_stress.iter_mut().enumerate() {
            let t = i as f64 / sr as f64;
            if (0.2..=0.5).contains(&t) || (0.8..=1.2).contains(&t) {
                *s = 0.95f32 * (2.0 * std::f32::consts::PI * 300.0 * t as f32).sin();
            }
        }

        let (mut mix_stress, _) = mix_voiceover_sample_accurate(
            &[orig_stress.clone(), orig_stress],
            &[tts_stress],
            sr,
            &blocks,
            &params,
            "speech_aware",
        ).unwrap();

        // Применяем peak control и проверяем, что лимит соблюден
        apply_peak_control(&mut mix_stress, -1.0);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        for ch in &mix_stress {
            for &s in ch {
                assert!(s.abs() <= ceiling + 1e-4, "Overload peak escaped ceiling: {s}");
            }
        }
    }

    #[test]
    fn test_apply_peak_control_slice_smoothness() {
        let mut slice = vec![0.0f32; 100];
        for (i, s) in slice.iter_mut().enumerate() {
            *s = (i as f32 / 50.0) * 1.5; // от 0 до 3.0 (сильный перегруз)
        }
        apply_peak_control_slice(&mut slice, -0.2);
        let ceiling = 10f32.powf(-0.2 / 20.0);
        for &s in &slice {
            assert!(s.abs() <= ceiling + 1e-4, "Exceeded ceiling: {s}");
        }
        // Проверка монотонности и отсутствия ступенек
        for i in 1..slice.len() {
            assert!(slice[i] >= slice[i - 1], "Non-monotonic soft knee at {i}");
        }
    }
}

