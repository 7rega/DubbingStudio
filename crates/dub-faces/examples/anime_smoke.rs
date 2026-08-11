//! Смоук аниме-детектора на реальных кадрах (Rust-путь приложения, НЕ python).
//! Запуск: cargo run -p dub-faces --example anime_smoke -- <frames_dir> <out_dir> [model.onnx]
//! Прогоняет AnimeFaceDetector по каждому PNG в frames_dir, печатает число+score лиц и сохраняет
//! кроп каждого найденного лица в out_dir (для визуальной проверки — реально ли ловит рисованные лица).

use dub_faces::{save_face_crop, AnimeFaceDetector};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames_dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
    let out_dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "anime_out".into()));
    let model = PathBuf::from(
        args.get(3).cloned().unwrap_or_else(|| "models/faces/anime_face/model.onnx".into()),
    );
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut det = AnimeFaceDetector::load(&model).expect("load anime detector");

    let mut frames: Vec<PathBuf> = std::fs::read_dir(&frames_dir)
        .expect("read frames_dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "png" || x == "jpg").unwrap_or(false))
        .collect();
    frames.sort();

    let mut total = 0usize;
    for f in &frames {
        let img = match image::open(f) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                eprintln!("{}: не открыть: {e}", f.display());
                continue;
            }
        };
        let faces = det.detect(&img).expect("detect");
        let scores: Vec<String> = faces.iter().map(|x| format!("{:.2}", x.score)).collect();
        println!("{}: лиц {} scores [{}]", f.file_name().unwrap().to_string_lossy(), faces.len(), scores.join(", "));
        let stem = f.file_stem().unwrap().to_string_lossy();
        for (k, fc) in faces.iter().enumerate() {
            let out = out_dir.join(format!("{stem}_face{k}.png"));
            if let Err(e) = save_face_crop(f, (fc.x1, fc.y1, fc.x2, fc.y2), 0.25, &out) {
                eprintln!("  кроп {}: {e}", out.display());
            }
            total += 1;
        }
    }
    println!("ИТОГО лиц: {total} по {} кадрам", frames.len());
}
