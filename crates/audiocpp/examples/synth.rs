//! Пример synth: озвучить текст через audiocpp_engine.dll и записать WAV.
//!
//! TTS:   synth --dll <путь.dll> --model-root <каталог_весов> --text "..." --out out.wav
//! Клон:  synth --dll ... --model-root ... --text "..." --ref-wav ref.wav --ref-text "..." --out out.wav
//!
//! Опции: --backend cpu|cuda|vulkan|metal (дефолт cpu), --device N, --threads N, --weight-type <str>,
//!        --opts '<json>'.

use audiocpp::AudiocppEngine;
use std::path::PathBuf;

struct Args {
    dll: PathBuf,
    model_root: PathBuf,
    text: String,
    out: PathBuf,
    ref_wav: Option<String>,
    ref_text: Option<String>,
    backend: String,
    device: i32,
    threads: i32,
    weight_type: Option<String>,
    opts: String,
}

fn parse_args() -> Result<Args, String> {
    let mut dll = None;
    let mut model_root = None;
    let mut text = None;
    let mut out = PathBuf::from("out.wav");
    let mut ref_wav = None;
    let mut ref_text = None;
    let mut backend = "cpu".to_string();
    let mut device = 0i32;
    let mut threads = 8i32;
    let mut weight_type = None;
    let mut opts = String::new();

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut next = || it.next().ok_or_else(|| format!("флаг {a} требует значение"));
        match a.as_str() {
            "--dll" => dll = Some(PathBuf::from(next()?)),
            "--model-root" => model_root = Some(PathBuf::from(next()?)),
            "--text" => text = Some(next()?),
            "--out" => out = PathBuf::from(next()?),
            "--ref-wav" => ref_wav = Some(next()?),
            "--ref-text" => ref_text = Some(next()?),
            "--backend" => backend = next()?,
            "--device" => device = next()?.parse().map_err(|e| format!("--device: {e}"))?,
            "--threads" => threads = next()?.parse().map_err(|e| format!("--threads: {e}"))?,
            "--weight-type" => weight_type = Some(next()?),
            "--opts" => opts = next()?,
            "-h" | "--help" => return Err("см. шапку файла для использования".into()),
            other => return Err(format!("неизвестный флаг: {other}")),
        }
    }

    Ok(Args {
        dll: dll.ok_or("нужен --dll")?,
        model_root: model_root.ok_or("нужен --model-root")?,
        text: text.ok_or("нужен --text")?,
        out,
        ref_wav,
        ref_text,
        backend,
        device,
        threads,
        weight_type,
        opts,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка аргументов: {e}");
            std::process::exit(2);
        }
    };

    eprintln!("загрузка DLL: {}", args.dll.display());
    let eng = match AudiocppEngine::load(&args.dll) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("не удалось загрузить движок: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("версия движка: {}", eng.version());

    eprintln!(
        "загрузка весов: {} (backend={}, device={}, threads={})",
        args.model_root.display(),
        args.backend,
        args.device,
        args.threads
    );
    if let Err(e) = eng.load_model(
        &args.model_root,
        &args.backend,
        args.device,
        args.threads,
        args.weight_type.as_deref(),
    ) {
        eprintln!("load_model: {e}");
        std::process::exit(1);
    }

    let result = if let Some(ref_wav) = &args.ref_wav {
        eprintln!("клон голоса из {ref_wav}");
        eng.voice_clone(&args.text, ref_wav, args.ref_text.as_deref(), &args.opts)
    } else {
        eprintln!("TTS");
        eng.tts(&args.text, &args.opts)
    };

    let (samples, sr) = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("генерация не удалась: {e}");
            std::process::exit(1);
        }
    };

    let wav = AudiocppEngine::encode_wav(&samples, sr, 1);
    if let Err(e) = std::fs::write(&args.out, &wav) {
        eprintln!("запись {}: {e}", args.out.display());
        std::process::exit(1);
    }
    eprintln!(
        "готово: {} ({} сэмплов, {} Hz, {:.2}s)",
        args.out.display(),
        samples.len(),
        sr,
        samples.len() as f32 / sr.max(1) as f32
    );
}
