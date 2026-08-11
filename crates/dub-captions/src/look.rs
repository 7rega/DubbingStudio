//! Пресеты/шаблоны/палитра субтитров — дословный порт констант и цветовых хелперов captions.py.
//! PRESETS (4 CapCut-плашки) + ROTATION; TEMPLATES (26 готовых луков) + REVEALS/PLATES; FONTS
//! (Cyrillic-capable набор); _hex_ass/_lum/_c6/_is_vivid/_resolve_look/_FONT_SCALE.

use std::collections::HashMap;

/// Дефолтный рендер-шрифт libass (bundled, без Arial).
pub const FONT_NAME: &str = "Montserrat";

/// Реверсы (анимация появления текста) — порядок как в питоне. Публичный список для GET /presets.
#[allow(dead_code)]
pub const REVEALS: [&str; 5] = ["whole", "karaoke", "word", "pop", "highlight"];
/// Плашки (фон): box..glow непрозрачные (прячут блюр оригинала); none/soft — для FRESH-режима.
#[allow(dead_code)]
pub const PLATES: [&str; 8] = [
    "box", "rounded", "pill", "blob", "card", "glow", "none", "soft",
];

pub const DEFAULT_PRESET: &str = "boxed";
pub const FRESH_DEFAULT: &str = "fresh";
pub const DEFAULT_TEMPLATE: &str = "clean";

/// Пресет-плашка BorderStyle=3: primary=текст, outline_c=цвет плашки, back.
#[derive(Clone, Copy)]
pub struct PresetColors {
    pub primary: &'static str,
    pub outline_c: &'static str,
    pub back: &'static str,
}

/// PRESETS + ROTATION (порядок ротации важен — random.choice по ROTATION при неизвестном пресете).
pub fn preset(name: &str) -> Option<PresetColors> {
    Some(match name {
        "boxed" => PresetColors { primary: "&H00FFFFFF", outline_c: "&H00101010", back: "&H00000000" },
        "boxed_yellow" => PresetColors { primary: "&H00101010", outline_c: "&H0000C8FF", back: "&H00000000" },
        "boxed_blue" => PresetColors { primary: "&H00FFFFFF", outline_c: "&H00C05A18", back: "&H00000000" },
        "boxed_pink" => PresetColors { primary: "&H00FFFFFF", outline_c: "&H00B030D0", back: "&H00000000" },
        _ => return None,
    })
}
pub const ROTATION: [&str; 4] = ["boxed", "boxed_yellow", "boxed_blue", "boxed_pink"];

/// Cyrillic-capable шрифты, предлагаемые оркестратору (family -> описание). Latin-only (Anton/Bebas/…)
/// НЕ предлагаются (тихо падают в Arial на кириллице), хотя файлы есть в fonts/ для будущих Latin-дублей.
pub fn is_known_font(name: &str) -> bool {
    matches!(
        name,
        "Montserrat" | "Oswald" | "Roboto" | "Russo One" | "Pacifico" | "Playfair Display" | "Caveat"
            | "Noto Sans" | "Fira Sans" | "Rubik" | "Comfortaa" | "Exo 2" | "Philosopher"
            | "Prata" | "Yeseva One" | "Pangolin" | "Marck Script"
    )
}

/// Шаблон луковый (TEMPLATES): reveal/plate/font/base/plate_c/accent.
#[derive(Clone)]
pub struct Template {
    pub reveal: &'static str,
    pub plate: &'static str,
    pub font: &'static str,
    pub base: &'static str,
    pub plate_c: &'static str,
    pub accent: Option<&'static str>,
}

/// TEMPLATES — 26 готовых луков (дословно из captions.py).
pub fn template(name: &str) -> Option<Template> {
    let t = |reveal, plate, font, base, plate_c, accent| Template {
        reveal,
        plate,
        font,
        base,
        plate_c,
        accent,
    };
    Some(match name {
        // minimal / clean
        "clean" => t("whole", "pill", "Montserrat", "#FFFFFF", "#1A1A1A", None),
        "minimal" => t("whole", "rounded", "Roboto", "#FFFFFF", "#181818", None),
        "boxed" => t("whole", "box", "Montserrat", "#FFFFFF", "#101010", None),
        "headline" => t("whole", "box", "Oswald", "#FFFFFF", "#0C0C0C", None),
        "serif" => t("whole", "card", "Playfair Display", "#FFFFFF", "#16110D", None),
        "card" => t("whole", "card", "Montserrat", "#FFFFFF", "#16110D", None),
        // viral / bold
        "hormozi" => t("highlight", "box", "Russo One", "#FFFFFF", "#0C0C0C", Some("#FFD400")),
        "hormozi_green" => t("highlight", "box", "Russo One", "#FFFFFF", "#0C0C0C", Some("#28E0A8")),
        "mrbeast" => t("pop", "box", "Russo One", "#FFFFFF", "#0C0C0C", Some("#FFE000")),
        "impact" => t("highlight", "box", "Russo One", "#FFFFFF", "#101010", Some("#FF3B30")),
        "pop" => t("pop", "pill", "Oswald", "#FFFFFF", "#141414", None),
        // karaoke
        "karaoke" => t("karaoke", "pill", "Oswald", "#FFFFFF", "#181818", Some("#28E0A8")),
        "karaoke_gold" => t("karaoke", "box", "Montserrat", "#FFFFFF", "#101010", Some("#FFD400")),
        "karaoke_neon" => t("karaoke", "glow", "Montserrat", "#FFFFFF", "#0A0A14", Some("#00E5FF")),
        // playful
        "bubble" => t("whole", "blob", "Caveat", "#201018", "#FF5DA2", None),
        "bubble_pop" => t("pop", "blob", "Pacifico", "#201018", "#FFC857", None),
        "candy" => t("word", "pill", "Pacifico", "#2A0E1E", "#FF6FB5", None),
        // neon / night
        "neon" => t("whole", "glow", "Montserrat", "#00E5FF", "#0A0A14", Some("#00E5FF")),
        "neon_pink" => t("whole", "glow", "Oswald", "#FF54C8", "#100A14", Some("#FF54C8")),
        "cyber" => t("word", "glow", "Oswald", "#7DF9FF", "#07101A", Some("#00E5FF")),
        // FRESH (no solid back)
        "fresh" => t("whole", "none", "Montserrat", "#FFFFFF", "#101010", None),
        "fresh_bold" => t("pop", "none", "Russo One", "#FFFFFF", "#101010", None),
        "fresh_pop" => t("pop", "none", "Montserrat", "#FFFFFF", "#101010", None),
        "fresh_karaoke" => t("karaoke", "none", "Oswald", "#FFFFFF", "#101010", Some("#FFD400")),
        "fresh_hormozi" => t("highlight", "none", "Russo One", "#FFFFFF", "#101010", Some("#FFD400")),
        "fresh_soft" => t("whole", "soft", "Montserrat", "#FFFFFF", "#101010", None),
        _ => return None,
    })
}

/// Пер-шрифтовой визуальный множитель размера (_FONT_SCALE) — чтобы все семейства читались одним
/// размером при одном базовом кегле.
pub fn font_scale(font: &str) -> f32 {
    match font {
        "Montserrat" => 1.0,
        "Oswald" => 1.06,
        "Roboto" => 1.0,
        "Russo One" => 0.92,
        "Pacifico" => 1.42,
        "Playfair Display" => 1.06,
        "Caveat" => 1.55,
        _ => 1.0,
    }
}

// ─── Цветовые хелперы (порт _hex_ass/_lum/_c6/_is_vivid) ──────────────────────

/// #RRGGBB -> ASS &H00BBGGRR. Некорректный вход -> белый.
pub fn hex_ass(hexstr: &str) -> String {
    let h = hexstr.trim().trim_start_matches('#');
    if h.len() != 6 {
        return "&H00FFFFFF".to_string();
    }
    // &H00 + BB + GG + RR, uppercase
    format!("&H00{}{}{}", &h[4..6], &h[2..4], &h[0..2]).to_uppercase()
}

/// Перцептивная яркость 0..1 из #RRGGBB. Некорректный -> 1.0.
pub fn lum(hexstr: &str) -> f32 {
    let h = hexstr.trim().trim_start_matches('#');
    if h.len() != 6 {
        return 1.0;
    }
    let r = i64::from_str_radix(&h[0..2], 16).unwrap_or(255) as f32;
    let g = i64::from_str_radix(&h[2..4], 16).unwrap_or(255) as f32;
    let b = i64::from_str_radix(&h[4..6], 16).unwrap_or(255) as f32;
    (0.299 * r + 0.587 * g + 0.114 * b) / 255.0
}

/// ASS &H[AA]BBGGRR -> инлайн/drawing форма &HBBGGRR& (порт _c6).
pub fn c6(ass: &str) -> String {
    let mut h = ass.replace("&H", "").replace('&', "");
    if h.len() == 8 {
        h = h[2..].to_string();
    }
    if h.is_empty() {
        h = "FFFFFF".to_string();
    }
    format!("&H{h}&")
}

/// Направленная тень: жёсткий сдвиг тени на `dist` px под углом `shadow_dir`° цветом `color_ass`.
/// None -> "" (равномерная тень остаётся). screen-y-down: 0=вправо, 45=вниз-вправо, 90=вниз, 270=вверх.
/// Порт captions._dir_shadow_tags 1:1.
pub fn dir_shadow_tags(shadow_dir: Option<i64>, dist: i64, color_ass: &str) -> String {
    let Some(deg) = shadow_dir else {
        return String::new();
    };
    let r = (deg as f64).to_radians();
    let d = dist.max(1);
    format!(
        "\\shad{d}\\xshad{}\\yshad{}\\4c{color_ass}",
        (d as f64 * r.cos()).round() as i64,
        (d as f64 * r.sin()).round() as i64
    )
}

/// True если #RRGGBB — явный цвет (не бело/серо/чёрный): (max-min)>=60 (порт _is_vivid).
pub fn is_vivid(hexstr: &str) -> bool {
    let h = hexstr.trim_start_matches('#');
    if h.len() != 6 {
        return false;
    }
    let r = i64::from_str_radix(&h[0..2], 16).unwrap_or(0);
    let g = i64::from_str_radix(&h[2..4], 16).unwrap_or(0);
    let b = i64::from_str_radix(&h[4..6], 16).unwrap_or(0);
    (r.max(g).max(b) - r.min(g).min(b)) >= 60
}

/// Разрешённый лук (результат _resolve_look): None = match-original путь.
#[derive(Clone)]
pub struct ResolvedLook {
    pub reveal: String,
    pub plate: String,
    pub font: String,
    pub base: String,     // c6
    pub base_lum: f32,
    pub accent6: String,  // c6
    pub plate6: String,   // c6
}

/// Порт _resolve_look. caption_style (имя TEMPLATE) + опц. plate/reveal/font оверрайды -> лук, либо
/// None = match-original (caption_style None/"match" и нет ручного оверрайда). accent по умолчанию —
/// цвет оркестратора (sub_style.color если vivid), иначе #FFD400.
pub fn resolve_look(
    caption_style: Option<&str>,
    plate: Option<&str>,
    reveal: Option<&str>,
    font: Option<&str>,
    sub_style_color: Option<&str>,
) -> Option<ResolvedLook> {
    let is_match = matches!(caption_style, None | Some("match"));
    let no_override = plate.is_none() && reveal.is_none() && font.is_none();
    if is_match && no_override {
        return None;
    }
    let base_name = caption_style.unwrap_or(DEFAULT_TEMPLATE);
    let t = template(base_name).unwrap_or_else(|| template(DEFAULT_TEMPLATE).unwrap());
    // Эффективные значения = оверрайд-или-шаблон (без Box::leak/'static-принуждения — не течёт память).
    let reveal_src = reveal.unwrap_or(t.reveal);
    let plate_src = plate.unwrap_or(t.plate);
    let font_src = font.unwrap_or(t.font);
    let det = sub_style_color;
    let accent_hex: String = t
        .accent
        .map(|s| s.to_string())
        .or_else(|| det.filter(|d| is_vivid(d)).map(|d| d.to_string()))
        .unwrap_or_else(|| "#FFD400".to_string());
    let font_out = if is_known_font(font_src) {
        font_src.to_string()
    } else {
        FONT_NAME.to_string()
    };
    Some(ResolvedLook {
        reveal: reveal_src.to_string(),
        plate: plate_src.to_string(),
        font: font_out,
        base: c6(&hex_ass(t.base)),
        base_lum: lum(t.base),
        accent6: c6(&hex_ass(&accent_hex)),
        plate6: c6(&hex_ass(t.plate_c)),
    })
}

/// FONTS-словарь (family -> описание) для GET /presets, если понадобится.
#[allow(dead_code)]
pub fn fonts_map() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("Montserrat", "clean geometric sans"),
        ("Oswald", "tall condensed sans (impact)"),
        ("Roboto", "neutral plain sans"),
        ("Russo One", "very heavy bold geometric display (max impact)"),
        ("Pacifico", "flowing brush script"),
        ("Playfair Display", "elegant high-contrast serif"),
        ("Caveat", "casual handwritten"),
    ])
}

/// FONTS в порядке питона (dict(captions.FONTS)) — для GET /fonts. Порядок вставки сохраняем.
pub const FONTS_ORDERED: [(&str, &str); 17] = [
    ("Montserrat", "clean geometric sans"),
    ("Oswald", "tall condensed sans (impact)"),
    ("Roboto", "neutral plain sans"),
    ("Noto Sans", "universal neutral sans (widest language coverage)"),
    ("Fira Sans", "clean humanist sans"),
    ("Rubik", "rounded geometric sans"),
    ("Exo 2", "techno futuristic sans"),
    ("Comfortaa", "soft rounded geometric display"),
    ("Russo One", "very heavy bold geometric display (max impact)"),
    ("Yeseva One", "heavy elegant display serif"),
    ("Playfair Display", "elegant high-contrast serif"),
    ("Prata", "high-contrast didone serif (fashion/elegant)"),
    ("Philosopher", "elegant humanist sans-serif"),
    ("Pacifico", "flowing brush script"),
    ("Caveat", "casual handwritten"),
    ("Pangolin", "casual rounded handwritten"),
    ("Marck Script", "flowing handwritten script"),
];

/// Имена TEMPLATES в порядке питона (list(captions.TEMPLATES)) — для GET /presets.
pub const TEMPLATE_NAMES: [&str; 26] = [
    "clean", "minimal", "boxed", "headline", "serif", "card", "hormozi", "hormozi_green",
    "mrbeast", "impact", "pop", "karaoke", "karaoke_gold", "karaoke_neon", "bubble", "bubble_pop",
    "candy", "neon", "neon_pink", "cyber", "fresh", "fresh_bold", "fresh_pop", "fresh_karaoke",
    "fresh_hormozi", "fresh_soft",
];

/// True если у пресета НЕТ ключа plate_c в питоне (fresh-семейство): dict без plate_c.
pub fn template_omits_plate_c(name: &str) -> bool {
    name.starts_with("fresh")
}
