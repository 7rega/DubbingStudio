//! Индустриальные правила субтитров (задача #86) — чистый pure-fn модуль поверх сегментов, вызываемый
//! ОПЦИОНАЛЬНО как timing-QC пасс. Порт из SubtitleEdit (RulesProfile/CalcLength/AutoBreakLine/
//! FixDurationLimits) + whisperX (SubtitlesProcessor.determine_advanced_split_points).
//!
//! Дизайн (DESIGN.md §2.7 + BORROWINGS #4/#14/#15):
//!  - `RulesProfile` + таблица пресетов (Netflix/BBC/Amazon/Arte/DCMP/generic) как ЧИСТЫЕ ДАННЫЕ
//!    (`&'static [RulesProfile]`, без lazy-init — все поля const).
//!  - `trait CalcLength` (Latin/CJK/Arabic по Unicode-диапазонам) — корректный счёт CPS для CJK.
//!  - `optimal_display_ms` (GetOptimalDisplayMilliseconds) — идеальная длительность = симв/optCps.
//!  - Балансный 2-строчный перенос (ширина / запятая / союз, no-break-after списки).
//!  - Нормализаторы длительности: clamp min/max, gap-bridge, merge/split. Merge НЕ через границу спикера.
//!
//! Крейт остаётся независимым от dub-core/dub-asr: работаем над локальным лёгким [`TimingSeg`],
//! который рендер-стадия заполняет из своего Segment. Ничего в движках не трогаем — только арифметика
//! над таймкодами/строками. Все функции детерминированы и не имеют побочных эффектов.

/// Лёгкий сегмент для timing-QC: [start,end] в СЕКУНДАХ, текст реплики и опц. метка спикера.
/// Держим отдельно от dub-asr::Segment, чтобы не тянуть зависимость (крейт — чистый рендер).
/// Метка спикера нужна нормализатору merge (не сливаем через границу спикера).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimingSeg {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Идентификатор спикера (для гейта merge). `None` == неизвестно (сливать разрешено с любым).
    pub speaker: Option<String>,
}

impl TimingSeg {
    pub fn new(start: f64, end: f64, text: impl Into<String>) -> Self {
        TimingSeg { start, end, text: text.into(), speaker: None }
    }
    pub fn with_speaker(mut self, spk: impl Into<String>) -> Self {
        self.speaker = Some(spk.into());
        self
    }
    #[inline]
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

// ───────────────────────────── RulesProfile (данные пресетов) ─────────────────────────────

/// Индустриальный профиль правил субтитров. Порт `libse/Common/Settings.cs::RulesProfile`.
/// Числа — из дефолтных профилей SubtitleEdit (вещатели). Все поля const → таблица без lazy-init.
#[derive(Clone, Copy, Debug)]
pub struct RulesProfile {
    /// Имя профиля (для UI-дропдауна).
    pub name: &'static str,
    /// Макс. символов в строке (перенос стремится не превышать).
    pub max_line_len: usize,
    /// Макс. число строк субтитра (обычно 2).
    pub max_lines: usize,
    /// Макс. допустимый CPS (chars-per-second) — жёсткий потолок читаемости.
    pub max_cps: f64,
    /// Оптимальный CPS — цель для `optimal_display_ms`.
    pub optimal_cps: f64,
    /// Минимальная длительность показа, мс.
    pub min_display_ms: u32,
    /// Максимальная длительность показа, мс.
    pub max_display_ms: u32,
    /// Минимальный зазор между двумя субтитрами, мс.
    pub min_gap_ms: u32,
    /// Стратегия подсчёта длины/CPS для этого профиля (язык-зависимо).
    pub calc: CalcKind,
}

/// Стратегия подсчёта символов (для CPS/длины строки). Порт `ICalcLength`-стратегий SubtitleEdit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcKind {
    /// Латиница/кириллица/большинство алфавитных — 1 символ = 1 (стрип тегов/zero-width).
    All,
    /// CJK-строгий: иероглиф/кана/хангыль = 1.0, японская полуширина = 0.5, латиница = 0.5.
    Cjk,
    /// Арабский: не считаем диакритику (harakat) и tatweel.
    Arabic,
}

/// Таблица пресетов вещателей (порт `Settings.GetDefaultProfiles`). Чистые данные, без аллокаций.
/// ~13 профилей: generic + Netflix (EN/latin/CJK/Arabic) + Amazon + BBC + Arte (2) + DCMP.
pub const PROFILES: &[RulesProfile] = &[
    RulesProfile {
        name: "Generic",
        max_line_len: 43,
        max_lines: 2,
        max_cps: 25.0,
        optimal_cps: 20.0,
        min_display_ms: 1000,
        max_display_ms: 8000,
        min_gap_ms: 24,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "Netflix (English)",
        max_line_len: 42,
        max_lines: 2,
        max_cps: 20.0,
        optimal_cps: 15.0,
        min_display_ms: 833,
        max_display_ms: 7007,
        min_gap_ms: 83,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "Netflix (Latin)",
        max_line_len: 42,
        max_lines: 2,
        max_cps: 17.0,
        optimal_cps: 13.0,
        min_display_ms: 833,
        max_display_ms: 7000,
        min_gap_ms: 83,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "Netflix (Chinese/Japanese/Korean)",
        max_line_len: 16,
        max_lines: 2,
        max_cps: 9.0,
        optimal_cps: 7.0,
        min_display_ms: 833,
        max_display_ms: 7000,
        min_gap_ms: 83,
        calc: CalcKind::Cjk,
    },
    RulesProfile {
        name: "Netflix (Arabic)",
        max_line_len: 42,
        max_lines: 2,
        max_cps: 20.0,
        optimal_cps: 17.0,
        min_display_ms: 833,
        max_display_ms: 7000,
        min_gap_ms: 83,
        calc: CalcKind::Arabic,
    },
    RulesProfile {
        name: "Amazon",
        max_line_len: 42,
        max_lines: 2,
        max_cps: 20.0,
        optimal_cps: 17.0,
        min_display_ms: 833,
        max_display_ms: 7000,
        min_gap_ms: 80,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "BBC",
        max_line_len: 37,
        max_lines: 2,
        max_cps: 17.0,
        optimal_cps: 15.0,
        min_display_ms: 800,
        max_display_ms: 6000,
        min_gap_ms: 40,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "BBC (Children)",
        max_line_len: 37,
        max_lines: 2,
        max_cps: 13.0,
        optimal_cps: 11.0,
        min_display_ms: 1000,
        max_display_ms: 6000,
        min_gap_ms: 40,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "Arte",
        max_line_len: 40,
        max_lines: 2,
        max_cps: 20.0,
        optimal_cps: 17.0,
        min_display_ms: 1000,
        max_display_ms: 6000,
        min_gap_ms: 80,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "Arte (Documentary)",
        max_line_len: 40,
        max_lines: 2,
        max_cps: 17.0,
        optimal_cps: 15.0,
        min_display_ms: 1000,
        max_display_ms: 8000,
        min_gap_ms: 80,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "DCMP (English)",
        max_line_len: 32,
        max_lines: 2,
        max_cps: 17.0,
        optimal_cps: 15.0,
        min_display_ms: 1000,
        max_display_ms: 6000,
        min_gap_ms: 48,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "SBS (Australia)",
        max_line_len: 37,
        max_lines: 2,
        max_cps: 16.0,
        optimal_cps: 14.0,
        min_display_ms: 1000,
        max_display_ms: 7000,
        min_gap_ms: 80,
        calc: CalcKind::All,
    },
    RulesProfile {
        name: "YouTube",
        max_line_len: 50,
        max_lines: 2,
        max_cps: 21.0,
        optimal_cps: 17.0,
        min_display_ms: 700,
        max_display_ms: 8000,
        min_gap_ms: 24,
        calc: CalcKind::All,
    },
];

/// Профиль по умолчанию (Generic) — совпадает с текущим поведением dub-captions (2 строки),
/// но с осмысленными таймингами. Возвращается когда имя не найдено.
pub const DEFAULT_PROFILE: &RulesProfile = &PROFILES[0];

impl RulesProfile {
    /// Профиль по имени (case-insensitive, точное совпадение). `None` если нет.
    pub fn by_name(name: &str) -> Option<&'static RulesProfile> {
        PROFILES.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
    /// Профиль по имени или Generic-дефолт, если имя неизвестно/None.
    pub fn by_name_or_default(name: Option<&str>) -> &'static RulesProfile {
        name.and_then(Self::by_name).unwrap_or(DEFAULT_PROFILE)
    }
}

// ───────────────────────────── CalcLength (счёт символов/CPS) ─────────────────────────────

/// Стратегия подсчёта визуальной «длины» строки для CPS/переноса.
/// Порт `ICalcLength` (SubtitleEdit): игнорирует SSA/HTML-теги и zero-width, по-разному весит скрипты.
pub trait CalcLength {
    /// Визуальная длина строки в «символо-единицах» (может быть дробной для CJK-полуширины).
    fn count(&self, text: &str) -> f64;
}

impl CalcKind {
    /// Получить объект-стратегию.
    #[inline]
    pub fn calculator(self) -> Box<dyn CalcLength> {
        match self {
            CalcKind::All => Box::new(CalcAll),
            CalcKind::Cjk => Box::new(CalcCjk),
            CalcKind::Arabic => Box::new(CalcArabic),
        }
    }
    /// Прямой подсчёт без аллокации Box (быстрый путь).
    #[inline]
    pub fn count(self, text: &str) -> f64 {
        match self {
            CalcKind::All => CalcAll.count(text),
            CalcKind::Cjk => CalcCjk.count(text),
            CalcKind::Arabic => CalcArabic.count(text),
        }
    }
}

/// `true` для code point, который НЕ должен считаться в длине: zero-width, BOM, LRM/RLM и прочие
/// невидимые управляющие метки направления (порт `Utilities.IsZeroWidth`-класса + strip тегов).
#[inline]
fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'  // ZERO WIDTH SPACE
        | '\u{200C}' // ZWNJ
        | '\u{200D}' // ZWJ
        | '\u{200E}' // LRM
        | '\u{200F}' // RLM
        | '\u{FEFF}' // BOM / ZWNBSP
        | '\u{00AD}' // SOFT HYPHEN
        | '\u{2060}' // WORD JOINER
    )
}

/// Стрип SSA/ASS override-блоков `{...}` и HTML/SSA-тегов `<...>` для честного счёта длины.
/// Порт `Utilities.RemoveSsaTags`/`HtmlUtil.RemoveHtmlTags`. Не аллоцирует, вызывает `f` на визуальных
/// символах. `\N`/`\n` (ASS-перенос) как последовательность backslash+буква пропускаем внутри `{}`.
fn for_each_visible(text: &str, mut f: impl FnMut(char)) {
    let mut in_brace = false; // {...} SSA override
    let mut in_angle = false; // <...> html
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if !in_angle => in_brace = true,
            '}' if in_brace => in_brace = false,
            '<' if !in_brace => in_angle = true,
            '>' if in_angle => in_angle = false,
            '\\' if !in_brace && !in_angle => {
                // ASS-перенос вне override: `\N` / `\n` / `\h` не считаем как 2 символа.
                match chars.peek() {
                    Some('N') | Some('n') => {
                        chars.next();
                    }
                    Some('h') => {
                        chars.next();
                        f(' '); // \h == неразрывный пробел, считается как 1
                    }
                    _ => f('\\'),
                }
            }
            _ if in_brace || in_angle => {}
            _ if is_zero_width(c) => {}
            _ => f(c),
        }
    }
}

/// Латиница/кириллица/общий: каждый видимый символ = 1.0.
pub struct CalcAll;
impl CalcLength for CalcAll {
    fn count(&self, text: &str) -> f64 {
        let mut n = 0.0;
        for_each_visible(text, |_| n += 1.0);
        n
    }
}

/// CJK: полноширинный CJK/кана/хангыль = 1.0; латиница/цифры/японская полуширина = 0.5.
/// Порт `CalcCjk.cs`: `if (Utilities.IsCjkChar(c)) len++; else len += 0.5m;` с halfwidth-исключениями.
pub struct CalcCjk;
impl CalcLength for CalcCjk {
    fn count(&self, text: &str) -> f64 {
        let mut n = 0.0;
        for_each_visible(text, |c| {
            n += if is_cjk_fullwidth(c) { 1.0 } else { 0.5 };
        });
        n
    }
}

/// Арабский: диакритика (harakat U+064B..U+0652, U+0670) и tatweel (U+0640) визуально не занимают
/// ширину — не считаем (порт логики стрипа арабской диакритики в счётчике CPS).
pub struct CalcArabic;
impl CalcLength for CalcArabic {
    fn count(&self, text: &str) -> f64 {
        let mut n = 0.0;
        for_each_visible(text, |c| {
            if !is_arabic_zero_advance(c) {
                n += 1.0;
            }
        });
        n
    }
}

/// Полноширинный CJK-класс (иероглифы, кана, хангыль, полноширинные формы). Порт `Utilities.IsCjkChar`.
#[inline]
pub fn is_cjk_fullwidth(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x2E80..=0x2EFF // CJK Radicals Supplement
        | 0x3000..=0x303F // CJK Symbols and Punctuation (полноширинные)
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana (полноширинная)
        | 0x3130..=0x318F // Hangul Compatibility Jamo
        | 0x3400..=0x4DBF // CJK Ext A
        | 0x4E00..=0x9FFF // CJK Unified
        | 0xA000..=0xA4CF // Yi
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF00..=0xFF60 // Fullwidth forms (полноширинные ASCII-аналоги)
        | 0xFFE0..=0xFFE6 // Fullwidth signs
        | 0x20000..=0x2FA1F // CJK Ext B..F + Compatibility Supplement
    )
    // Halfwidth katakana (U+FF61..=U+FF9F) сознательно НЕ здесь: у него advance 0.5 (ветка else).
}

/// Арабская метка нулевой ширины продвижения (harakat/tatweel/маленькие знаки).
#[inline]
fn is_arabic_zero_advance(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x0640            // ARABIC TATWEEL (kashida) — растяжка, не символ
        | 0x064B..=0x065F // harakat (fathatan..) + мелкие знаки
        | 0x0670          // superscript alef
        | 0x06D6..=0x06ED // мелкие коранические знаки
        | 0x08E3..=0x08FF // расширенные арабские метки
    )
}

// ───────────────────────────── optimal_display_ms ─────────────────────────────

/// Идеальная длительность показа субтитра, мс — порт `Utilities.GetOptimalDisplayMilliseconds`.
/// База = длина/optimal_cps секунд; SubtitleEdit слегка растягивает короткие строки (piecewise-shaping),
/// чтобы совсем короткие не мелькали. Затем клампится к [min_display_ms, max_display_ms] профиля.
pub fn optimal_display_ms(text: &str, p: &RulesProfile) -> u32 {
    let len = p.calc.count(text);
    if len <= 0.0 {
        return p.min_display_ms;
    }
    // SubtitleEdit: optimalCharactersPerSecond, с «shaping»-фактором для коротких.
    // ms = len / optCps * 1000, но короткие строки получают надбавку (нижняя граница по кол-ву слов).
    let base_ms = (len / p.optimal_cps) * 1000.0;
    // piecewise-shaping (порт: очень короткие строки получают доп. время «на восприятие»).
    let shaped = if len < 8.0 {
        base_ms + (8.0 - len) * 30.0 // +30мс за каждый недостающий до 8 символ
    } else {
        base_ms
    };
    (shaped.round() as i64).clamp(p.min_display_ms as i64, p.max_display_ms as i64) as u32
}

/// Фактический CPS текста при заданной длительности (сек). Ноль при неположительной длительности.
pub fn chars_per_second(text: &str, dur_s: f64, p: &RulesProfile) -> f64 {
    if dur_s <= 0.0 {
        return 0.0;
    }
    p.calc.count(text) / dur_s
}

// ───────────────────────────── Балансный 2-строчный перенос ─────────────────────────────

/// Языковые множества «союзов/предлогов», после которых предпочтительно НЕ рвать строку (перенос
/// делаем ПЕРЕД союзом, чтобы не осиротить его в конце верхней строки). Порт conjunction-таблиц
/// whisperX SubtitlesProcessor. Данные, регистр нижний.
const CONJUNCTIONS_EN: &[&str] = &[
    "and", "or", "but", "so", "for", "nor", "yet", "the", "a", "an", "to", "of", "in", "on", "at",
    "by", "as", "if", "that", "with",
];
const CONJUNCTIONS_RU: &[&str] = &[
    "и", "а", "но", "или", "да", "что", "чтобы", "как", "если", "то", "же", "бы", "ли", "в", "во",
    "на", "по", "с", "со", "к", "ко", "от", "до", "за", "из", "у", "о", "об",
];
const CONJUNCTIONS_ES: &[&str] = &["y", "o", "u", "e", "que", "de", "la", "el", "en", "a", "por", "con", "para"];
const CONJUNCTIONS_FR: &[&str] = &["et", "ou", "que", "de", "la", "le", "les", "un", "une", "à", "en", "dans", "pour"];
const CONJUNCTIONS_DE: &[&str] = &["und", "oder", "aber", "der", "die", "das", "ein", "eine", "in", "zu", "mit", "für"];

/// «Не рвать ПОСЛЕ» (no-break-after) — сокращения/титулы, за которыми следует имя/число.
/// Порт `ContinuationUtilities`/`AutoBreakLine` no-break-after списков. Данные.
const NO_BREAK_AFTER: &[&str] =
    &["mr.", "mrs.", "ms.", "dr.", "st.", "prof.", "sr.", "jr.", "vs.", "no.", "fig.", "св.", "т.", "д.", "г.", "стр."];

/// Собрать список союзов под язык (по ISO-коду; префикс достаточно). Неизвестный → EN.
fn conjunctions_for(lang: &str) -> &'static [&'static str] {
    let l = lang.to_ascii_lowercase();
    if l.starts_with("ru") || l.starts_with("uk") || l.starts_with("be") {
        CONJUNCTIONS_RU
    } else if l.starts_with("es") {
        CONJUNCTIONS_ES
    } else if l.starts_with("fr") {
        CONJUNCTIONS_FR
    } else if l.starts_with("de") {
        CONJUNCTIONS_DE
    } else {
        CONJUNCTIONS_EN
    }
}

/// Разбить одну реплику на ≤`max_lines` СБАЛАНСИРОВАННЫХ строк по правилам профиля.
/// Порт `determine_advanced_split_points` (whisperX) + `AutoBreakLinePrivate` (SubtitleEdit):
///  1) если укладывается в 1 строку ≤ max_line_len — не рвём;
///  2) иначе ищем точку реза около середины: (a) запятая с достатком с обеих сторон, (b) ПЕРЕД союзом,
///     (c) просто по середине слов; предпочитая наиболее сбалансированный вариант, не рвя no-break-after.
/// Возвращает Vec строк (≤ max_lines; лишнее сливается в последнюю). Счёт длины — по стратегии профиля.
pub fn balanced_wrap(text: &str, p: &RulesProfile, lang: &str) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    // Влезает в одну строку — ничего не рвём (порт: single-line, если len<=max).
    if p.calc.count(t) as usize <= p.max_line_len {
        return vec![t.to_string()];
    }
    // 2 строки — базовый случай (max_lines обычно 2). Для max_lines>2 применяем рекурсивно к длинной.
    let (a, b) = split_two(t, p, lang);
    if p.max_lines <= 2 || b.is_empty() {
        return if b.is_empty() { vec![a] } else { vec![a, b] };
    }
    // >2 строк: добираем, рекурсивно деля самую длинную часть, пока не упрёмся в max_lines.
    let mut lines = vec![a, b];
    while lines.len() < p.max_lines {
        // индекс самой длинной строки, которую ещё имеет смысл делить
        let Some((idx, _)) = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| p.calc.count(l) as usize > p.max_line_len)
            .max_by(|(_, x), (_, y)| p.calc.count(x).partial_cmp(&p.calc.count(y)).unwrap())
        else {
            break;
        };
        let (x, y) = split_two(&lines[idx], p, lang);
        if y.is_empty() {
            break;
        }
        lines.splice(idx..=idx, [x, y]);
    }
    lines
}

/// Разбить строку на ДВЕ сбалансированные части по лучшей точке реза. `("вся", "")` если не нашли.
fn split_two(text: &str, p: &RulesProfile, lang: &str) -> (String, String) {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 {
        return (text.to_string(), String::new());
    }
    let conj = conjunctions_for(lang);
    let total = p.calc.count(text);
    let half = total / 2.0;

    // Кумулятивная «длина» до каждой границы слов (после i-го слова, включая пробел).
    // boundary[i] = длина текста words[0..=i] (с внутренними пробелами).
    let mut cum: Vec<f64> = Vec::with_capacity(words.len());
    let mut acc = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            acc.push(' ');
        }
        acc.push_str(w);
        cum.push(p.calc.count(&acc));
    }

    // Кандидатные точки реза = граница ПОСЛЕ слова i (i in 0..len-2 → верхняя непустая, нижняя непустая).
    // Скоринг: |cum[i] - half| (баланс) + бонусы/штрафы по типу границы.
    let mut best_i: Option<usize> = None;
    let mut best_score = f64::INFINITY;
    for i in 0..words.len() - 1 {
        let upper = cum[i];
        let lower = total - upper;
        // обе части не должны превышать max_line_len, если это достижимо (мягкий штраф иначе).
        let mut score = (upper - half).abs();
        let overflow = (upper as usize).saturating_sub(p.max_line_len) as f64
            + (lower as usize).saturating_sub(p.max_line_len) as f64;
        score += overflow * 3.0; // штраф за переполнение строки

        let cur = words[i];
        let next = words[i + 1];
        let cur_l = cur.to_ascii_lowercase();
        let next_l = next.to_ascii_lowercase();

        // (a) запятая/точка/двоеточие в конце текущего слова — естественный шов (бонус).
        if cur.ends_with([',', '.', ':', ';', '—', '–', '!', '?']) {
            score -= 6.0;
        }
        // (b) СЛЕДУЮЩЕЕ слово — союз: рвём ПЕРЕД ним (не осиротить союз) — бонус.
        if conj.contains(&next_l.as_str()) {
            score -= 4.0;
        }
        // (штраф) текущее слово — союз в конце верхней строки (осиротили) — избегаем.
        if conj.contains(&cur_l.as_str()) {
            score += 4.0;
        }
        // (запрет) no-break-after (Mr./Dr./св.) в конце верхней строки — резать нельзя.
        if NO_BREAK_AFTER.contains(&cur_l.as_str()) {
            score += 1000.0;
        }

        if score < best_score {
            best_score = score;
            best_i = Some(i);
        }
    }

    match best_i {
        Some(i) => {
            let a = words[..=i].join(" ");
            let b = words[i + 1..].join(" ");
            (a, b)
        }
        None => (text.to_string(), String::new()),
    }
}

// ───────────────────────────── Нормализаторы длительности ─────────────────────────────

/// Параметры merge/split для нормализации (в дополнение к RulesProfile). Как в SubtitleEdit-формах.
#[derive(Clone, Copy, Debug)]
pub struct NormalizeOpts {
    /// Растягивать короткие субтитры до min_display_ms (FixDurationLimits).
    pub fix_min: bool,
    /// Обрезать длинные субтитры до max_display_ms.
    pub fix_max: bool,
    /// Мостить малые зазоры (DurationsBridgeGaps).
    pub bridge_gaps: bool,
    /// Делить закрываемый зазор поровну (иначе тянем только конец предыдущего).
    pub bridge_divide_even: bool,
    /// Верхняя граница зазора для мостика, мс (больше — не трогаем, это осмысленная пауза).
    pub bridge_max_gap_ms: u32,
    /// Сливать смежные короткие близкие реплики (MergeShortLines).
    pub merge_short: bool,
    /// Порог «короткой» реплики для merge, мс.
    pub merge_short_ms: u32,
    /// Макс. зазор между репликами для merge, мс.
    pub merge_max_gap_ms: u32,
}

impl Default for NormalizeOpts {
    fn default() -> Self {
        NormalizeOpts {
            fix_min: true,
            fix_max: true,
            bridge_gaps: true,
            bridge_divide_even: false,
            bridge_max_gap_ms: 250,
            merge_short: false,
            merge_short_ms: 1000,
            merge_max_gap_ms: 250,
        }
    }
}

#[inline]
fn ms(s: f64) -> f64 {
    s * 1000.0
}
#[inline]
fn s(ms: f64) -> f64 {
    ms / 1000.0
}

/// Полный ordering-safe пасс нормализации над Vec<TimingSeg> под профиль `p`. Порядок:
///  1) merge коротких (не через границу спикера) — если включён;
///  2) clamp длительностей min/max (не наезжая на следующий с учётом min_gap);
///  3) bridge малых зазоров.
/// Реплики считаются отсортированными по start; функция сама сортирует на входе для безопасности.
/// Возвращает НОВЫЙ вектор (вход не мутируется).
pub fn normalize(segs: &[TimingSeg], p: &RulesProfile, opts: &NormalizeOpts) -> Vec<TimingSeg> {
    let mut v: Vec<TimingSeg> = segs.to_vec();
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

    if opts.merge_short {
        v = merge_short_lines(&v, p, opts);
    }
    clamp_durations(&mut v, p, opts);
    if opts.bridge_gaps {
        bridge_gaps(&mut v, p, opts);
    }
    v
}

/// Слить смежные короткие+близкие реплики. НЕ сливаем через границу спикера и через смену предложения
/// (текущая реплика заканчивается на .!?…). Порт `MergeShortLinesUtils`. Результат ре-переносим не тут
/// (это делает `balanced_wrap` на рендере) — только объединяем тайминги+текст.
pub fn merge_short_lines(segs: &[TimingSeg], p: &RulesProfile, opts: &NormalizeOpts) -> Vec<TimingSeg> {
    if segs.is_empty() {
        return Vec::new();
    }
    let max_gap = s(opts.merge_max_gap_ms as f64);
    let short = s(opts.merge_short_ms as f64);
    let mut out: Vec<TimingSeg> = Vec::with_capacity(segs.len());
    out.push(segs[0].clone());

    for next in &segs[1..] {
        let cur = out.last().unwrap();
        let gap = next.start - cur.end;
        let cur_short = cur.duration() < short;
        let next_short = next.duration() < short;
        let same_speaker = match (&cur.speaker, &next.speaker) {
            (Some(a), Some(b)) => a == b,
            _ => true, // неизвестный спикер — сливать разрешено
        };
        // не сливаем если: разный спикер, зазор велик, обе длинные, текущая кончает предложение,
        // или объединённый текст переполнит max_lines*max_line_len (грубая проверка по счёту).
        let cur_ends_sentence = cur.text.trim_end().ends_with(['.', '!', '?', '…']);
        let merged_len = p.calc.count(&cur.text) + p.calc.count(&next.text) + 1.0;
        let cap = (p.max_line_len * p.max_lines) as f64;
        if same_speaker
            && gap <= max_gap
            && gap >= 0.0
            && (cur_short || next_short)
            && !cur_ends_sentence
            && merged_len <= cap
        {
            let last = out.last_mut().unwrap();
            last.end = next.end.max(last.end);
            if !last.text.is_empty() && !next.text.is_empty() {
                last.text.push(' ');
            }
            last.text.push_str(next.text.trim());
        } else {
            out.push(next.clone());
        }
    }
    out
}

/// Растянуть короткие до min_display_ms и обрезать длинные до max_display_ms, не наезжая на следующую
/// реплику (оставляя min_gap). Порт `FixDurationLimits`. Мутирует на месте (порядко-безопасно).
pub fn clamp_durations(segs: &mut [TimingSeg], p: &RulesProfile, opts: &NormalizeOpts) {
    let min_gap = s(p.min_gap_ms as f64);
    let min_d = s(p.min_display_ms as f64);
    let max_d = s(p.max_display_ms as f64);
    let n = segs.len();
    for i in 0..n {
        let dur = segs[i].duration();
        // верхняя граница конца: старт следующей минус min_gap (или +inf для последней).
        let ceil_end = if i + 1 < n {
            segs[i + 1].start - min_gap
        } else {
            f64::INFINITY
        };
        // растянуть короткую (только если fix_min и не наедем на следующую).
        if opts.fix_min && dur < min_d {
            let wanted = segs[i].start + min_d;
            if wanted <= ceil_end {
                segs[i].end = wanted;
            } else if ceil_end > segs[i].start {
                segs[i].end = ceil_end; // упёрлись в соседа — тянем максимум допустимого
            }
        }
        // обрезать длинную.
        if opts.fix_max && segs[i].duration() > max_d {
            segs[i].end = segs[i].start + max_d;
        }
    }
}

/// Замостить малые зазоры между соседними репликами (>min_gap и ≤bridge_max_gap): либо тянем конец
/// предыдущей до старта следующей минус min_gap, либо делим зазор поровну. Порт `DurationsBridgeGaps`.
pub fn bridge_gaps(segs: &mut [TimingSeg], p: &RulesProfile, opts: &NormalizeOpts) {
    let min_gap = s(p.min_gap_ms as f64);
    let max_bridge = ms(s(opts.bridge_max_gap_ms as f64)); // мс
    let n = segs.len();
    for i in 0..n.saturating_sub(1) {
        let (left, right) = segs.split_at_mut(i + 1);
        let cur = &mut left[i];
        let next = &mut right[0];
        let gap_ms = ms(next.start - cur.end);
        // мостим только «дыры» больше min_gap (иначе уже слитно) и не больше порога (иначе пауза).
        if gap_ms <= p.min_gap_ms as f64 || gap_ms > max_bridge {
            continue;
        }
        if opts.bridge_divide_even {
            // поделить зазор пополам, сохраняя min_gap между.
            let mid = (cur.end + next.start) / 2.0;
            cur.end = (mid - min_gap / 2.0).max(cur.end);
            next.start = (mid + min_gap / 2.0).min(next.start);
            if next.start - cur.end < min_gap {
                cur.end = next.start - min_gap;
            }
        } else {
            // тянуть только конец предыдущей до старта следующей минус min_gap.
            let wanted = next.start - min_gap;
            if wanted > cur.end {
                cur.end = wanted;
            }
        }
    }
}

/// Флаг «строка нарушает CPS профиля» (для QC-подсветки в UI). `true` если фактический CPS > max_cps.
pub fn violates_cps(seg: &TimingSeg, p: &RulesProfile) -> bool {
    chars_per_second(&seg.text, seg.duration(), p) > p.max_cps
}

// ───────────────────────────── Тесты ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_lookup_and_default() {
        assert_eq!(RulesProfile::by_name("Netflix (English)").unwrap().max_line_len, 42);
        assert_eq!(RulesProfile::by_name("netflix (english)").unwrap().optimal_cps, 15.0);
        assert!(RulesProfile::by_name("no-such").is_none());
        assert_eq!(RulesProfile::by_name_or_default(None).name, "Generic");
        assert_eq!(RulesProfile::by_name_or_default(Some("BBC")).name, "BBC");
        assert_eq!(RulesProfile::by_name_or_default(Some("xxx")).name, "Generic");
    }

    #[test]
    fn calc_all_strips_tags_and_zero_width() {
        // {\b1}Hello{\b0} <i>World</i>  -> "Hello World" = 11 визуальных символов.
        let t = "{\\b1}Hello{\\b0} <i>World</i>";
        assert_eq!(CalcKind::All.count(t) as usize, 11);
        // zero-width + soft hyphen не считаются.
        assert_eq!(CalcKind::All.count("a\u{200B}b\u{00AD}c") as usize, 3);
        // ASS-перенос \N не добавляет длины.
        assert_eq!(CalcKind::All.count("ab\\Ncd") as usize, 4);
    }

    #[test]
    fn calc_cjk_weights_halfwidth() {
        // 2 иероглифа (=2.0) + 2 латинских (=1.0) = 3.0
        assert_eq!(CalcKind::Cjk.count("你好ab"), 3.0);
        // чисто латиница считается по 0.5
        assert_eq!(CalcKind::Cjk.count("abcd"), 2.0);
    }

    #[test]
    fn calc_arabic_ignores_harakat() {
        // базовые буквы + фатха (U+064E) — диакритика не считается.
        let base = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}"; // 5 букв "мархаба"
        let with_harakat = "\u{0645}\u{064E}\u{0631}\u{064E}\u{062D}\u{0628}\u{0627}";
        assert_eq!(CalcKind::Arabic.count(base) as usize, 5);
        assert_eq!(CalcKind::Arabic.count(with_harakat) as usize, 5);
    }

    #[test]
    fn optimal_display_clamps_and_scales() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap();
        // очень короткая строка -> min_display_ms floor.
        assert_eq!(optimal_display_ms("Hi", p), p.min_display_ms.max({
            // "Hi"=2 симв: base=2/15*1000≈133 + shaping(8-2)*30=180 -> 313, но min=833 клампит.
            833
        }));
        // очень длинная строка -> max_display_ms ceil.
        let long = "a".repeat(300);
        assert_eq!(optimal_display_ms(&long, p), p.max_display_ms);
        // средняя строка масштабируется линейно и лежит внутри границ.
        let mid = "a".repeat(45);
        let d = optimal_display_ms(&mid, p);
        assert!(d >= p.min_display_ms && d <= p.max_display_ms);
        // 45/15*1000 = 3000 мс.
        assert_eq!(d, 3000);
    }

    #[test]
    fn balanced_wrap_single_line_when_short() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap();
        let out = balanced_wrap("Hello world", p, "en");
        assert_eq!(out, vec!["Hello world".to_string()]);
    }

    #[test]
    fn balanced_wrap_breaks_before_conjunction() {
        let p = RulesProfile::by_name("BBC").unwrap(); // max_line_len=37
        // строка >37, союз "and" в середине — рвём ПЕРЕД "and".
        let text = "The quick brown fox jumped over and the lazy sleeping dog ran";
        let out = balanced_wrap(text, p, "en");
        assert_eq!(out.len(), 2);
        // "and" не должен быть последним словом верхней строки (не осиротили).
        assert!(!out[0].ends_with("and"), "союз осиротили: {:?}", out);
        // обе строки непусты и сбалансированы.
        assert!(!out[0].is_empty() && !out[1].is_empty());
    }

    #[test]
    fn balanced_wrap_prefers_comma() {
        let p = RulesProfile::by_name("BBC").unwrap();
        let text = "I came here to talk, but you already left the building";
        let out = balanced_wrap(text, p, "en");
        assert_eq!(out.len(), 2);
        // рез у запятой — верхняя строка кончается на "talk,".
        assert!(out[0].ends_with("talk,"), "ожидался рез у запятой: {:?}", out);
    }

    #[test]
    fn balanced_wrap_no_break_after_title() {
        let p = RulesProfile::by_name("BBC").unwrap();
        // "Dr." не должен оказаться в конце строки (no-break-after).
        let text = "Yesterday I finally met with Dr. Smith about the important research project";
        let out = balanced_wrap(text, p, "en");
        assert_eq!(out.len(), 2);
        assert!(!out[0].ends_with("Dr."), "нельзя рвать после Dr.: {:?}", out);
    }

    #[test]
    fn clamp_min_stretches_short_without_overlap() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap(); // min=833ms, gap=83ms
        let mut segs = vec![
            TimingSeg::new(0.0, 0.2, "Hi"),   // 200мс < 833 -> растянуть
            TimingSeg::new(5.0, 6.0, "World"),
        ];
        let opts = NormalizeOpts::default();
        clamp_durations(&mut segs, p, &opts);
        // растянута до min (есть место до 5.0-0.083).
        assert!((segs[0].end - 0.833).abs() < 1e-6, "end={}", segs[0].end);
    }

    #[test]
    fn clamp_min_respects_next_start_gap() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap();
        let mut segs = vec![
            TimingSeg::new(0.0, 0.2, "Hi"),
            TimingSeg::new(0.5, 1.5, "World"), // следующий очень близко
        ];
        clamp_durations(&mut segs, p, &NormalizeOpts::default());
        // не должен наехать: end <= 0.5 - 0.083 = 0.417.
        assert!(segs[0].end <= 0.5 - 0.083 + 1e-9, "наехал: {}", segs[0].end);
    }

    #[test]
    fn clamp_max_trims_long() {
        let p = RulesProfile::by_name("BBC").unwrap(); // max=6000ms
        let mut segs = vec![TimingSeg::new(0.0, 20.0, "Long line")];
        clamp_durations(&mut segs, p, &NormalizeOpts::default());
        assert!((segs[0].end - 6.0).abs() < 1e-6, "end={}", segs[0].end);
    }

    #[test]
    fn bridge_gaps_extends_previous() {
        let p = RulesProfile::by_name("Generic").unwrap(); // min_gap=24ms
        let mut segs = vec![
            TimingSeg::new(0.0, 1.0, "A"),
            TimingSeg::new(1.15, 2.0, "B"), // зазор 150мс -> мостим (по умолч. max 250мс)
        ];
        bridge_gaps(&mut segs, p, &NormalizeOpts::default());
        // конец A тянется до 1.15 - 0.024 = 1.126.
        assert!((segs[0].end - (1.15 - 0.024)).abs() < 1e-6, "end={}", segs[0].end);
    }

    #[test]
    fn bridge_gaps_ignores_large_pause() {
        let p = RulesProfile::by_name("Generic").unwrap();
        let mut segs = vec![
            TimingSeg::new(0.0, 1.0, "A"),
            TimingSeg::new(3.0, 4.0, "B"), // зазор 2с > 250мс -> не трогаем
        ];
        bridge_gaps(&mut segs, p, &NormalizeOpts::default());
        assert!((segs[0].end - 1.0).abs() < 1e-6);
    }

    #[test]
    fn merge_short_not_across_speaker() {
        let p = RulesProfile::by_name("Generic").unwrap();
        let opts = NormalizeOpts { merge_short: true, ..Default::default() };
        let segs = vec![
            TimingSeg::new(0.0, 0.4, "Yes").with_speaker("A"),
            TimingSeg::new(0.5, 0.9, "no").with_speaker("B"), // другой спикер -> НЕ сливать
        ];
        let out = merge_short_lines(&segs, p, &opts);
        assert_eq!(out.len(), 2, "нельзя сливать через границу спикера");
    }

    #[test]
    fn merge_short_same_speaker_close() {
        let p = RulesProfile::by_name("Generic").unwrap();
        let opts = NormalizeOpts { merge_short: true, ..Default::default() };
        let segs = vec![
            TimingSeg::new(0.0, 0.4, "Hello").with_speaker("A"),
            TimingSeg::new(0.5, 0.9, "there").with_speaker("A"),
        ];
        let out = merge_short_lines(&segs, p, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Hello there");
        assert!((out[0].end - 0.9).abs() < 1e-6);
    }

    #[test]
    fn merge_short_stops_at_sentence_end() {
        let p = RulesProfile::by_name("Generic").unwrap();
        let opts = NormalizeOpts { merge_short: true, ..Default::default() };
        let segs = vec![
            TimingSeg::new(0.0, 0.4, "Yes.").with_speaker("A"), // конец предложения
            TimingSeg::new(0.5, 0.9, "No").with_speaker("A"),
        ];
        let out = merge_short_lines(&segs, p, &opts);
        assert_eq!(out.len(), 2, "не сливаем через конец предложения");
    }

    #[test]
    fn normalize_full_pass_is_ordering_safe() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap();
        // вход намеренно НЕ отсортирован.
        let segs = vec![
            TimingSeg::new(5.0, 5.2, "Second"),
            TimingSeg::new(0.0, 0.1, "First"),
        ];
        let out = normalize(&segs, p, &NormalizeOpts::default());
        assert_eq!(out.len(), 2);
        assert!(out[0].start < out[1].start, "должно быть отсортировано");
        // короткая первая растянута к min (места до 5.0 достаточно).
        assert!(out[0].duration() >= s(p.min_display_ms as f64) - 1e-6);
    }

    #[test]
    fn violates_cps_flag() {
        let p = RulesProfile::by_name("Netflix (English)").unwrap(); // max_cps=20
        // 40 символов за 1с = 40 cps > 20 -> нарушение.
        let bad = TimingSeg::new(0.0, 1.0, "a".repeat(40));
        assert!(violates_cps(&bad, p));
        // 15 символов за 2с = 7.5 cps -> ок.
        let ok = TimingSeg::new(0.0, 2.0, "a".repeat(15));
        assert!(!violates_cps(&ok, p));
    }
}
