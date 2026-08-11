//! Пример build_ass: собрать ASS из нескольких субтитров и (опц.) титра, вывести на stdout.
//!
//!   build_ass --fonts fonts --w 1080 --h 1920 [--preset clean]
//!
//! Печатает сгенерённый ASS (для глазной проверки тегов). Реального burn нет (нужен ffmpeg + видео).

use dub_captions::{build, burn_frame, set_fonts_dir, BuildArgs, Sub, SubStyle};
use std::path::PathBuf;

fn main() {
    let mut fonts = PathBuf::from("fonts");
    let mut w = 1080i64;
    let mut h = 1920i64;
    let mut preset: Option<String> = None;
    let mut burn_video: Option<PathBuf> = None;
    let mut burn_png: Option<PathBuf> = None;
    let mut burn_t = 1.0f64;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut next = || it.next().expect("флаг требует значение");
        match a.as_str() {
            "--fonts" => fonts = PathBuf::from(next()),
            "--w" => w = next().parse().unwrap(),
            "--h" => h = next().parse().unwrap(),
            "--preset" => preset = Some(next()),
            "--burn-video" => burn_video = Some(PathBuf::from(next())),
            "--burn-png" => burn_png = Some(PathBuf::from(next())),
            "--burn-t" => burn_t = next().parse().unwrap(),
            _ => {}
        }
    }
    set_fonts_dir(&fonts);
    let subs = vec![
        Sub { start: 0.0, end: 2.0, tgt: "Hello world".into(), y: Some((h as f64 * 0.82) as i64) },
        Sub { start: 2.0, end: 4.0, tgt: "This is a dubbed subtitle line".into(), y: Some((h as f64 * 0.82) as i64) },
    ];
    let style = SubStyle { color: "#FFFFFF".into(), background: Some("none".into()), bold: true, ..Default::default() };
    let out = std::env::temp_dir().join("dub_captions_example.ass");
    let args = BuildArgs {
        preset: preset.as_deref(),
        subs: &subs,
        sub_style: Some(&style),
        sub_y: Some((h as f64 * 0.82) as i64),
        caption_style: preset.as_deref(),
        ..Default::default()
    };
    build(w, h, &out, args).expect("build");
    if let (Some(video), Some(png)) = (burn_video, burn_png) {
        // живой burn одного кадра через ffmpeg+libass (проверка, что ASS парсится движком).
        burn_frame(&video, &out, &png, burn_t, &[], Some((w, h)), false, 60, None).expect("burn_frame");
        eprintln!("burned frame -> {}", png.display());
    } else {
        print!("{}", std::fs::read_to_string(&out).unwrap());
        eprintln!("\n---> {}", out.display());
    }
}
