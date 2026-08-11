//! Юнит-тесты на генерацию ASS для нескольких пресетов (сравнение ожидаемых тэгов). Проверяем, что
//! каждый лук эмитит характерные для него ASS-теги (karaoke \kf, word reveal \t..\alpha, hormozi
//! highlight recolour, neon glow-плашка), плюс базовый head/стили.

use dub_captions::{build, set_fonts_dir, BuildArgs, Sub, SubStyle, Title};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn fonts_dir() -> PathBuf {
    // тесты запускаются из корня крейта -> ../../fonts (repo/fonts).
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../fonts");
    p
}

// Уникальный счётчик -> отдельный ASS-файл на каждый вызов (тесты идут ПАРАЛЛЕЛЬНО; общий путь =
// гонка чтения/записи, из-за которой два None-теста дрались за dc_test_match.ass).
static SEQ: AtomicU64 = AtomicU64::new(0);

fn gen(caption_style: Option<&str>, sub_style: Option<&SubStyle>) -> String {
    set_fonts_dir(fonts_dir());
    let subs = vec![
        Sub { start: 0.0, end: 2.0, tgt: "one two three".into(), y: Some(1500) },
        Sub { start: 2.0, end: 4.0, tgt: "four five six seven".into(), y: Some(1500) },
    ];
    let uid = SEQ.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("dc_test_{}_{uid}.ass", caption_style.unwrap_or("match")));
    let args = BuildArgs {
        subs: &subs,
        sub_style,
        sub_y: Some(1500),
        caption_style,
        ..Default::default()
    };
    build(1080, 1920, &out, args).unwrap();
    std::fs::read_to_string(&out).unwrap()
}

#[test]
fn head_has_core_styles() {
    let ass = gen(None, Some(&SubStyle::default()));
    assert!(ass.contains("[Script Info]"), "нет Script Info");
    assert!(ass.contains("PlayResX: 1080") && ass.contains("PlayResY: 1920"), "нет PlayRes");
    assert!(ass.contains("Style: S,"), "нет S-стиля");
    assert!(ass.contains("Style: T,"), "нет T-стиля (титры)");
    assert!(ass.contains("Style: KP,") && ass.contains("Style: KT,"), "нет KP/KT стилей");
    assert!(ass.contains("[Events]"), "нет секции Events");
}

#[test]
fn match_original_uses_s_style_events() {
    // без caption_style (match-original) субтитры идут стилем S на Layer 1.
    let ass = gen(None, Some(&SubStyle::default()));
    assert!(ass.contains("Dialogue: 1,") && ass.contains(",S,,"), "нет S-событий субтитра");
    // match-original НЕ должен эмитить karaoke/pop-теги.
    assert!(!ass.contains("\\kf"), "match-original не должен содержать \\kf");
}

#[test]
fn karaoke_preset_emits_kf() {
    let ass = gen(Some("karaoke"), None);
    assert!(ass.contains("\\kf"), "karaoke-пресет должен эмитить \\kf");
    assert!(ass.contains(",KT,,"), "стилизованный текст идёт стилем KT");
    assert!(ass.contains(",KP,,"), "плашка идёт стилем KP");
}

#[test]
fn word_and_pop_emit_alpha_transition() {
    let word = gen(Some("candy"), None); // candy: reveal=word
    assert!(word.contains("\\alpha&HFF&") && word.contains("\\t("), "word reveal: alpha+transition");
    let pop = gen(Some("mrbeast"), None); // mrbeast: reveal=pop
    assert!(pop.contains("\\fscx55") || pop.contains("\\fscy55"), "pop reveal: scale-in");
}

#[test]
fn hormozi_highlight_recolours_active_word() {
    let ass = gen(Some("hormozi"), None); // reveal=highlight, accent #FFD400
    // highlight перекрашивает активное слово в accent, потом обратно в base -> два \1c подряд в одном событии.
    assert!(ass.matches("\\1c").count() >= 3, "highlight должен многократно менять \\1c");
    assert!(ass.contains(",KT,,"), "highlight-текст идёт стилем KT");
}

#[test]
fn neon_uses_glow_plate() {
    let ass = gen(Some("neon"), None); // plate=glow
    // glow-плашка = размытый accent-ореол \blur за непрозрачной плашкой -> есть \blur в KP-событии.
    assert!(ass.contains("\\blur"), "glow-плашка должна содержать \\blur");
    assert!(ass.contains(",KP,,"), "плашка стилем KP");
}

#[test]
fn subtitles_are_non_overlapping() {
    // каждая строка кончается там, где начинается следующая: два субтитра 0-2 и 2-4 не пересекаются.
    let ass = gen(None, Some(&SubStyle::default()));
    // проверим что есть событие, кончающееся на 0:00:02.00 (конец первого == старт второго).
    assert!(ass.contains("0:00:02.00"), "границы окон должны совпадать (2.00)");
}

#[test]
fn uppercase_style_uppercases_text() {
    let mut ss = SubStyle::default();
    ss.uppercase = true;
    let ass = gen(None, Some(&ss));
    assert!(ass.contains("ONE TWO THREE"), "uppercase-стиль должен дать капс");
}

// Сгенерировать ASS только с одним титром (без сабов) и вернуть текст.
fn gen_title(t: Title, width: i64, height: i64) -> String {
    set_fonts_dir(fonts_dir());
    let uid = SEQ.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("dc_title_{uid}.ass"));
    let titles = [t];
    let args = BuildArgs { titles: &titles, ..Default::default() };
    build(width, height, &out, args).unwrap();
    std::fs::read_to_string(&out).unwrap()
}

// Достать \fs<N> из строки KT-события (нарисованный текст титра).
fn title_fs(ass: &str) -> i64 {
    let line = ass.lines().find(|l| l.contains(",KT,,")).expect("нет KT-события титра");
    let i = line.find("\\fs").expect("нет \\fs в титре") + 3;
    let rest = &line[i..];
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().expect("не число после \\fs")
}

// Число нарисованных строк титра = кол-во \N в теле + 1.
fn title_lines(ass: &str) -> usize {
    let line = ass.lines().find(|l| l.contains(",KT,,")).expect("нет KT-события титра");
    // тело — после закрывающей } тег-блока.
    let body = line.rsplit('}').next().unwrap_or("");
    body.matches("\\N").count() + 1
}

// Титр «POV: Ты пытаешься затащить славянку к себе домой.» не должен рисоваться
// ГИГАНТСКИМ кеглем. Фикс блюр-блида (af95837) растянул bbox до ВСЕЙ вертикальной стопки (h=169 против
// строки ~70), а max_lines=round(h/lh)+1 читал h СТОПКИ -> позволял 3 строки, автошринк не срабатывал,
// fs=lh=70 раздувало титр. Разделение назначений: bbox — позиционирование/перенос/БЛЮР (вся стопка), lh —
// КЕГЛЬ (строка). Проверяем: при bbox.h=169, lh=70 длинный русский титр ужимается до <=2 строк и его fs
// НЕ равен полному 70 (шринк отработал), т.е. стопка НЕ раздувает кегль.
#[test]
fn title_kegel_from_line_height_not_blur_stack() {
    // bbox = вся contiguous-стопка (для блюра), lh = высота ОДНОЙ строки.
    let long = Title {
        text: "POV: Ты пытаешься затащить славянку к себе домой.".into(),
        bbox: Some(vec![65, 162, 541, 169]), // x,y,w,h — h=169 (СТОПКА)
        lh: Some(70),                        // строка ~70px
        start: 0.0,
        end: 28.5,
        align: "center".into(),
        bold: true,
        color: Some("#FFFFFF".into()),
        ..Default::default()
    };
    let ass = gen_title(long, 720, 1280);
    let fs = title_fs(&ass);
    let nlines = title_lines(&ass);
    // max_lines теперь = n_src+1 = 2 (стопка не влияет) -> длинный титр ужат до <=2 строк.
    assert!(nlines <= 2, "титр должен уложиться в <=2 строки, получил {nlines}");
    // при 2-строчном лимите fs ужат ниже 70 (иначе не влезло бы) — стопка не раздула кегль.
    assert!(fs < 70, "кегль должен ужаться ниже line-height от шринка, получил fs={fs}");
    assert!(fs >= 22, "но не ниже пола 22, получил fs={fs}");

    // КОНТРОЛЬ: тот же титр с ТЕСНЫМ bbox (h=lh=70) даёт ТОТ ЖЕ результат — доказывает, что раньше
    // разницу давала именно h стопки, а теперь кегль/строки от неё не зависят.
    let tight = Title {
        text: "POV: Ты пытаешься затащить славянку к себе домой.".into(),
        bbox: Some(vec![65, 162, 541, 70]), // h=70 (как было ДО af95837)
        lh: Some(70),
        start: 0.0,
        end: 28.5,
        align: "center".into(),
        bold: true,
        color: Some("#FFFFFF".into()),
        ..Default::default()
    };
    let ass2 = gen_title(tight, 720, 1280);
    assert_eq!(title_fs(&ass2), fs, "кегль идентичен вне зависимости от h стопки");
    assert_eq!(title_lines(&ass2), nlines, "кол-во строк идентично вне зависимости от h стопки");
}
