//! E2E-верификация caption-композита на живых данных без ASR/Gemma/TTS.
//! Использование:
//!   cargo run -p dub-server --example verify_captions -- <repo_root> <project.json> <video> <work_dir>

use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: verify_captions <repo_root> <project.json> <video> <work_dir>");
        std::process::exit(2);
    }
    let repo = PathBuf::from(&a[1]);
    let proj = PathBuf::from(&a[2]);
    let video = PathBuf::from(&a[3]);
    let wd = PathBuf::from(&a[4]);
    std::fs::create_dir_all(&wd).unwrap();
    match dub_server::verify_captions_e2e(&repo, &proj, &video, &wd) {
        Ok(p) => println!("OK captioned -> {}", p.display()),
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}
