//! Юнит-тесты постпроцесса (порт text_detect.py + pipeline.py) на фикстурах-массивах.
//! Проверяют sim/iou/merge_rows/detect_regions_frames и band_blur без ONNX.
//! Поглощают тест-сет ветки r4-ocr (12 тестов); публичный API — интеграции rust-port.

use dub_ocr::blur;
use dub_ocr::{detect_regions_frames, iou, merge_rows, sim, Line};

const FPS: i32 = 2;

#[test]
fn sim_substring_is_one() {
    // подстрока -> 1.0 (порт _sim)
    assert_eq!(sim("КОРОЧЕ ОН", "короче"), 1.0);
    assert_eq!(sim("hello world", "world"), 1.0);
}

#[test]
fn sim_ratio_partial() {
    // частичное совпадение -> в (0,1) (точный difflib.ratio)
    let r = sim("подключен", "подключён");
    assert!(r > 0.7 && r < 1.0, "got {r}");
    // разное -> низко
    assert!(sim("кошка", "самолёт") < 0.5);
}

#[test]
fn sim_empty_is_zero() {
    assert_eq!(sim("", "abc"), 0.0);
    assert_eq!(sim("!!!", "abc"), 0.0); // после нормализации alnum пусто
}

#[test]
fn iou_basic() {
    assert_eq!(iou((0.0, 0.0, 10.0, 10.0), (0.0, 0.0, 10.0, 10.0)), 1.0);
    assert_eq!(iou((0.0, 0.0, 10.0, 10.0), (20.0, 20.0, 5.0, 5.0)), 0.0);
    let v = iou((0.0, 0.0, 10.0, 10.0), (5.0, 0.0, 10.0, 10.0));
    assert!((v - (50.0 / 150.0)).abs() < 1e-5, "got {v}");
}

#[test]
fn merge_rows_joins_same_row() {
    // два слова на одной строке (близкие cy) -> одна линия, объединённый бокс
    let items: Vec<Line> = vec![
        ("КОРОЧЕ".into(), 100.0, 600.0, 80.0, 20.0),
        ("ОН".into(), 190.0, 602.0, 30.0, 18.0),
        ("ПОДКЛЮЧЕН".into(), 120.0, 640.0, 140.0, 22.0),
    ];
    let merged = merge_rows(items, 0.6);
    assert_eq!(merged.len(), 2, "два ряда -> две линии");
    let top = &merged[0];
    assert!(top.0.contains("КОРОЧЕ") && top.0.contains("ОН"));
    assert!((top.1 - 100.0).abs() < 1e-3); // x = min
    assert!((top.3 - 120.0).abs() < 1e-3); // w = 190+30 - 100 = 120
}

#[test]
fn detect_regions_tracks_held_caption() {
    // Один и тот же саб держится на кадрах t=0,0.5,1.0 (fps=2) -> один регион с pad и правильным t1.
    let mk = |t: f32| -> (f32, Vec<Line>) {
        (t, vec![("КОРОЧЕ ОН ПОДКЛЮЧЕН".into(), 100.0, 640.0, 200.0, 30.0)])
    };
    let frames = vec![mk(0.0), mk(0.5), mk(1.0)];
    let (regions, raw) = detect_regions_frames(&frames, FPS, 0.3, 0.3, 8, 20.0);
    assert_eq!(raw.len(), 3, "три сырых детекции");
    assert_eq!(regions.len(), 1, "один трек");
    let r = &regions[0];
    assert!(r.text.contains("ПОДКЛЮЧЕН"));
    // pad=8: x = 100-8 = 92, y = 640-8 = 632, w = 200+16 = 216
    assert_eq!(r.x, 92);
    assert_eq!(r.y, 632);
    assert_eq!(r.w, 216);
    // t0=0.0, t1 = 1.0 + 1/fps(0.5) = 1.5
    assert!((r.t0 - 0.0).abs() < 1e-3);
    assert!((r.t1 - 1.5).abs() < 1e-3, "t1 got {}", r.t1);
}

#[test]
fn detect_regions_aspect_filter_drops_tall_box() {
    // высокий бокс (w < 1.2*h) отфильтровывается до raw (aspect: линии широкие)
    let frames = vec![
        (0.0, vec![("X".into(), 10.0, 10.0, 20.0, 40.0)]),
        (0.5, vec![("X".into(), 10.0, 10.0, 20.0, 40.0)]),
    ];
    let (regions, raw) = detect_regions_frames(&frames, FPS, 0.3, 0.3, 8, 20.0);
    assert_eq!(raw.len(), 0, "aspect-фильтр убирает высокий бокс до raw");
    assert_eq!(regions.len(), 0);
}

#[test]
fn detect_regions_short_track_dropped() {
    // Отбрасывается трек короче min_dur ДАЖЕ С хвостом +1/fps (питон text_detect.py:126:
    // t1-t0+1/fps>=min_dur). При fps=2 один кадр = длительность 0 + хвост 0.5; чтобы РЕАЛЬНО отбросить,
    // min_dur должен быть > 0.5 -> берём 0.6.
    let frames = vec![(0.0, vec![("hi there text".into(), 50.0, 700.0, 120.0, 25.0)])];
    let (regions, _) = detect_regions_frames(&frames, FPS, 0.6, 0.3, 8, 20.0);
    assert_eq!(regions.len(), 0);
}

#[test]
fn detect_regions_single_frame_kept_with_tail() {
    // ПАРИТЕТ С ПИТОНОМ: гейт t1-t0+1/fps>=min_dur -> один кадр (0 + хвост 0.5) при min_dur=0.3 ПРОХОДИТ
    // и трек СОХРАНЯЕТСЯ. Раньше Rust считал t1-t0>=min_dur (0>=0.3=false) и выкидывал — расхождение.
    let frames = vec![(0.0, vec![("hi there text".into(), 50.0, 700.0, 120.0, 25.0)])];
    let (regions, _) = detect_regions_frames(&frames, FPS, 0.3, 0.3, 8, 20.0);
    assert_eq!(regions.len(), 1);
    assert!((regions[0].t1 - 0.5).abs() < 1e-3, "t1 got {}", regions[0].t1);
}

#[test]
fn detect_regions_different_text_new_track() {
    // тот же спот, но другой текст (sim<0.7) -> НОВЫЙ трек, не absorb
    let frames = vec![
        (0.0, vec![("первый титр здесь".into(), 100.0, 640.0, 200.0, 30.0)]),
        (0.5, vec![("первый титр здесь".into(), 100.0, 640.0, 200.0, 30.0)]),
        (1.0, vec![("совсем иная строка".into(), 100.0, 640.0, 200.0, 30.0)]),
        (1.5, vec![("совсем иная строка".into(), 100.0, 640.0, 200.0, 30.0)]),
    ];
    let (regions, _) = detect_regions_frames(&frames, FPS, 0.3, 0.3, 8, 20.0);
    assert_eq!(regions.len(), 2, "смена текста на том же месте -> два трека");
}

#[test]
fn straddles_center_gate() {
    let vw = 464.0; // как в тестовом видео
    assert!(blur::straddles_center(120.0, 200.0, vw)); // 120..320 накрывает центр
    assert!(!blur::straddles_center(0.0, 100.0, vw)); // левый край
    assert!(!blur::straddles_center(300.0, 100.0, vw)); // правый край
}

#[test]
fn band_blur_coalesces_held_caption() {
    // держащийся саб на одном месте на 3 кадрах -> один бокс-спан с паддингом
    let dets: Vec<blur::Det> = vec![
        (100.0, 640.0, 200.0, 30.0, 0.0),
        (100.0, 640.0, 200.0, 30.0, 0.5),
        (100.0, 640.0, 200.0, 30.0, 1.0),
    ];
    let bb = blur::band_blur(dets, 2);
    assert_eq!(bb.len(), 1, "один спан");
    let b = &bb[0];
    // паддинг (-6,-4,+12,+8): x=94, y=636, w=212, h=38
    assert_eq!(b.0, 94);
    assert_eq!(b.1, 636);
    assert_eq!(b.2, 212);
    assert_eq!(b.3, 38);
    // t0=0.0, t1 = 1.0 + dt(0.5) = 1.5
    assert!((b.4 - 0.0).abs() < 1e-3);
    assert!((b.5 - 1.5).abs() < 1e-3, "t1 got {}", b.5);
}

#[test]
fn band_blur_separate_spots() {
    // два разнесённых места -> два бокса
    let dets: Vec<blur::Det> = vec![
        (100.0, 640.0, 100.0, 30.0, 0.0),
        (100.0, 640.0, 100.0, 30.0, 0.5),
        (100.0, 100.0, 100.0, 30.0, 0.0),
        (100.0, 100.0, 100.0, 30.0, 0.5),
    ];
    let bb = blur::band_blur(dets, 2);
    assert_eq!(bb.len(), 2);
}

#[test]
fn fold_homoglyphs_latin_to_cyrillic() {
    // rec путает латиницу/кириллицу на вшитом тексте — фолд приводит к тому, что видит зритель.
    assert_eq!(dub_ocr::fold_homoglyphs("KоpочEон"), "КорочЕон");
    assert_eq!(dub_ocr::fold_homoglyphs("CLAUDE"), "СLАUDЕ");
    // не-гомоглифы не трогаем
    assert_eq!(dub_ocr::fold_homoglyphs("подключен"), "подключен");
}

#[test]
fn analyze_layout_blurs_spoken_band_rejects_scene() {
    use dub_ocr::{analyze_layout, RawDet};
    use std::collections::HashSet;
    // нижняя субтитр-полоса (cy~640): 3 distinct СКАЗАННЫХ строки; верхняя сцен-графика (cy~460):
    // 3 distinct НЕ сказанных. Гейт должен накрыть полосу и исключить сцен-графику.
    let mk = |txt: &str, y: f32, t: f32| RawDet { text: txt.into(), x: 100.0, y, w: 200.0, h: 26.0, t };
    let raw = vec![
        mk("короче он", 628.0, 0.0), mk("подключен", 628.0, 0.5), mk("сигнал", 628.0, 1.0),
        mk("green pea snack", 455.0, 0.0), mk("bakery", 455.0, 0.5), mk("chiya cafe", 455.0, 1.0),
    ];
    let spoken: HashSet<String> =
        ["короче", "он", "подключен", "сигнал"].iter().map(|s| s.to_string()).collect();
    let (localize, caps, sub_y) = analyze_layout(&[], 824, &raw, &spoken);
    assert!(sub_y.is_some(), "субтитр-полоса найдена");
    let sy = sub_y.unwrap();
    assert!((sy - 641).abs() <= 42, "sub_y на нижней полосе (~641), got {sy}");
    assert!(!caps.is_empty(), "полоса сабов накрыта блюром");
    // сцен-графика (cy~468) НЕ на субтитр-линии -> в localize, не в caps
    assert!(caps.iter().all(|c| c.1 > 500), "в caps только нижняя полоса, got {:?}",
        caps.iter().map(|c| c.1).collect::<Vec<_>>());
    let _ = localize;
}
