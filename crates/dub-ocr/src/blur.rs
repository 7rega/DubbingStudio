//! Порт правил превращения OCR-детекций в блюр-прямоугольники (dubengine/pipeline.py).
//!
//! Портированы САМОДОСТАТОЧНЫЕ, «числовые» преобразования, не зависящие от Gemma-vision/перевода:
//!  - centered-straddle гейт: бокс должен пересекать центральную зону кадра (40..60% ширины),
//!    иначе это боковая вывеска сцены (CHIYA/BAKERY) — не сабтайтл (pipeline.py ~415-420);
//!  - band_blur: покадровые детекции сабов коалесцируются в бокс-спаны по IoU>=0.5 и тайм-гейту
//!    1.6*dt, затем паддинг (-6,-4,+12,+8) и время (t0 .. t1+dt) (pipeline.py ~416-442).
//!
//! Формат блюр-бокса совпадает с project.json: (x, y, w, h, start, end) целочисленные координаты.

/// Покадровая детекция для band-блюра: (x, y, w, h, t).
pub type Det = (f32, f32, f32, f32, f32);
/// Блюр-прямоугольник project.json: (x, y, w, h, start, end).
pub type BlurBox = (i32, i32, i32, i32, f32, f32);

/// Центральная зона кадра по ширине: [0.40*vw, 0.60*vw]. Бокс проходит гейт, если straddle-ит её.
pub fn centered_zone(vw: f32) -> (f32, f32) {
    (0.40 * vw, 0.60 * vw)
}

/// Бокс (x, w) пересекает центральную зону (порт _centered / straddle-гейта).
pub fn straddles_center(x: f32, w: f32, vw: f32) -> bool {
    let (lo, hi) = centered_zone(vw);
    x < hi && x + w > lo
}

/// Frame-accurate band blur (порт pipeline.py band_blur). `dets` — покадровые детекции сабов,
/// уже отфильтрованные по центральной зоне. fps -> dt=1/fps. Коалесцируем последовательные
/// детекции на «одном месте» (IoU>=0.5 и разрыв по времени <=1.6*dt) в один бокс-спан, затем
/// паддинг и время.
pub fn band_blur(mut dets: Vec<Det>, fps: u32) -> Vec<BlurBox> {
    let dt = 1.0 / fps.max(1) as f32;
    dets.sort_by(|a, b| a.4.partial_cmp(&b.4).unwrap());
    // ран: [x0, y0, x1, y1, t0, t_last]
    let mut runs: Vec<[f32; 6]> = Vec::new();
    for (x, y, w, h, t) in dets {
        let mut hit: Option<usize> = None;
        for (i, r) in runs.iter().enumerate() {
            let ix = (r[2]).min(x + w) - (r[0]).max(x);
            let iy = (r[3]).min(y + h) - (r[1]).max(y);
            let inter = ix.max(0.0) * iy.max(0.0);
            let union = (r[2] - r[0]) * (r[3] - r[1]) + w * h - inter;
            if t - r[5] <= 1.6 * dt && union > 0.0 && inter / union >= 0.5 {
                hit = Some(i);
                break;
            }
        }
        match hit {
            None => runs.push([x, y, x + w, y + h, t, t]),
            Some(i) => {
                let r = &mut runs[i];
                r[0] = r[0].min(x);
                r[1] = r[1].min(y);
                r[2] = r[2].max(x + w);
                r[3] = r[3].max(y + h);
                r[5] = t;
            }
        }
    }
    runs.into_iter()
        .map(|r| {
            let (x0, y0, x1, y1, t0, t1) = (r[0], r[1], r[2], r[3], r[4], r[5]);
            (
                (x0 - 6.0).max(0.0) as i32,
                (y0 - 4.0).max(0.0) as i32,
                (x1 - x0 + 12.0) as i32,
                (y1 - y0 + 8.0) as i32,
                round2(t0),
                round2(t1 + dt),
            )
        })
        .collect()
}

/// Округление секунд до 2 знаков (стабильные тайминги в project.json).
#[inline]
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// Прямой блюр-бокс из региона/детекции с паддингом (порт таглайн/leftover-веток: (x,y,w,h,t0,t1)).
/// Регион уже с pad от detect_regions; здесь просто приведение к целым для project.json.
pub fn region_blur_box(x: f32, y: f32, w: f32, h: f32, t0: f32, t1: f32) -> BlurBox {
    (
        x.max(0.0) as i32,
        y.max(0.0) as i32,
        w as i32,
        h as i32,
        round2(t0),
        round2(t1),
    )
}
