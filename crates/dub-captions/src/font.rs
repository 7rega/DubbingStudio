//! Метрики глифов TTF — замена PIL ImageFont в captions.py. ab_glyph даёт per-glyph bounding box и
//! ascent/descent, из которых воспроизводим `_wrap` (жадный перенос по ширине), `_text_geom` (ink-экстенты
//! блока строк относительно метрического центра — куда libass ставит \an5) и `_wrap_chars`.
//!
//! PIL-контракт, который повторяем:
//!  - `getbbox(s) -> (l,t,r,b)`; ширина = r-l. Мы берём ink-габариты глифов на строке.
//!  - `getmetrics() -> (ascent, descent)`; line-height lh = ascent+descent (для укладки многострочника).
//! Числа не обязаны совпадать с PIL до пикселя (другой растеризатор метрик), но соотношения и логика
//! идентичны, а размеры плашек ниже клампятся к кадру — визуальный паритет держится.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Кэш загруженных шрифтов по пути файла (загрузка TTF не бесплатна, а _text_geom зовётся много раз).
static FONT_CACHE: Mutex<Option<HashMap<PathBuf, &'static FontVec>>> = Mutex::new(None);

fn load_font(path: &Path) -> Option<&'static FontVec> {
    let mut guard = FONT_CACHE.lock().ok()?;
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(f) = map.get(path) {
        return Some(*f);
    }
    let bytes = std::fs::read(path).ok()?;
    let font = FontVec::try_from_vec(bytes).ok()?;
    // Утечка в 'static: шрифтов единицы, живут весь процесс — проще, чем таскать лайфтаймы в эмиттерах.
    let leaked: &'static FontVec = Box::leak(Box::new(font));
    map.insert(path.to_path_buf(), leaked);
    Some(leaked)
}

/// Ширина текста в пикселях при размере fs (сумма advance-ширин глифов). Аналог PIL getbbox()[2]-[0].
pub fn text_width(text: &str, fs: i64, fontpath: &Path) -> f32 {
    let Some(font) = load_font(fontpath) else {
        // Фолбэк-оценка (шрифт не найден): ~0.55*fs на символ — грубо, но код выше клампит к кадру.
        return text.chars().count() as f32 * fs as f32 * 0.55;
    };
    let scaled = font.as_scaled(PxScale::from(fs as f32));
    advance_width(&scaled, text)
}

fn advance_width<SF: ScaleFont<F>, F: Font>(sf: &SF, text: &str) -> f32 {
    let mut w = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let g = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, g);
        }
        w += sf.h_advance(g);
        prev = Some(g);
    }
    w
}

/// Жадный перенос по ширине max_w (px) при размере fs — порт `_wrap`. Возвращает список строк.
pub fn wrap(text: &str, fs: i64, max_w: f32, fontpath: &Path) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    // Шрифт резолвим ОДИН раз на перенос (был Mutex::lock+HashMap::get на каждое слово через text_width).
    let scaled = load_font(fontpath).map(|f| f.as_scaled(PxScale::from(fs as f32)));
    let width = |s: &str| -> f32 {
        match &scaled {
            Some(sf) => advance_width(sf, s),
            None => s.chars().count() as f32 * fs as f32 * 0.55, // тот же фолбэк, что в text_width
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in words {
        let test = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if width(&test) <= max_w || cur.is_empty() {
            cur = test;
        } else {
            lines.push(cur);
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

/// Жадный перенос по числу символов max_chars — порт `_wrap_chars`.
pub fn wrap_chars(text: &str, max_chars: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in words {
        if cur.chars().count() + w.chars().count() + 1 <= max_chars || cur.is_empty() {
            cur = if cur.is_empty() {
                w.to_string()
            } else {
                format!("{cur} {w}")
            };
        } else {
            lines.push(cur);
            cur = w.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

/// Ink-экстенты блока строк при размере fs — порт `_text_geom`. Возвращает (ink_w, ink_top, ink_bot)
/// ОТНОСИТЕЛЬНО метрического центра блока (куда libass \an5 ставит середину), как в питоне:
///   lh = ascent+descent; top0 = -lh*n/2; для каждой строки i её ink bbox (top/bottom) от baseline
///   смещается на top0 + i*lh + bbox.top .. +bbox.bottom.
pub fn text_geom(lines: &[String], fs: i64, fontpath: &Path) -> (f32, f32, f32) {
    let Some(font) = load_font(fontpath) else {
        // Фолбэк без шрифта: ширина по оценке, вертикаль по fs*n.
        let n = lines.len().max(1) as f32;
        let w = lines
            .iter()
            .map(|l| text_width(l, fs, fontpath))
            .fold(0.0f32, f32::max);
        let lh = fs as f32;
        return (w, -lh * n / 2.0, lh * n / 2.0);
    };
    let scaled = font.as_scaled(PxScale::from(fs as f32));
    let asc = scaled.ascent();
    let desc = -scaled.descent(); // ab_glyph descent отрицателен; PIL descent положителен
    let lh = asc + desc;
    let n = lines.len().max(1) as f32;
    let top0 = -lh * n / 2.0;
    let mut ink_w = 0.0f32;
    let mut ink_top = 1e9f32;
    let mut ink_bot = -1e9f32;
    for (i, line) in lines.iter().enumerate() {
        let s = if line.is_empty() { " " } else { line.as_str() };
        ink_w = ink_w.max(advance_width(&scaled, s));
        // line_ink_vspan отдаёт bbox от baseline; PIL getbbox — от верха ячейки. lt = верх ячейки,
        // baseline = lt + asc.
        let (bb_top, bb_bot) = line_ink_vspan(&scaled, s, asc, desc);
        let lt = top0 + i as f32 * lh;
        ink_top = ink_top.min(lt + asc + bb_top);
        ink_bot = ink_bot.max(lt + asc + bb_bot);
    }
    (ink_w, ink_top, ink_bot)
}

/// Вертикальный ink-диапазон строки от её baseline: (top, bottom), top<0 вверх. Берём максимум по
/// глифам из ab_glyph outline bounds; пусто (пробелы) -> метрики шрифта.
fn line_ink_vspan<SF: ScaleFont<F>, F: Font>(sf: &SF, text: &str, asc: f32, desc: f32) -> (f32, f32) {
    let mut top = 0.0f32;
    let mut bot = 0.0f32;
    let mut any = false;
    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        let gid = sf.glyph_id(c);
        let g = gid.with_scale(sf.scale());
        if let Some(og) = sf.font().outline_glyph(g) {
            let b = og.px_bounds();
            // px_bounds.y отсчитывается от позиции глифа (baseline на y=0, вверх отрицательно).
            top = top.min(b.min.y);
            bot = bot.max(b.max.y);
            any = true;
        }
    }
    if any {
        (top, bot)
    } else {
        (-asc, desc)
    }
}

/// Резолв пути к шрифту по имени family. Точный порт _font_path_for/_FONT_FILE: family -> файл в
/// fonts_dir; если нет — дефолтный FONT_PATH (Montserrat.ttf). Для \b1-измерения субтитра берётся
/// `<Family>-Bold.ttf` если существует.
pub fn font_file_for(family: &str) -> Option<&'static str> {
    Some(match family {
        "Montserrat" => "Montserrat.ttf",
        "Oswald" => "Oswald.ttf",
        "Roboto" => "Roboto.ttf",
        "Russo One" => "RussoOne-Regular.ttf",
        "Pacifico" => "Pacifico-Regular.ttf",
        "Playfair Display" => "PlayfairDisplay.ttf",
        "Caveat" => "Caveat.ttf",
        // Доп. шрифты (кириллица+латиница, добавлены 2026-07-16).
        "Noto Sans" => "NotoSans-Regular.ttf",
        "Fira Sans" => "FiraSans-Regular.ttf",
        "Rubik" => "Rubik-Regular.ttf",
        "Comfortaa" => "Comfortaa-Regular.ttf",
        "Exo 2" => "Exo2-Regular.ttf",
        "Philosopher" => "Philosopher-Regular.ttf",
        "Prata" => "Prata-Regular.ttf",
        "Yeseva One" => "YesevaOne-Regular.ttf",
        "Pangolin" => "Pangolin-Regular.ttf",
        "Marck Script" => "MarckScript-Regular.ttf",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ink-экстенты относительно метрического центра блока лежат внутри ячейки [top0, top0+n*lh].
    #[test]
    fn text_geom_ink_within_metric_cell() {
        let fp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fonts/Montserrat-Bold.ttf");
        assert!(fp.exists(), "нет шрифта для теста: {}", fp.display());
        let fs = 48i64;
        let font = load_font(&fp).expect("шрифт грузится");
        let scaled = font.as_scaled(PxScale::from(fs as f32));
        let lh = scaled.ascent() - scaled.descent();
        for lines in [vec!["ПРЯМО.".to_string()], vec!["Ag".into(), "Что".into()]] {
            let n = lines.len() as f32;
            let (iw, top, bot) = text_geom(&lines, fs, &fp);
            assert!(iw > 0.0);
            assert!(top < 0.0 && bot > 0.0, "ink должен обнимать центр: top={top} bot={bot}");
            assert!(
                top >= -lh * n / 2.0 - 1.0 && bot <= lh * n / 2.0 + 1.0,
                "ink вне метрической ячейки (сдвиг на ascent?): top={top} bot={bot} lh={lh} n={n}"
            );
        }
    }
}
