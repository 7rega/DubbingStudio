//! Дымовой прогон: загрузка SCRFD + LVFace, детект на синтетическом кадре, эмбеддинг.
//! Запуск: DUBENGINE_MODELS_ROOT=<...>/models cargo run -p dub-faces --example smoke

use dub_faces::{FacesModels, LvFace, Scrfd};

fn main() {
    let root = std::env::var("DUBENGINE_MODELS_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("models"));
    let models = FacesModels::resolve(&root);
    println!("scrfd: {} ({})", models.scrfd.display(), models.scrfd.is_file());
    println!("lvface: {} ({})", models.lvface.display(), models.lvface.is_file());
    println!("lr_asd: {:?}", models.lr_asd);
    if !models.available() {
        eprintln!("веса не найдены — прогон невозможен");
        std::process::exit(1);
    }

    let mut scrfd = Scrfd::load(&models.scrfd).expect("SCRFD load");
    println!("SCRFD загружен");
    let mut lvface = LvFace::load(&models.lvface).expect("LVFace load");
    println!("LVFace загружен");

    // Синтетический кадр 640x480 с градиентом (лиц не будет — проверяем, что прогон не падает).
    let mut img = image::RgbImage::new(640, 480);
    for (x, _y, p) in img.enumerate_pixels_mut() {
        let v = (x % 256) as u8;
        *p = image::Rgb([v, 128, 255 - v]);
    }
    let faces = scrfd.detect(&img).expect("detect");
    println!("детектировано лиц (на синт. кадре): {}", faces.len());

    // Эмбеддинг фиктивного лица (bbox по центру, kps в разумных точках) — проверка LVFace-инференса.
    let fake = dub_faces::Face {
        x1: 220.0,
        y1: 140.0,
        x2: 420.0,
        y2: 380.0,
        score: 0.99,
        kps: [
            (270.0, 220.0),
            (370.0, 220.0),
            (320.0, 270.0),
            (285.0, 320.0),
            (355.0, 320.0),
        ],
    };
    let emb = lvface.embed_face(&img, &fake).expect("embed");
    let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    println!("эмбеддинг: dim={} L2-норма={:.4}", emb.len(), norm);
    assert_eq!(emb.len(), 512, "LVFace-L должен дать 512-d");

    // Опц.: реальное видео (arg 1) -> семплинг кадров + детект реальных лиц + кластеризация.
    if let Some(video) = std::env::args().nth(1) {
        let dir = std::env::temp_dir().join("dubfaces_smoke_frames");
        let frames = dub_faces::sample_frames(std::path::Path::new(&video), &dir, 1.0).expect("frames");
        println!("\nвидео {video}: кадров {}", frames.len());
        let mut all: Vec<dub_faces::FrameFace> = Vec::new();
        let mut total_faces = 0usize;
        for fr in &frames {
            let fs = scrfd.detect(&fr.img).unwrap_or_default();
            total_faces += fs.len();
            for f in &fs {
                if let Ok(e) = lvface.embed_face(&fr.img, f) {
                    all.push(dub_faces::FrameFace {
                        t: fr.t,
                        bbox: (f.x1, f.y1, f.x2, f.y2),
                        score: f.score,
                        sharpness: dub_faces::crop_sharpness(&fr.img, (f.x1, f.y1, f.x2, f.y2)),
                        frontality: dub_faces::frontality(f),
                        embedding: e,
                    });
                }
            }
        }
        println!("всего лиц: {total_faces}");
        let clusters = dub_faces::cluster_faces(&all, dub_faces::cluster_cos_threshold());
        println!("кластеров (персонажей): {}", clusters.len());
        for (i, c) in clusters.iter().enumerate() {
            println!("  персонаж {i}: {} кадров, медоид={}, аватар-кадр={}", c.members.len(), c.medoid, all[c.avatar].t);
        }
    }
    println!("OK");
}
