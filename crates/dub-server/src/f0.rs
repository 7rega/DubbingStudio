//! F0 (основная частота) по WAV-сэмплам — автокорреляция во временной области (#114). Нужна для
//! определения ПОЛА спикера при авто-распределении голосов-слотов: медиана F0 его реплик на чистом
//! вокале. Лёгкий детектор без внешних зависимостей: фреймы ~40мс шаг 20мс, voiced-гейт по нормированному
//! пику ACF и энергии, F0 = sr/lag лучшего пика в диапазоне 60-400 Гц; результат — медиана voiced-фреймов.

/// Диапазон человеческого голоса для лаг-поиска ACF (Гц). 60 Гц — низкий мужской бас, 400 Гц — высокий
/// женский/детский. Лаги вне диапазона не рассматриваем (октавные ошибки/шум).
const F0_MIN_HZ: f64 = 60.0;
const F0_MAX_HZ: f64 = 400.0;
/// Порог нормированного пика ACF: фрейм считается voiced, если пик r(lag)/r(0) выше — периодичность есть.
const VOICED_ACF: f64 = 0.5;
/// Порог энергии фрейма (RMS) — отсекаем тишину/шумовые хвосты, где ACF-пик случаен.
const VOICED_RMS: f64 = 0.01;
/// Пороги пола по медиане F0 (Гц): <155 -> male, >165 -> female, серая зона 155..=165 -> ближе к порогу.
const MALE_HZ: f64 = 155.0;
const FEMALE_HZ: f64 = 165.0;

/// Пол спикера по F0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,
}

/// Медиана F0 (Гц) по mono-сэмплам [-1,1] @ sr. None — нет voiced-фреймов (тишина/шёпот/только шум).
/// Фреймы 40мс, шаг 20мс; на каждом — автокорреляция в лаг-диапазоне [sr/F0_MAX, sr/F0_MIN], voiced-гейт
/// по нормированному пику ACF и RMS; F0 = sr/lag лучшего пика.
pub fn median_f0(samples: &[f32], sr: u32) -> Option<f64> {
    if sr == 0 {
        return None;
    }
    let sr_f = sr as f64;
    let frame = (sr_f * 0.040).round() as usize; // 40мс
    let hop = (sr_f * 0.020).round() as usize; // 20мс
    let min_lag = (sr_f / F0_MAX_HZ).floor() as usize;
    let max_lag = (sr_f / F0_MIN_HZ).ceil() as usize;
    if frame < 4 || min_lag < 1 || max_lag >= frame || samples.len() < frame {
        return None;
    }
    let mut f0s: Vec<f64> = Vec::new();
    let mut start = 0;
    while start + frame <= samples.len() {
        let win = &samples[start..start + frame];
        start += hop.max(1);
        if let Some(f0) = frame_f0(win, sr_f, min_lag, max_lag) {
            f0s.push(f0);
        }
    }
    if f0s.is_empty() {
        return None;
    }
    Some(median(&mut f0s))
}

/// F0 одного фрейма или None (не voiced). r(0) = энергия; лучший пик r(lag) в [min_lag,max_lag];
/// voiced если RMS выше порога И нормированный пик выше VOICED_ACF.
fn frame_f0(win: &[f32], sr: f64, min_lag: usize, max_lag: usize) -> Option<f64> {
    let n = win.len();
    // r(0) = сумма квадратов; RMS-гейт тишины.
    let r0: f64 = win.iter().map(|&x| (x as f64) * (x as f64)).sum();
    let rms = (r0 / n as f64).sqrt();
    if rms < VOICED_RMS || r0 <= 0.0 {
        return None;
    }
    // НОРМИРОВАННАЯ кросс-корреляция (#116, находка [1]): score(lag)=r(lag)/sqrt(E1·E2) — коэффициент в
    // [-1,1] без перекоса (сырая сумма r(lag) занижала низкие частоты -> октавная ошибка вверх; деление
    // на (n-lag) раздувало шум длинных лагов). Энергия-нормировка убирает оба перекоса.
    let hi = max_lag.min(n - 1);
    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(hi - min_lag + 1);
    let mut best_score = f64::MIN;
    for lag in min_lag..=hi {
        let (mut num, mut e1, mut e2) = (0.0f64, 0.0f64, 0.0f64);
        for i in lag..n {
            let (a, b) = (win[i] as f64, win[i - lag] as f64);
            num += a * b;
            e1 += a * a;
            e2 += b * b;
        }
        let denom = (e1 * e2).sqrt();
        let score = if denom > 0.0 { num / denom } else { 0.0 };
        scores.push((lag, score));
        if score > best_score {
            best_score = score;
        }
    }
    if best_score < VOICED_ACF {
        return None; // нет достаточно периодичного пика -> не voiced
    }
    // ФУНДАМЕНТАЛЬНЫЙ период = ВЕРШИНА ПЕРВОГО пика score выше порога. Нормировка одинаково оценивает и
    // КРАТНЫЕ периоды (2×,3× -> октава ВНИЗ, 220 Гц детектился как 73); берём первый лепесток, но его argmax
    // (не восходящий фронт — тот дал бы завышенную частоту), т.е. истинный период. Ложный РАННИЙ лепесток от
    // формант структурно исключён диапазоном лагов [60,400 Гц]: форманты (>500 Гц) и гармоники дают лаг <40 —
    // ниже min_lag, вне поиска (#116 [4]).
    let thr = (best_score * 0.9).max(VOICED_ACF);
    let first = scores.iter().position(|(_, s)| *s >= thr);
    let Some(mut i) = first else { return None };
    let mut best_lag = scores[i].0;
    let mut lobe_max = scores[i].1;
    while i < scores.len() && scores[i].1 >= thr {
        if scores[i].1 > lobe_max {
            lobe_max = scores[i].1;
            best_lag = scores[i].0;
        }
        i += 1;
    }
    Some(sr / best_lag as f64)
}

/// Пол по медиане F0. Серая зона 155..=165 Гц: ближе к какому порогу (расстояние до MALE_HZ vs FEMALE_HZ).
pub fn gender_of(f0: f64) -> Gender {
    if f0 < MALE_HZ {
        Gender::Male
    } else if f0 > FEMALE_HZ {
        Gender::Female
    } else if (f0 - MALE_HZ) <= (FEMALE_HZ - f0) {
        Gender::Male
    } else {
        Gender::Female
    }
}

/// Медиана среза f64 (сортирует на месте). Вызывающий гарантирует непустоту.
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Синтетическая синусоида freq Гц, dur сек @ sr — сэмплы в [-1,1].
    fn sine(freq: f64, dur: f64, sr: u32) -> Vec<f32> {
        let n = (dur * sr as f64) as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / sr as f64).sin() as f32 * 0.8)
            .collect()
    }

    #[test]
    fn f0_of_sine_110_and_220() {
        let sr = 16_000;
        let a = median_f0(&sine(110.0, 1.0, sr), sr).expect("voiced 110");
        assert!((a - 110.0).abs() < 5.0, "110Гц -> {a}");
        let b = median_f0(&sine(220.0, 1.0, sr), sr).expect("voiced 220");
        assert!((b - 220.0).abs() < 5.0, "220Гц -> {b}");
    }

    #[test]
    fn gender_from_f0() {
        assert_eq!(gender_of(110.0), Gender::Male); // низкий мужской
        assert_eq!(gender_of(220.0), Gender::Female); // высокий женский
        assert_eq!(gender_of(150.0), Gender::Male); // ниже серой зоны
        assert_eq!(gender_of(200.0), Gender::Female);
        // серая зона: 156 ближе к 155 (male), 164 ближе к 165 (female).
        assert_eq!(gender_of(156.0), Gender::Male);
        assert_eq!(gender_of(164.0), Gender::Female);
    }

    #[test]
    fn sine_110_is_male_220_is_female() {
        let sr = 16_000;
        let m = median_f0(&sine(110.0, 1.0, sr), sr).unwrap();
        let f = median_f0(&sine(220.0, 1.0, sr), sr).unwrap();
        assert_eq!(gender_of(m), Gender::Male);
        assert_eq!(gender_of(f), Gender::Female);
    }

    #[test]
    fn silence_has_no_f0() {
        let sr = 16_000;
        assert!(median_f0(&vec![0.0f32; sr as usize], sr).is_none());
    }

    #[test]
    fn empty_and_zero_sr() {
        assert!(median_f0(&[], 16_000).is_none());
        assert!(median_f0(&sine(120.0, 0.5, 16_000), 0).is_none());
    }
}
