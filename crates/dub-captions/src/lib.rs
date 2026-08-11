//! dub-captions — CapCut-стиль вжжёные субтитры через ffmpeg + libass (ASS). Дословный порт
//! dubengine/captions.py: build() (ОДИН ASS с ТИТРАМИ localized-in-place + нашими дублированными
//! субтитрами) и burn()/burn_frame() (gblur боксов оригинального текста + оверлей ASS, NVENC).
//!
//! Реверсы/плашки/26 пресетов — модуль look; эмиттеры (плашка/стилизованный субтитр/титр) — ass;
//! метрики глифов (замена PIL) — font. Строки ASS формируются один-в-один с питоном.

mod ass;
mod burn;
mod font;
mod look;
mod timing_qc;
mod types;

pub use burn::{burn, burn_frame};
pub use look::lum;
pub use look::{DEFAULT_PRESET, DEFAULT_TEMPLATE, FRESH_DEFAULT, FONT_NAME};
pub use types::{BlurBox, Sub, SubStyle, Title};

// Индустриальные правила субтитров (#86) — опциональный timing-QC пасс (RulesProfile/CalcLength/
// балансный перенос/нормализаторы длительности). Чистые pure-fn, движки не трогают.
pub use timing_qc::{
    balanced_wrap, bridge_gaps, chars_per_second, clamp_durations, merge_short_lines, normalize,
    optimal_display_ms, violates_cps, CalcAll, CalcArabic, CalcCjk, CalcKind, CalcLength,
    NormalizeOpts, RulesProfile, TimingSeg, DEFAULT_PROFILE, PROFILES,
};

use std::path::Path;

/// Габарит нарисованного дублированного субтитра (по фактической геометрии text_geom + паддинг) для
/// блюр-подложки ПОД нашим текстом. Отдаётся из build(), когда стиль без непрозрачной плашки (иначе
/// плашка сама кроет фон) — render добавляет её к блюр-боксам, чтобы новый текст лёг на размытый фон,
/// а не половиной на чистое видео. Старые band-боксы (прячут ОРИГИНАЛ) при этом остаются нетронутыми.
#[derive(Clone, Debug)]
pub struct SubCover {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub t0: f64,
    pub t1: f64,
}

/// Параметры build (порт сигнатуры captions.build). preset — имя PRESET (BorderStyle-плашка); titles/
/// subs — контент; sub_y/sub_style/caption_style/… — стиль; sub_px — измеренный размер оригинала.
#[derive(Default)]
pub struct BuildArgs<'a> {
    pub preset: Option<&'a str>,
    pub titles: &'a [Title],
    pub subs: &'a [Sub],
    pub max_lines: i64,
    pub sub_y: Option<i64>,
    pub sub_style: Option<&'a SubStyle>,
    pub caption_style: Option<&'a str>,
    pub caption_plate: Option<&'a str>,
    pub caption_reveal: Option<&'a str>,
    pub caption_font: Option<&'a str>,
    pub sub_px: Option<i64>,
}

/// Установить каталог bundled-шрифтов (для измерения + fontsdir libass). Вызывать до build/burn.
pub fn set_fonts_dir(dir: impl AsRef<Path>) {
    ass::set_fonts_dir(dir);
}

/// FONTS-каталог (family -> описание) в порядке питона — для GET /fonts. Порт dict(captions.FONTS).
pub fn fonts_catalog() -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in look::FONTS_ORDERED {
        m.insert(k.to_string(), serde_json::Value::from(v));
    }
    m
}

/// TEMPLATES-каталог (name -> {reveal,plate,font,base,plate_c?,accent?}) + список REVEALS — для
/// GET /presets. Порт {k: dict(v)} + list(REVEALS) из app.py. fresh-семейство БЕЗ plate_c (как питон).
pub fn presets_catalog() -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
    let mut presets = serde_json::Map::new();
    for name in look::TEMPLATE_NAMES {
        if let Some(t) = look::template(name) {
            let mut d = serde_json::Map::new();
            d.insert("reveal".into(), serde_json::Value::from(t.reveal));
            d.insert("plate".into(), serde_json::Value::from(t.plate));
            d.insert("font".into(), serde_json::Value::from(t.font));
            d.insert("base".into(), serde_json::Value::from(t.base));
            if !look::template_omits_plate_c(name) {
                d.insert("plate_c".into(), serde_json::Value::from(t.plate_c));
            }
            if let Some(a) = t.accent {
                d.insert("accent".into(), serde_json::Value::from(a));
            }
            presets.insert(name.to_string(), serde_json::Value::Object(d));
        }
    }
    let reveals: Vec<String> = look::REVEALS.iter().map(|s| s.to_string()).collect();
    (presets, reveals)
}

/// Собрать ОДИН ASS с титрами + дублированными субтитрами и записать в out_ass. Порт captions.build.
/// Возвращает габариты подложек под нашими субтитрами (для блюр-подложки на рендере; пусто, если стиль
/// сам даёт непрозрачную плашку или сцена плоская с cover-крышкой).
pub fn build(width: i64, height: i64, out_ass: &Path, mut args: BuildArgs) -> Result<Vec<SubCover>, String> {
    if args.max_lines <= 0 {
        args.max_lines = 2;
    }
    let mut max_lines = args.max_lines;

    // sub_style по умолчанию (нет ни диалог-субтитра, ни on-screen капшенов) — белый+outline в шрифте
    // титров. Порт ветки `if not sub_style`.
    let default_style;
    let sub_style: Option<&SubStyle> = if args.sub_style.is_none() {
        let tf: Vec<String> = args
            .titles
            .iter()
            .filter_map(|t| t.font.clone())
            .filter(|f| look::is_known_font(f))
            .collect();
        let itl: Vec<bool> = args.titles.iter().map(|t| t.italic).collect();
        let font = if tf.is_empty() {
            None
        } else {
            // max по частоте
            Some(mode_string(&tf))
        };
        let italic = if itl.is_empty() {
            false
        } else {
            itl.iter().filter(|&&x| x).count() as f64 > itl.len() as f64 / 2.0
        };
        default_style = SubStyle {
            color: "#FFFFFF".to_string(),
            background: Some("none".to_string()),
            solid: false,
            align: "center".to_string(),
            bold: true,
            italic,
            font,
            ..SubStyle::default()
        };
        Some(&default_style)
    } else {
        args.sub_style
    };

    // style = preset если валиден, иначе первый из ROTATION (детерминированно, без random — паритет
    // держим по видимому результату; питон брал random.choice(ROTATION), но при заданном sub_style
    // ветка S-style ниже НЕ использует p, а при отсутствии sub_style дефолт выше его задаёт).
    let style_name = args.preset.filter(|p| look::preset(p).is_some()).unwrap_or(look::ROTATION[0]);
    let p = look::preset(style_name).unwrap();

    // sub_fs: размер оригинала (sub_px*1.25) либо size_frac*height либо height/16, клампы. Порт captions.py:496-501.
    let szf = sub_style.and_then(|s| s.size_frac);
    // size_px==0 -> auto-fit (питон: `if _explicit:` — 0 falsy), НЕ «явные 20».
    let explicit = sub_style.and_then(|s| s.size_px).filter(|&e| e > 0);
    let h5 = (height as f64 / 5.0).round() as i64;
    let h10 = (height as f64 / 10.0).round() as i64;
    let sub_fs: i64 = if let Some(e) = explicit {
        e.min(h5).max(20) // max(20, min(size_px, height/5))
    } else {
        let seed = if let Some(px) = args.sub_px {
            (px as f64 * 1.25).round() as i64
        } else if let Some(sf) = szf {
            (sf * height as f64).round() as i64
        } else {
            (height as f64 / 16.0).round() as i64
        };
        // min(max(44, seed), height/10): frame-cap выигрывает на МАЛОМ кадре (h10<44) -> без паники clamp.
        seed.max(44).min(h10)
    };
    let margin_v = (height as f64 * 0.13).round() as i64;

    let fontname = sub_style
        .and_then(|s| s.font.clone())
        .filter(|f| look::is_known_font(f))
        .unwrap_or_else(|| look::FONT_NAME.to_string());

    // S-style строка (порт всех веток if sub_style / elif / else). plate_opaque -> плашка уже кроет фон.
    let (s_style, plate_opaque) = build_s_style(&fontname, sub_fs, margin_v, sub_style, &p);

    // Направленная тень субтитра (порт captions.py:593): вычисляем ОДИН раз, префиксом к каждой S-строке.
    // dist = outline_w редактора либо max(2, sub_fs*0.06); цвет = outline редактора либо чёрный.
    let subdir = {
        let sd = sub_style.and_then(|s| s.shadow_dir);
        let dist = sub_style
            .and_then(|s| s.outline_w)
            .unwrap_or(((sub_fs as f64 * 0.06) as i64).max(2));
        let col = sub_style
            .and_then(|s| s.outline.as_deref())
            .filter(|o| !o.is_empty()) // питон truthy-guard: пустая строка outline -> чёрный дефолт, не белый
            .map(|o| look::c6(&look::hex_ass(o)))
            .unwrap_or_else(|| "&H000000&".to_string());
        look::dir_shadow_tags(sd, dist, &col)
    };
    let subdir_tag = if subdir.is_empty() { String::new() } else { format!("{{{subdir}}}") };

    // head (Script Info + V4+ Styles: T/S/KP/KT).
    let head = build_head(width, height, &s_style, sub_fs, &p);
    let mut lines: Vec<String> = vec![head];

    // 1) титры localized in place.
    for b in args.titles {
        ass::emit_title(&mut lines, b, width, height);
    }

    // 2) наши дублированные субтитры.
    if let Some(nl) = sub_style.and_then(|s| s.n_lines) {
        max_lines = nl.clamp(1, 3);
    }
    let block_half = max_lines as f64 * sub_fs as f64 * 0.75;
    let clampy = |y: f64| -> i64 {
        (y.max(height as f64 * 0.04))
            .min(height as f64 - block_half - height as f64 * 0.04) as i64
    };
    let max_chars = ((width as f64 / (sub_fs as f64 * 0.52)) as i64).max(10) as usize;

    // vis: отсортированные по start, непустой tgt.
    let mut vis: Vec<(f64, f64, String, Option<i64>)> = args
        .subs
        .iter()
        .map(|s| (s.start, s.end, ass::esc(s.tgt.trim()).trim().to_string(), s.y))
        .collect();
    vis.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    vis.retain(|v| !v.2.is_empty());

    let look_resolved = look::resolve_look(
        args.caption_style,
        args.caption_plate,
        args.caption_reveal,
        args.caption_font,
        sub_style.map(|s| s.color.as_str()),
    );

    // cover_c (captions.py 583-589): невидимая крышка цвета сцены; глушится принудительной плашкой.
    let ss_scene = sub_style.and_then(|s| s.scene_color.clone());
    let cover_c: Option<String> = ss_scene.filter(|sc| {
        sub_style
            .map(|s| {
                s.scene_flat
                    && look::lum(&s.color) > 0.45
                    && s.background.as_deref().map(|b| b == "none").unwrap_or(true)
                    && s.plate != Some(true)
            })
            .unwrap_or(false)
            && !sc.is_empty()
    });

    // измерять heavy-весом (<Family>-Bold.ttf если есть).
    let bold_file = ass::fonts_dir().join(format!("{}-Bold.ttf", fontname.replace(' ', "")));
    let sub_fp = if bold_file.exists() {
        bold_file
    } else {
        ass::font_path_for(&fontname)
    };
    let up = sub_style.map(|s| s.uppercase).unwrap_or(false);

    // Блюр-подложку ПОД нашим текстом собираем, только когда стиль сам НЕ даёт непрозрачную плашку
    // (plate_opaque) и нет cover-крышки плоской сцены — иначе фон и так закрыт. Пресеты (look_resolved)
    // несут собственную плашку/reveal. Старые band-боксы (прячут оригинал) остаются отдельно.
    let needs_cover = !plate_opaque && look_resolved.is_none() && cover_c.is_none();
    let mut covers: Vec<SubCover> = Vec::new();

    let n = vis.len();
    for idx in 0..n {
        let st = vis[idx].0;
        let mut en = vis[idx].1;
        let sy = vis[idx].3;
        let tgt = &vis[idx].2; // ссылка вместо клона всего кортежа (String клонируем лишь когда нужно)
        if idx + 1 < n {
            en = en.min(vis[idx + 1].0); // не перекрывать следующий
        }
        if en - st < 0.08 {
            continue;
        }
        let yy: Option<i64> = match (sy, args.sub_y) {
            (Some(v), _) => Some(clampy(v as f64)),
            (None, Some(v)) => Some(clampy(v as f64)),
            _ => None,
        };
        let src_txt = if up { tgt.to_uppercase() } else { tgt.clone() };
        let chunks = font::wrap_chars(&src_txt, max_chars);
        // groups = chunks по max_lines
        let mut groups: Vec<Vec<String>> = Vec::new();
        let ml = max_lines as usize;
        let mut i = 0;
        while i < chunks.len() {
            groups.push(chunks[i..(i + ml).min(chunks.len())].to_vec());
            i += ml;
        }
        if groups.is_empty() {
            groups.push(vec![tgt.clone()]);
        }
        let per = (en - st) / groups.len() as f64;
        for (gi, g) in groups.iter().enumerate() {
            let a = st + gi as f64 * per;
            let b = if gi == groups.len() - 1 {
                en
            } else {
                st + (gi as f64 + 1.0) * per
            };
            if let Some(lk) = &look_resolved {
                let cy = yy.unwrap_or((height - margin_v - sub_fs) as i64);
                ass::emit_styled(&mut lines, lk, a, b, g, width / 2, cy, sub_fs, width, true);
            } else {
                // match-original -> S-style. FIT: ужать шрифт если строка переполняет.
                let mut fs_g = sub_fs;
                let (mut iw, mut tr, mut br) = font::text_geom(g, fs_g, &sub_fp);
                while iw > width as f32 * 0.92 && fs_g > 30 {
                    fs_g = ((fs_g as f32 * 0.94) as i64).max(30);
                    let gg = font::text_geom(g, fs_g, &sub_fp);
                    iw = gg.0;
                    tr = gg.1;
                    br = gg.2;
                }
                let ovr = if fs_g != sub_fs {
                    format!("\\fs{fs_g}")
                } else {
                    String::new()
                };
                let ptag = if let Some(y) = yy {
                    format!("{{\\an5\\pos({},{}){}}}", width / 2, y, ovr)
                } else if !ovr.is_empty() {
                    format!("{{{ovr}}}")
                } else {
                    String::new()
                };
                let body = g.join("\\N");
                if let (Some(cc), Some(y)) = (&cover_c, yy) {
                    let padx = (fs_g as f32 * 0.5) as i64;
                    let pady = (fs_g as f32 * 0.6) as i64;
                    let x0 = ((width / 2) as f32 - iw / 2.0 - padx as f32).max(6.0) as i64;
                    let x1 = ((width / 2) as f32 + iw / 2.0 + padx as f32).min(width as f32 - 6.0) as i64;
                    let y0 = (y as f32 + tr - pady as f32) as i64;
                    let y1 = (y as f32 + br + pady as f32) as i64;
                    let rr = (((y1 - y0) as f64 * 0.14) as i64).max(4);
                    lines.push(format!(
                        "Dialogue: 0,{},{},KP,,0,0,0,,{{\\an7\\pos(0,0)\\1c{}\\bord0\\shad0\\p1}}{}",
                        ass::ts(a),
                        ass::ts(b),
                        look::c6(&look::hex_ass(cc)),
                        ass::round_rect(x0 as f64, y0 as f64, x1 as f64, y1 as f64, rr as f64)
                    ));
                }
                lines.push(format!(
                    "Dialogue: 1,{},{},S,,0,0,0,,{ptag}{subdir_tag}{body}",
                    ass::ts(a),
                    ass::ts(b)
                ));
                // блюр-подложка РОВНО по габариту нарисованного текста (тот же fs_g/iw/tr/br, что и рендер).
                if needs_cover {
                    if let Some(y) = yy {
                        let padx = (fs_g as f32 * 0.34) as i64;
                        let pady = (fs_g as f32 * 0.30) as i64;
                        let x0 = (((width / 2) as f32) - iw / 2.0 - padx as f32).max(2.0) as i64;
                        let x1 = (((width / 2) as f32) + iw / 2.0 + padx as f32).min(width as f32 - 2.0) as i64;
                        let y0 = (((y as f32) + tr - pady as f32).max(0.0)) as i64;
                        let y1 = (((y as f32) + br + pady as f32).min(height as f32)) as i64;
                        if x1 > x0 && y1 > y0 {
                            covers.push(SubCover { x: x0, y: y0, w: x1 - x0, h: y1 - y0, t0: a, t1: b });
                        }
                    }
                }
            }
        }
    }

    std::fs::write(out_ass, lines.join("\n")).map_err(|e| format!("запись ASS: {e}"))?;
    Ok(covers)
}

/// Наиболее частая строка (замена max(vals, key=vals.count)).
fn mode_string(vals: &[String]) -> String {
    let mut best = &vals[0];
    let mut best_c = 0usize;
    for v in vals {
        let c = vals.iter().filter(|x| *x == v).count();
        if c > best_c {
            best_c = c;
            best = v;
        }
    }
    best.clone()
}

/// Построить S-style строку (порт всех веток if sub_style / elif bg / elif light / else / no-style).
/// Возвращает (строка стиля, plate_opaque) — plate_opaque=true, если стиль сам даёт НЕПРОЗРАЧНУЮ плашку
/// (BorderStyle=3), которая уже кроет фон под текстом (тогда отдельная блюр-подложка не нужна).
fn build_s_style(
    fontname: &str,
    sub_fs: i64,
    margin_v: i64,
    sub_style: Option<&SubStyle>,
    p: &look::PresetColors,
) -> (String, bool) {
    let Some(ss) = sub_style else {
        return (format!(
            "Style: S,{fontname},{sub_fs},{},&H000000FF,{},{},-1,0,0,0,100,100,0,0,3,11,0,2,80,80,{margin_v},1",
            p.primary, p.outline_c, p.back
        ), true);
    };
    let txt_hex = ss.color.clone();
    let col = look::hex_ass(&txt_hex);
    let bold = if ss.bold { -1 } else { 0 };
    let ital = if ss.italic { -1 } else { 0 };
    let bg = ss.background.clone();
    let sow = ss.outline_w;
    // Явный контрастный bg от vision (полоса в оригинале) — приоритетнее продуктовой подложки.
    let vision_band = bg
        .as_deref()
        .map(|b| b != "none" && (look::lum(b) - look::lum(&txt_hex)).abs() >= 0.20)
        .unwrap_or(false);
    // plate=Some(true) — принудительная плашка из редактора; иначе стиль и цвета от vision.
    let plate_on = ss.plate == Some(true) && !vision_band && sow.is_none();
    if let Some(w) = sow {
        let soc = look::hex_ass(
            ss.outline
                .clone()
                .unwrap_or_else(|| {
                    if look::lum(&txt_hex) > 0.45 {
                        "#000000".to_string()
                    } else {
                        "#FFFFFF".to_string()
                    }
                })
                .as_str(),
        );
        // outline-only (BorderStyle=1) -> плашки нет, нужна блюр-подложка под текстом.
        (format!(
            "Style: S,{fontname},{sub_fs},{col},&H000000FF,{soc},&H64000000,{bold},{ital},0,0,100,100,0,0,1,{},2,2,80,80,{margin_v},1",
            w.max(0)
        ), false)
    } else if vision_band {
        (format!(
            "Style: S,{fontname},{sub_fs},{col},&H000000FF,{},&H00000000,{bold},{ital},0,0,100,100,0,0,3,11,0,2,80,80,{margin_v},1",
            look::hex_ass(bg.as_deref().unwrap())
        ), true)
    } else if look::lum(&txt_hex) > 0.45 {
        if plate_on {
            // Принудительная сплошная плашка plate_color (BorderStyle=3/Outline=11).
            let plate_hex = ss.plate_color.clone().unwrap_or_else(|| "#000000".to_string());
            (format!(
                "Style: S,{fontname},{sub_fs},{col},&H000000FF,{},&H00000000,{bold},{ital},0,0,100,100,0,0,3,11,0,2,80,80,{margin_v},1",
                look::hex_ass(&plate_hex)
            ), true)
        } else {
            // Светлый текст -> тонкая чёрная обводка (captions.py 530-534). Плашки нет -> блюр-подложка.
            let bord = ((sub_fs as f64 * 0.025).round() as i64).max(2);
            (format!(
                "Style: S,{fontname},{sub_fs},{col},&H000000FF,&H00000000,&H00000000,{bold},{ital},0,0,100,100,0,0,1,{bord},0,2,80,80,{margin_v},1"
            ), false)
        }
    } else {
        // Тёмный текст -> белая плашка (BorderStyle=3) сама кроет фон.
        (format!(
            "Style: S,{fontname},{sub_fs},{col},&H000000FF,&H00FFFFFF,&H00F2F2F2,{bold},{ital},0,0,100,100,0,0,3,10,0,2,80,80,{margin_v},1"
        ), true)
    }
}

/// Построить head (Script Info + V4+ Styles) — порт head-строки captions.build.
fn build_head(width: i64, height: i64, s_style: &str, sub_fs: i64, p: &look::PresetColors) -> String {
    let t_fs = ((height as f64 / 22.0).round() as i64).max(24);
    let kt_bord = ((sub_fs as f64 * 0.11).round() as i64).max(2);
    format!(
        "[Script Info]\nScriptType: v4.00+\n\
PlayResX: {width}\nPlayResY: {height}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, \
Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, \
Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: T,{FN},{t_fs},{prim},&H000000FF,{outl},{back},-1,0,0,0,100,100,0,0,3,12,0,5,40,40,40,1\n\
{s_style}\n\
Style: KP,{FN},{sub_fs},&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\n\
Style: KT,{FN},{sub_fs},&H00FFFFFF,&H000000FF,&H00101010,&H64000000,-1,0,0,0,100,100,0,0,1,{kt_bord},2,5,40,40,40,1\n\n\
[Events]\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        FN = look::FONT_NAME,
        prim = p.primary,
        outl = p.outline_c,
        back = p.back,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_str(w: i64, h: i64, ss: &SubStyle, subs: &[Sub]) -> String {
        // уникальное имя на вызов: тесты идут параллельно, общий файл -> гонка/склейка выводов.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let uniq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("dubcap_test_{}_{}.ass", std::process::id(), uniq));
        let args = BuildArgs {
            subs,
            sub_style: Some(ss),
            sub_y: Some((h as f64 * 0.82) as i64),
            ..Default::default()
        };
        build(w, h, &dir, args).unwrap();
        let s = std::fs::read_to_string(&dir).unwrap();
        let _ = std::fs::remove_file(&dir);
        s
    }

    fn s_style_line(ass: &str) -> String {
        ass.lines().find(|l| l.starts_with("Style: S,")).unwrap().to_string()
    }

    fn subs1() -> Vec<Sub> {
        vec![Sub { start: 1.0, end: 3.0, tgt: "привет мир".into(), y: Some(600) }]
    }

    // Реальная контрастная полоса (vision.background=hex) -> BorderStyle=3, Outline=11 = НЕПРОЗРАЧНАЯ
    // плашка цвета полосы (прячет блюр). Порт elif-ветки captions.py 507-510.
    #[test]
    fn vision_band_gives_border3_plate() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("#101010".into()), // тёмная полоса под белым текстом (контраст >=0.20)
            bold: true,
            ..Default::default()
        };
        let ass = build_str(720, 1280, &ss, &subs1());
        let s = s_style_line(&ass);
        // OutlineColour слот = цвет полосы (BBGGRR of #101010), BorderStyle=3, Outline=11.
        assert!(s.contains("&H00101010"), "плашка должна нести цвет полосы: {s}");
        assert!(
            s.contains(",3,11,0,2,"),
            "BorderStyle=3 (boxed plate) Outline=11 ожидается: {s}"
        );
    }

    // Тёмный текст без полосы -> BorderStyle=3 с почти-белой плашкой (тёмные буквы читаются). Порт
    // else-ветки captions.py 516-518. Тоже НЕПРОЗРАЧНАЯ плашка -> блюр накрыт.
    #[test]
    fn dark_text_no_bg_gives_white_plate() {
        let ss = SubStyle {
            color: "#101010".into(),
            background: Some("none".into()),
            ..Default::default()
        };
        let ass = build_str(720, 1280, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(
            s.contains(",3,10,0,2,"),
            "тёмный текст -> BorderStyle=3 Outline=10 (белая плашка): {s}"
        );
        assert!(s.contains("&H00FFFFFF,&H00F2F2F2"), "OutlineColour белый + BackColour: {s}");
    }

    // Светлый box-less текст + плоская сцена -> KP-крышка цвета сцены (cover_c, captions.py 583-589).
    #[test]
    fn flat_scene_emits_cover_plate_kp_event() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("none".into()),
            scene_color: Some("#E0E0E0".into()),
            scene_flat: true,
            bold: true,
            ..Default::default()
        };
        let ass = build_str(720, 1280, &ss, &subs1());
        // должна быть хотя бы одна плашка-плитка на Layer 0 стиля KP, цвета сцены (#E0E0E0 -> E0E0E0).
        let kp = ass
            .lines()
            .find(|l| l.starts_with("Dialogue: 0,") && l.contains(",KP,") && l.contains("\\p1"));
        assert!(kp.is_some(), "ожидалась cover-плашка KP при scene_flat: \n{ass}");
        assert!(kp.unwrap().contains("&HE0E0E0&"), "cover-плашка цвета сцены: {}", kp.unwrap());
    }

    // Принудительная плашка непрозрачна — KP-крышка при ней не эмитится даже на плоской сцене.
    #[test]
    fn forced_plate_suppresses_cover_kp() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("none".into()),
            scene_color: Some("#FFFFFF".into()),
            scene_flat: true,
            bold: true,
            plate: Some(true),
            ..Default::default()
        };
        let ass = build_str(720, 1280, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(s.contains(",3,11,0,2,"), "принудительная плашка BorderStyle=3 ожидается: {s}");
        assert!(
            !ass.lines().any(|l| l.starts_with("Dialogue: 0,") && l.contains(",KP,")),
            "при принудительной плашке cover-KP быть не должно:\n{ass}"
        );
    }

    // Светлый box-less текст на текстурной сцене -> тонкая обводка BorderStyle=1, без KP-крышки.
    #[test]
    fn textured_scene_python_outline_no_cover_kp() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("none".into()),
            scene_color: Some("#E0E0E0".into()),
            scene_flat: false,
            bold: true,
            ..Default::default()
        };
        let ass = build_str(720, 1280, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(s.contains(",1,2,0,2,"), "питоновская outline-ветка BorderStyle=1 ожидается: {s}");
        assert!(
            !ass.lines().any(|l| l.starts_with("Dialogue: 0,") && l.contains(",KP,")),
            "на текстурной сцене cover-плашки (KP) быть не должно"
        );
    }

    // Живой greedy-вход example_original.mp4 -> BorderStyle=1 тонкая обводка (captions.py 530-534).
    #[test]
    fn greedy_example_original_python_outline_default() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("none".into()),
            outline: Some("none".into()),
            bold: true,
            italic: false,
            uppercase: true,
            align: "center".into(),
            font: Some("Oswald".into()),
            n_lines: Some(2),
            size_frac: Some(0.08),
            scene_color: Some("#E0E0E0".into()),
            scene_flat: false,
            solid: false,
            ..Default::default()
        };
        let ass = build_str(464, 824, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(
            s.contains(",1,2,0,2,"),
            "дефолт = питоновская outline-ветка BorderStyle=1: {s}"
        );
    }

    // plate=Some(false) -> BorderStyle=1 outline-ветка.
    #[test]
    fn plate_disabled_falls_back_to_border1() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("none".into()),
            bold: true,
            uppercase: true,
            font: Some("Oswald".into()),
            plate: Some(false),
            ..Default::default()
        };
        let ass = build_str(464, 824, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(
            s.contains(",1,2,0,2,"),
            "plate=false -> BorderStyle=1 Outline=2 (outline-only, питоновская ветка): {s}"
        );
    }

    // Обратная сторона якоря: КОГДА vision читает сплошную КОНТРАСТНУЮ полосу (bg=solid hex) — как
    // было в эталоне example_dub.mp4 — порт ОБЯЗАН дать чёрную BorderStyle=3 плашку. Это ветка,
    // воспроизводящая заливку полос эталона (доминанта #000000-класс). Проверено py-прогоном
    // py_build.py case A -> A_solid_black.ass (BorderStyle=3, Outline=11, &H00000000-плита).
    #[test]
    fn solid_band_reproduces_reference_black_plate() {
        let ss = SubStyle {
            color: "#FFFFFF".into(),
            background: Some("#000000".into()),
            bold: true,
            uppercase: true,
            font: Some("Oswald".into()),
            ..Default::default()
        };
        let ass = build_str(464, 824, &ss, &subs1());
        let s = s_style_line(&ass);
        assert!(
            s.contains(",3,11,0,2,") && s.contains("&H00000000"),
            "сплошная полоса -> BorderStyle=3 Outline=11 чёрная плита (заливка эталона): {s}"
        );
    }
}
