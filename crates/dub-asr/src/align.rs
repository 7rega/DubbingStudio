//! Прецизионное выравнивание субтитров/слотов озвучки по вокальной дорожке.
//!
//! Разработано специально для слотов дубляжа и закадрового перевода (TTS-синтез):
//! 1. Отсекает вздохи, кашель, шипение микрофона (Hysteresis VAD + автокорреляция основного тона/гармоник).
//! 2. Не сжимает длинные фразы в одно слово (Multi-span fusion: объединяет слова и фразовые кластеры).
//! 3. Ставит начало строго на старт артикуляции первого звука (Onset) с микро-упреждением 25–35 мс на атаку согласных.
//! 4. Сохраняет естественный хронометраж реплики для естественной скорости TTS-речи.
//! 5. Гарантирует микро-зазор между соседними фразами (MIN_GAP = 40 мс) без каскадного сдвига.

use std::f64::consts::PI;

/// Минимальный защитный зазор между соседними репликами (секунды).
pub const MIN_SUBTITLE_GAP: f64 = 0.040; // 40мс

/// Микро-упреждение перед первым звуком речи (атака согласных "п", "т", "к", "с", "ш").
pub const SPEECH_LEAD_IN: f64 = 0.030; // 30мс

/// Естественное акустическое затухание после окончания речи.
pub const SPEECH_TAIL: f64 = 0.045; // 45мс

/// Максимальная пауза между словами внутри одной фразы, считающаяся единой репликой (секунды).
pub const MAX_INTRA_PHRASE_GAP: f64 = 0.550; // 550мс

/// Максимальный допустимый дрейф от исходного таймкода при поиске вокала (секунды).
pub const MAX_ALIGN_DRIFT: f64 = 0.900; // 900мс

/// Простой biquad IIR-фильтр (Direct Form II Transposed).
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn highpass(fc: f64, q: f64, fs: f64) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b0 = ((1.0 + cos_w0) / 2.0) as f32;
        let b1 = (-(1.0 + cos_w0)) as f32;
        let b2 = ((1.0 + cos_w0) / 2.0) as f32;
        let a0 = (1.0 + alpha) as f32;
        let a1 = (-2.0 * cos_w0) as f32;
        let a2 = (1.0 - alpha) as f32;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn lowpass(fc: f64, q: f64, fs: f64) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b0 = ((1.0 - cos_w0) / 2.0) as f32;
        let b1 = (1.0 - cos_w0) as f32;
        let b2 = ((1.0 - cos_w0) / 2.0) as f32;
        let a0 = (1.0 + alpha) as f32;
        let a1 = (-2.0 * cos_w0) as f32;
        let a2 = (1.0 - alpha) as f32;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, in_sample: f32) -> f32 {
        let out = self.b0 * in_sample + self.z1;
        self.z1 = self.b1 * in_sample - self.a1 * out + self.z2;
        self.z2 = self.b2 * in_sample - self.a2 * out;
        out
    }
}

/// Полосовая фильтрация вокала (130 Гц – 4000 Гц): убирает низкочастотный гул и высокочастотный шум/шипение.
fn filter_speech_band(samples: &[f32], sr: u32) -> Vec<f32> {
    let fs = sr as f64;
    let mut hp = Biquad::highpass(130.0, 0.7071, fs);
    let mut lp = Biquad::lowpass(4000.0, 0.7071, fs);
    let mut filtered = Vec::with_capacity(samples.len());
    for &s in samples {
        let h = hp.process(s);
        let l = lp.process(h);
        filtered.push(l);
    }
    filtered
}

/// Вычисление максимального коэффициента автокорреляции в диапазоне частот голоса (80–500 Гц).
/// Голос (гласные, сонорные звуки) имеет r_max >= 0.35..0.95.
/// Вздохи, шум воздуха, шипение микрофона имеют r_max < 0.20.
fn voice_autocorrelation(frame: &[f32], sr: u32) -> f32 {
    let n = frame.len();
    if n < 64 {
        return 0.0;
    }
    let mut energy = 0.0f64;
    for &x in frame {
        energy += (x as f64) * (x as f64);
    }
    if energy < 1e-8 {
        return 0.0;
    }

    // Лаг для частот 80..500 Гц
    let min_lag = (sr as usize / 500).max(1);
    let max_lag = (sr as usize / 80).min(n.saturating_sub(10));
    if min_lag >= max_lag {
        return 0.0;
    }

    let mut best_corr = 0.0f64;
    for lag in min_lag..=max_lag {
        let mut sum_xy = 0.0f64;
        let mut sum_xx = 0.0f64;
        let mut sum_yy = 0.0f64;
        let len = n - lag;
        for i in 0..len {
            let x = frame[i] as f64;
            let y = frame[i + lag] as f64;
            sum_xy += x * y;
            sum_xx += x * x;
            sum_yy += y * y;
        }
        let denom = (sum_xx * sum_yy).sqrt();
        if denom > 1e-8 {
            let norm_corr = sum_xy / denom;
            if norm_corr > best_corr {
                best_corr = norm_corr;
            }
        }
    }
    best_corr as f32
}

/// Обнаруженный речевой спан [start, end] в секундах с меткой наличия истинного голоса.
#[derive(Clone, Copy, Debug)]
pub struct SpeechSpan {
    pub start: f64,
    pub end: f64,
    pub onset_exact: f64,
    pub max_energy: f32,
    pub is_voiced: bool,
}

/// Высокоточный детектор речевых спанов с отсечением вздохов и дыхания.
pub fn extract_vocal_spans(samples: &[f32], sr: u32) -> Vec<SpeechSpan> {
    if samples.is_empty() || sr == 0 {
        return Vec::new();
    }

    // 1. Полосовая фильтрация
    let filtered = filter_speech_band(samples, sr);

    // 2. Расчет покадровых признаков (кадр 20мс, шаг 10мс)
    let frame_len = ((0.020 * sr as f64).round() as usize).max(16);
    let step_len = ((0.010 * sr as f64).round() as usize).max(8);
    let frame_sec = step_len as f64 / sr as f64;

    let mut energies = Vec::new();
    let mut voicings = Vec::new();
    let mut pos = 0;

    while pos + frame_len <= filtered.len() {
        let f = &filtered[pos..pos + frame_len];
        let mut acc = 0.0f64;
        for &s in f {
            acc += (s as f64) * (s as f64);
        }
        let rms = (acc / frame_len as f64).sqrt() as f32;
        let r_max = voice_autocorrelation(f, sr);

        energies.push(rms);
        voicings.push(r_max);
        pos += step_len;
    }

    if energies.is_empty() {
        return Vec::new();
    }

    // 3. Адаптивные пороги (устойчивые к шуму процентили)
    let mut sorted_e = energies.clone();
    sorted_e.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> f32 {
        let idx = ((sorted_e.len() as f64 - 1.0) * p).round() as usize;
        sorted_e[idx.min(sorted_e.len() - 1)]
    };

    let floor = pct(0.12);
    let peak = pct(0.92);
    let dynamic_range = (peak - floor).max(1e-4);

    // Двухпороговый гистерезис:
    // low_thr: граница затухания/начала
    // core_thr: подтверждение настоящей речи (не тихого вздоха)
    let low_thr = floor + 0.045 * dynamic_range;
    let core_thr = floor + 0.150 * dynamic_range;

    // 4. Поиск речевых регионов
    let mut raw_spans: Vec<(usize, usize, usize, f32, bool)> = Vec::new(); // (start_idx, end_idx, core_idx, max_e, is_voiced)
    let mut in_span = false;
    let mut span_start = 0;
    let mut max_e = 0.0f32;
    let mut max_v = 0.0f32;
    let mut first_core_idx = None;

    for i in 0..energies.len() {
        let e = energies[i];
        let v = voicings[i];

        if e > low_thr {
            if !in_span {
                in_span = true;
                span_start = i;
                max_e = e;
                max_v = v;
                first_core_idx = None;
            } else {
                if e > max_e {
                    max_e = e;
                }
                if v > max_v {
                    max_v = v;
                }
            }

            // Проверка на ядро речи: обязательное наличие гармоник тона связок (v >= 0.35) или взрывная атака с умеренным тоном
            let is_voiced = v >= 0.35 && e > low_thr;
            let is_strong_core = e > core_thr && v >= 0.28;
            if (is_voiced || is_strong_core) && first_core_idx.is_none() {
                first_core_idx = Some(i);
            }
        } else if in_span {
            in_span = false;
            let span_end = i;
            // Исключаем вздохи и чистый шум воздуха: ОБЯЗАТЕЛЬНО наличие голосовых гармоник (max_v >= 0.30)
            let is_genuine_speech = first_core_idx.is_some() && max_v >= 0.30 && max_e > low_thr * 1.3;

            if is_genuine_speech {
                let core = first_core_idx.unwrap_or(span_start);
                raw_spans.push((span_start, span_end, core, max_e, true));
            }
        }
    }

    if in_span && first_core_idx.is_some() && max_v >= 0.30 && max_e > low_thr * 1.3 {
        let span_end = energies.len();
        raw_spans.push((span_start, span_end, first_core_idx.unwrap(), max_e, true));
    }

    // 5. Преобразование индексов в секунды с отсечением предвдоха
    let mut result_spans: Vec<SpeechSpan> = Vec::new();
    for (s_idx, e_idx, core_idx, peak_e, voiced) in raw_spans {
        let raw_start_sec = s_idx as f64 * frame_sec;
        let core_sec = core_idx as f64 * frame_sec;
        let end_sec = e_idx as f64 * frame_sec;

        // Если между началом нарастания и ядром речи более 150мс — начало было вдохом.
        // За истинный Onset берем точку непосредственно перед ядром речи (~40мс перед core).
        let onset_sec = if core_sec - raw_start_sec > 0.150 {
            (core_sec - 0.040).max(raw_start_sec)
        } else {
            raw_start_sec
        };

        if end_sec - onset_sec >= 0.050 {
            result_spans.push(SpeechSpan {
                start: onset_sec,
                end: end_sec,
                onset_exact: onset_sec,
                max_energy: peak_e,
                is_voiced: voiced,
            });
        }
    }

    result_spans
}

/// Трейт для сегмента (субтитра), позволяющий выравнивать как структуры dub-core::Segment, так и тестовые типы.
pub trait AlignableSegment {
    fn start(&self) -> f64;
    fn end(&self) -> f64;
    fn set_bounds(&mut self, start: f64, end: f64);
    fn mark_dirty(&mut self);
}

/// Простая структура границ сегмента.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentBound {
    pub start: f64,
    pub end: f64,
}

impl AlignableSegment for SegmentBound {
    fn start(&self) -> f64 {
        self.start
    }
    fn end(&self) -> f64 {
        self.end
    }
    fn set_bounds(&mut self, start: f64, end: f64) {
        self.start = start;
        self.end = end;
    }
    fn mark_dirty(&mut self) {}
}

/// Выравнивание массива границ `[start, end]`. Возвращает вектор флагов `true`, если соответствующий сегмент был изменён.
pub fn align_bounds(bounds: &mut [SegmentBound], samples: &[f32], sr: u32) -> Vec<bool> {
    let original: Vec<SegmentBound> = bounds.to_vec();
    align_segments_to_vocals(bounds, samples, sr);
    bounds
        .iter()
        .zip(original.iter())
        .map(|(cur, orig)| {
            (cur.start - orig.start).abs() > 0.005 || (cur.end - orig.end).abs() > 0.005
        })
        .collect()
}

/// Выравнивание списка сегментов по вокальной дорожке с сохранением хронометража и мульти-спановым охватом.
pub fn align_segments_to_vocals<T: AlignableSegment>(
    segments: &mut [T],
    samples: &[f32],
    sr: u32,
) -> usize {
    if segments.is_empty() || samples.is_empty() || sr == 0 {
        return 0;
    }

    let spans = extract_vocal_spans(samples, sr);
    if spans.is_empty() {
        return 0;
    }

    let mut changed_count = 0;
    let n = segments.len();

    for i in 0..n {
        let orig_start = segments[i].start();
        let orig_end = segments[i].end();
        let orig_center = (orig_start + orig_end) / 2.0;
        let orig_len = (orig_end - orig_start).max(0.1);

        // Границы со стороны соседей
        let prev_bound = if i > 0 {
            segments[i - 1].end() + MIN_SUBTITLE_GAP
        } else {
            0.0
        };

        let next_bound = if i + 1 < n {
            segments[i + 1].start() - MIN_SUBTITLE_GAP
        } else {
            f64::INFINITY
        };

        let drift_limit = (orig_len * 0.55).clamp(0.40, MAX_ALIGN_DRIFT);

        // 1. Находим все речевые спаны, относящиеся к этой фразе (Multi-span)
        let matching_spans: Vec<&SpeechSpan> = spans
            .iter()
            .filter(|span| {
                // Прямое перекрытие с текущим сегментом
                let overlap = (span.end.min(orig_end) - span.start.max(orig_start)).max(0.0);
                if overlap > 0.02 {
                    return true;
                }
                // Или спан находится в непосредственной близости в пределах допустимого дрейфа
                let dist_start = (span.start - orig_start).abs();
                let dist_end = (span.end - orig_end).abs();
                let dist_center = ((span.start + span.end) / 2.0 - orig_center).abs();

                (dist_start <= drift_limit || dist_end <= drift_limit || dist_center <= drift_limit)
                    && span.start >= prev_bound - 0.1
                    && span.end <= next_bound + 0.1
            })
            .collect();

        if matching_spans.is_empty() {
            continue;
        }

        // 2. Объединяем речевые спаны фразы от первого Onset до последнего Offset
        let mut min_onset = f64::INFINITY;
        let mut max_offset = 0.0f64;

        for span in matching_spans {
            if span.onset_exact < min_onset {
                min_onset = span.onset_exact;
            }
            if span.end > max_offset {
                max_offset = span.end;
            }
        }

        if min_onset >= max_offset {
            continue;
        }

        // 3. Добавляем упреждение для согласных (LEAD_IN) и шлейф затухания (TAIL)
        let target_start = min_onset - SPEECH_LEAD_IN;
        let target_end = max_offset + SPEECH_TAIL;

        // 4. Защита от наездов на соседей и минимальная длительность
        let clamped_start = target_start.max(prev_bound).max(0.0);
        let clamped_end = target_end.min(next_bound).max(clamped_start + 0.100);

        // 5. Проверяем, что сдвиг оправдан и не превышает лимит дрейфа
        if (clamped_start - orig_start).abs() <= drift_limit + 0.15
            && (clamped_end - orig_end).abs() <= drift_limit + 0.15
        {
            let rounded_start = (clamped_start * 100.0).round() / 100.0;
            let rounded_end = (clamped_end * 100.0).round() / 100.0;

            if (rounded_start - orig_start).abs() > 0.015 || (rounded_end - orig_end).abs() > 0.015 {
                segments[i].set_bounds(rounded_start, rounded_end);
                segments[i].mark_dirty();
                changed_count += 1;
            }
        }
    }

    changed_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct MockSegment {
        start: f64,
        end: f64,
        dirty: bool,
    }

    impl AlignableSegment for MockSegment {
        fn start(&self) -> f64 {
            self.start
        }
        fn end(&self) -> f64 {
            self.end
        }
        fn set_bounds(&mut self, start: f64, end: f64) {
            self.start = start;
            self.end = end;
        }
        fn mark_dirty(&mut self) {
            self.dirty = true;
        }
    }

    /// Генератор синусоиды (голос со стабильным тоном F0).
    fn generate_voice_tone(sr: u32, duration_sec: f64, freq_hz: f64, amp: f32) -> Vec<f32> {
        let n = (sr as f64 * duration_sec).round() as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / sr as f64;
            let s = (2.0 * PI * freq_hz * t).sin() as f32 * amp;
            out.push(s);
        }
        out
    }

    /// Генератор шума (вздох/шипение воздуха без тона).
    fn generate_breath_noise(sr: u32, duration_sec: f64, amp: f32) -> Vec<f32> {
        let n = (sr as f64 * duration_sec).round() as usize;
        let mut out = Vec::with_capacity(n);
        let mut seed: u32 = 12345;
        for _ in 0..n {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let raw = ((seed >> 16) & 0x7fff) as f32 / 32768.0;
            out.push((raw * 2.0 - 1.0) * amp);
        }
        out
    }

    #[test]
    fn test_rejects_pre_speech_breath() {
        let sr = 16000;
        let mut audio = Vec::new();

        // 0.0 - 0.5s: тишина
        audio.extend(vec![0.0f32; 8000]);

        // 0.5 - 0.8s: вдох (шум воздуха)
        audio.extend(generate_breath_noise(sr, 0.3, 0.08));

        // 0.8 - 1.8s: чистая речь (голос 200 Гц с высокой амплитудой)
        audio.extend(generate_voice_tone(sr, 1.0, 200.0, 0.5));

        // 1.8 - 2.5s: тишина
        audio.extend(vec![0.0f32; 11200]);

        let spans = extract_vocal_spans(&audio, sr);
        assert!(!spans.is_empty(), "Должен найти речевой спан");

        let first = &spans[0];
        // Начало должно быть в районе 0.76..0.82с (речь), а НЕ на 0.5с (вдох)
        assert!(
            first.onset_exact >= 0.75 && first.onset_exact <= 0.85,
            "Onset должен отсечь вдох: actual={}",
            first.onset_exact
        );
    }

    #[test]
    fn test_multi_span_sentence_fusion() {
        let sr = 16000;
        let mut audio = Vec::new();

        // Фраза из двух слов с паузой 250мс:
        // 0.5 - 1.0s: Слово 1
        audio.extend(vec![0.0f32; 8000]);
        audio.extend(generate_voice_tone(sr, 0.5, 200.0, 0.5));

        // 1.0 - 1.25s: Пауза 250мс
        audio.extend(vec![0.0f32; 4000]);

        // 1.25 - 1.85s: Слово 2
        audio.extend(generate_voice_tone(sr, 0.6, 200.0, 0.5));
        audio.extend(vec![0.0f32; 8000]);

        let mut segs = vec![MockSegment {
            start: 0.45,
            end: 1.90,
            dirty: false,
        }];

        let changed = align_segments_to_vocals(&mut segs, &audio, sr);
        assert!(changed > 0 || (segs[0].start <= 0.50 && segs[0].end >= 1.80));

        // Проверяем, что сегмент охватывает ОБА слова (до ~1.85с), а не схлопнулся до 1.0с!
        assert!(
            segs[0].end >= 1.80,
            "Сегмент должен охватывать оба слова: end={}",
            segs[0].end
        );
        assert!(
            segs[0].start <= 0.52,
            "Сегмент должен начинаться с первого слова: start={}",
            segs[0].start
        );
    }
}
