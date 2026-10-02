//! Юнит-тесты на генерацию ASS для нескольких пресетов (сравнение ожидаемых тэгов). Проверяем, что
//! каждый лук эмитит характерные для него ASS-теги (karaoke \kf, word reveal \t..\alpha, hormozi
//! highlight recolour, neon glow-плашка), плюс базовый head/стили.

use dub_captions::{build, set_fonts_dir, BuildArgs, Secondary, Sub, SubStyle, Title};
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
        Sub { start: 0.0, end: 2.0, tgt: "one two three".into(), y: Some(1500), words: None, secondary: None },
        Sub { start: 2.0, end: 4.0, tgt: "four five six seven".into(), y: Some(1500), words: None, secondary: None },
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

fn gen_with_words(caption_style: &str, subs: Vec<Sub>) -> String {
    set_fonts_dir(fonts_dir());
    let uid = SEQ.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("dc_words_{caption_style}_{uid}.ass"));
    let args = BuildArgs { subs: &subs, sub_y: Some(1500), caption_style: Some(caption_style), ..Default::default() };
    build(1080, 1920, &out, args).unwrap();
    let s = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    s
}

fn heard(ws: &[(&str, f64, f64)]) -> Option<Vec<(String, f64, f64)>> {
    Some(ws.iter().map(|&(w, s, e)| (w.to_string(), s, e)).collect())
}

#[test]
fn karaoke_follows_the_heard_words() {
    // «one» звучит с 0.5 с, «two» — с 1.5 с: полоса ждёт 50 сс, «one» заливается 100 сс, а не треть экрана.
    let subs = vec![Sub {
        start: 0.0,
        end: 3.0,
        tgt: "one two three".into(),
        y: Some(1500),
        words: heard(&[("one", 0.5, 0.9), ("two", 1.5, 1.9), ("three", 2.0, 2.6)]),
        secondary: None,
    }];
    let ass = gen_with_words("karaoke", subs);
    assert!(ass.contains("{\\k50}{\\kf100}ONE ") || ass.contains("{\\k50}{\\kf100}one "), "{ass}");
    assert!(ass.contains("{\\kf50}") && ass.contains("{\\kf100}"), "{ass}");
}

#[test]
fn highlight_events_start_when_each_word_is_spoken() {
    let subs = vec![Sub {
        start: 0.0,
        end: 3.0,
        tgt: "one two three".into(),
        y: Some(1500),
        words: heard(&[("one", 0.5, 0.9), ("two", 1.5, 1.9), ("three", 2.0, 2.6)]),
        secondary: None,
    }];
    let ass = gen_with_words("hormozi", subs);
    let starts: Vec<&str> = ass
        .lines()
        .filter(|l| l.starts_with("Dialogue: 1,") && l.contains(",KT,"))
        .map(|l| l.split(',').nth(1).unwrap())
        .collect();
    assert_eq!(starts, vec!["0:00:00.00", "0:00:00.50", "0:00:01.50", "0:00:02.00"], "{ass}");
}

#[test]
fn a_long_line_turns_the_page_when_the_voice_gets_there() {
    // Две страницы (узкий кадр, max_lines=2): вторая начинается с первого своего слова, а не с середины.
    let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango";
    let times: Vec<(&str, f64, f64)> = text
        .split_whitespace()
        .enumerate()
        .map(|(i, w)| (w, if i < 16 { i as f64 * 0.1 } else { 5.0 + (i - 16) as f64 * 0.5 }, 0.0))
        .map(|(w, s, _)| (w, s, s + 0.08))
        .collect();
    let subs = vec![Sub { start: 0.0, end: 8.0, tgt: text.into(), y: Some(1500), words: heard(&times), secondary: None }];
    let ass = gen_with_words("karaoke", subs);
    let pages: Vec<(&str, &str)> = ass
        .lines()
        .filter(|l| l.starts_with("Dialogue: 1,") && l.contains(",KT,"))
        .map(|l| {
            let mut it = l.split(',').skip(1);
            (it.next().unwrap(), it.next().unwrap())
        })
        .collect();
    assert!(pages.len() >= 2, "{ass}");
    assert_eq!(pages[0].0, "0:00:00.00");
    let word_starts: Vec<String> = times
        .iter()
        .map(|&(_, s, _)| format!("0:00:{:02}.{:02}", s as i64, ((s - s.floor()) * 100.0).round() as i64))
        .collect();
    for (k, p) in pages.iter().enumerate().skip(1) {
        assert!(word_starts.iter().any(|w| w == p.0), "страница {k} начинается не со слова: {pages:?}");
    }
}

fn gen_bilingual(caption_style: Option<&str>, secondary: &Secondary, subs: Vec<Sub>) -> String {
    gen_bilingual_styled(caption_style, secondary, subs, SubStyle::default())
}

fn gen_bilingual_styled(caption_style: Option<&str>, secondary: &Secondary, subs: Vec<Sub>, style: SubStyle) -> String {
    set_fonts_dir(fonts_dir());
    let uid = SEQ.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("dc_bi_{}_{uid}.ass", caption_style.unwrap_or("match")));
    let args = BuildArgs {
        subs: &subs,
        sub_style: Some(&style),
        sub_y: Some(1500),
        caption_style,
        secondary: Some(secondary),
        ..Default::default()
    };
    build(1080, 1920, &out, args).unwrap();
    let s = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    s
}

fn bi_sub() -> Sub {
    Sub {
        start: 1.0,
        end: 3.0,
        tgt: "Где ты был?".into(),
        y: Some(1500),
        words: None,
        secondary: Some("Where were you?".into()),
    }
}

/// (start, end, style, y из \pos, текст) каждого Dialogue слоя 1.
fn events(ass: &str) -> Vec<(String, String, String, i64, String)> {
    ass.lines()
        .filter(|l| l.starts_with("Dialogue: 1,"))
        .map(|l| {
            let f: Vec<&str> = l.splitn(10, ',').collect();
            let y = l
                .find("\\pos(")
                .map(|i| {
                    let rest = &l[i + 5..];
                    rest[..rest.find(')').unwrap()].split(',').nth(1).unwrap().trim().parse::<i64>().unwrap()
                })
                .unwrap_or(-1);
            (f[1].to_string(), f[2].to_string(), f[3].to_string(), y, f[9].rsplit('}').next().unwrap().to_string())
        })
        .collect()
}

#[test]
fn a_bilingual_line_is_two_events_with_their_own_styles() {
    let ass = gen_bilingual(None, &Secondary::default(), vec![bi_sub()]);
    let s = ass.lines().find(|l| l.starts_with("Style: S,")).unwrap();
    let s2 = ass.lines().find(|l| l.starts_with("Style: S2,")).expect("нет стиля второй строки");
    let size = |l: &str| l.split(',').nth(2).unwrap().parse::<i64>().unwrap();
    assert_eq!(size(s2), size(s) * 70 / 100, "кегль второй строки — 70 % основной");
    let colours = |l: &str| l.split(',').skip(3).take(4).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(colours(s), colours(s2), "цвета — как у основной");
    assert_eq!(s.split(',').nth(1), s2.split(',').nth(1), "шрифт — как у основной");
    let ev = events(&ass);
    assert_eq!(ev.len(), 2, "{ass}");
    let (prim, sec) = (&ev[0], &ev[1]);
    assert_eq!((prim.2.as_str(), prim.4.as_str()), ("S", "Где ты был?"));
    assert_eq!((sec.2.as_str(), sec.4.as_str()), ("S2", "Where were you?"));
    assert_eq!((&prim.0, &prim.1), (&sec.0, &sec.1), "обе строки на экране одно и то же время");
    assert!(sec.3 > prim.3, "перевод сверху, оригинал под ним: {ev:?}");
}

#[test]
fn the_original_can_go_above_and_be_restyled() {
    let secondary = Secondary { below: false, size_pct: 60, color: Some("#FFD400".into()), opacity: Some(80) };
    let ass = gen_bilingual(None, &secondary, vec![bi_sub()]);
    let ev = events(&ass);
    assert!(ev[1].3 < ev[0].3, "оригинал над переводом: {ev:?}");
    let line = ass.lines().find(|l| l.contains(",S2,")).unwrap();
    assert!(line.contains("\\1c&H00D4FF&"), "свой цвет: {line}");
    assert!(line.contains("\\alpha&H33&"), "80 % непрозрачности: {line}");
}

#[test]
fn a_faded_original_keeps_the_solid_plate_under_it() {
    let secondary = Secondary { below: true, size_pct: 70, color: None, opacity: Some(60) };
    let style = SubStyle { background: Some("#101010".into()), ..Default::default() };
    let ass = gen_bilingual_styled(None, &secondary, vec![bi_sub()], style);
    assert!(ass.lines().any(|l| l.starts_with("Style: S2,") && l.split(',').nth(15) == Some("3")), "плашка стиля S2 непрозрачная: {ass}");
    let line = ass.lines().find(|l| l.contains(",S2,")).unwrap();
    assert!(line.contains("\\1a&H66&") && !line.contains("\\alpha"), "прозрачна только заливка текста, не плашка: {line}");
}

#[test]
fn karaoke_lights_only_the_translation() {
    let mut sub = bi_sub();
    sub.tgt = "one two three".into();
    sub.words = heard(&[("one", 1.2, 1.5), ("two", 1.6, 2.0), ("three", 2.1, 2.8)]);
    let ass = gen_bilingual(Some("karaoke"), &Secondary::default(), vec![sub]);
    let kt = ass.lines().find(|l| l.contains(",KT,")).expect("основная строка в луке");
    assert!(kt.contains("\\kf"), "{kt}");
    let s2 = ass.lines().find(|l| l.contains(",S2,")).expect("вторая строка");
    assert!(!s2.contains("\\k"), "у второй строки нет подсветки: {s2}");
    assert!(s2.ends_with("Where were you?"), "{s2}");
}

/// Вертикальный охват (верх, низ) плашек KP с цветом заливки `color`.
fn plate_spans(ass: &str, color: &str) -> Vec<(f64, f64)> {
    let fill = format!("\\1c{color}");
    ass.lines()
        .filter(|l| l.starts_with("Dialogue: 0,") && l.contains(",KP,") && l.contains("\\p1") && l.contains(&fill))
        .map(|l| {
            let nums: Vec<f64> = l.rsplit('}').next().unwrap().split_whitespace().filter_map(|t| t.parse().ok()).collect();
            let ys: Vec<f64> = nums.iter().skip(1).step_by(2).copied().collect();
            (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        })
        .collect()
}

fn inline_fs(line: &str) -> i64 {
    let i = line.find("\\fs").unwrap() + 3;
    line[i..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
}

fn style_font(ass: &str, style: &str) -> String {
    let head = format!("Style: {style},");
    ass.lines().find(|l| l.starts_with(&head)).unwrap().split(',').nth(1).unwrap().to_string()
}

#[test]
fn in_a_look_the_original_takes_the_look_font_and_its_own_plate() {
    let ass = gen_bilingual(Some("karaoke"), &Secondary::default(), vec![bi_sub()]);
    assert_eq!(style_font(&ass, "S2"), "Oswald");
    let kt = ass.lines().find(|l| l.contains(",KT,")).unwrap();
    let sec = ass.lines().find(|l| l.contains(",S2,")).unwrap();
    assert!(kt.contains("\\fnOswald") && sec.contains("\\fnOswald"), "{kt}\n{sec}");
    let ratio = inline_fs(sec) as f64 / inline_fs(kt) as f64;
    assert!((ratio - 0.7).abs() < 0.02, "кегль второй строки — 70 % основной в том же шрифте: {ratio}");
    let plates = plate_spans(&ass, "&H181818&");
    assert_eq!(plates.len(), 2, "плашка лука под каждой строкой:\n{ass}");
    assert!(plates[0].1 < plates[1].0 || plates[1].1 < plates[0].0, "плашки не налезают: {plates:?}");
    let ev = events(&ass);
    for style in ["KT", "S2"] {
        let y = ev.iter().find(|e| e.2 == style).unwrap().3 as f64;
        assert!(plates.iter().any(|&(lo, hi)| lo < y && y < hi), "строка {style} ({y}) не на плашке: {plates:?}");
    }
}

#[test]
fn a_dark_look_keeps_the_original_on_its_bright_plate() {
    let ass = gen_bilingual(Some("bubble"), &Secondary::default(), vec![bi_sub()]);
    assert_eq!(style_font(&ass, "S2"), "Caveat");
    let sec = ass.lines().find(|l| l.contains(",S2,")).unwrap();
    assert!(sec.contains("\\1c&H181020&") && sec.contains("\\3c&HFFFFFF&"), "тёмный текст лука с белой обводкой: {sec}");
    let y2 = events(&ass).iter().find(|e| e.2 == "S2").unwrap().3 as f64;
    let plates = plate_spans(&ass, "&HA25DFF&");
    assert!(plates.iter().any(|&(lo, hi)| lo < y2 && y2 < hi), "вторая строка ({y2}) лежит на розовой плашке: {plates:?}");
}

#[test]
fn in_a_look_the_original_keeps_its_own_colour_and_opacity() {
    let secondary = Secondary { below: true, size_pct: 70, color: Some("#FFD400".into()), opacity: Some(80) };
    let ass = gen_bilingual(Some("hormozi"), &secondary, vec![bi_sub()]);
    let sec = ass.lines().find(|l| l.contains(",S2,")).unwrap();
    assert!(sec.contains("\\fnRusso One"), "{sec}");
    assert!(sec.contains("\\1c&H00D4FF&\\alpha&H33&"), "свой цвет и 80 % непрозрачности: {sec}");
    assert!(sec.contains("\\3c&H101010&"), "светлому тексту — тёмная обводка: {sec}");
    assert_eq!(plate_spans(&ass, "&H0C0C0C&").len(), 2, "{ass}");
    assert!(
        ass.lines().filter(|l| l.contains(",KP,") && l.contains("\\p1")).all(|l| !l.contains("\\alpha")),
        "плашки лука остаются непрозрачными:\n{ass}"
    );
}

#[test]
fn without_the_second_line_the_ass_has_no_s2() {
    let ass = gen(None, Some(&SubStyle::default()));
    assert!(!ass.contains("S2"), "{ass}");
}

#[test]
fn an_eight_second_dub_line_is_shown_as_two_events() {
    let subs = vec![Sub {
        start: 0.0,
        end: 8.0,
        tgt: "Это длинная реплика дубляжа".into(),
        y: Some(1500),
        words: None,
        secondary: None,
    }];
    let ass = gen_with_words("karaoke", subs);
    let spans: Vec<(String, String)> = events(&ass).into_iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(spans.len(), 2, "{ass}");
    assert_eq!(spans[0].0, "0:00:00.00");
    assert_eq!(spans[0].1, spans[1].0);
    assert_eq!(spans[1].1, "0:00:08.00");
}
