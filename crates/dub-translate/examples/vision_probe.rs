//! Vision-проба: поднять llama-server с mmproj и прогнать analyze_layout (GREEDY sub-style temp=0.0,
//! VP temp=0.2) на видео — печатает sub_style / sub_y / titles / brands. Диагностика паритета плашек:
//! проверяем, что именно Gemma читает с example_original.mp4 (bg solid? scene_color? titles?).
//!
//! Запуск (Git Bash):
//!   export PATH="$(cygpath -u "$USERPROFILE/.cargo/bin"):$PATH"
//!   cargo run -p dub-translate --example vision_probe -- docs/example_original.mp4

use std::path::PathBuf;

use dub_llm::{ChatClient, LlamaServer, ServerOpts};
use dub_translate::analyze_layout;

fn main() {
    let video = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "docs/example_original.mp4".into()),
    );
    let model = PathBuf::from(
        std::env::var("DUB_MT_MODEL").unwrap_or_else(|_| "models/mt/gemma-4-12b-it-qat-q4_0.gguf".into()),
    );
    let mmproj = PathBuf::from(
        std::env::var("DUB_MMPROJ")
            .unwrap_or_else(|_| "models/mt/mmproj-gemma-4-12b-it-qat-q4_0.gguf".into()),
    );
    let bin = std::env::var("DUB_STUDIO_LLAMA_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tools/llama/llama-server.exe"));

    // размеры кадра через ffprobe
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries",
               "stream=width,height,duration", "-of", "default=nw=1"])
        .arg(&video)
        .output()
        .expect("ffprobe");
    let info = String::from_utf8_lossy(&probe.stdout);
    let mut vh = 0f64;
    let mut total = 0f64;
    for ln in info.lines() {
        if let Some(v) = ln.strip_prefix("height=") { vh = v.trim().parse().unwrap_or(0.0); }
        if let Some(v) = ln.strip_prefix("duration=") { total = v.trim().parse().unwrap_or(0.0); }
    }
    eprintln!("video={} vh={} total={}", video.display(), vh, total);

    let opts = ServerOpts::new(bin, &model).with_mmproj(&mmproj);
    eprintln!("поднимаю llama-server (+mmproj)...");
    let srv = LlamaServer::start(&opts).expect("start llama-server");
    eprintln!("готов, base_url={}", srv.base_url());
    let client = ChatClient::new(srv.base_url()).expect("client");

    let tmp = std::env::temp_dir().join("_vision_probe_kf.png");
    let layout = analyze_layout(&client, &video, &tmp, total, vh).expect("analyze_layout");

    println!("=== VISION LAYOUT (greedy sub-style) ===");
    println!("sub_y   = {:?}", layout.sub_y);
    println!("sub_style = {}", serde_json::to_string_pretty(&layout.sub_style).unwrap());
    println!("titles  = {}", serde_json::to_string_pretty(&layout.titles).unwrap());
    println!("brands  = {}", serde_json::to_string_pretty(&layout.brands).unwrap());
    println!("captions= {}", serde_json::to_string_pretty(&layout.captions).unwrap());
}
