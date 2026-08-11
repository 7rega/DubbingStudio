//! Пример asr: транскрибировать WAV со словными таймстемпами, опционально с диаризацией. JSON в stdout.
//!
//!   asr --wav <in.wav> --tdt <каталог_TDT> [--diarize --sortformer <sortformer.onnx>] [--lang auto]
//!
//! Без --diarize: печатает {"segments":[{start,end,text,words:[{word,start,end}]}]}.
//! С --diarize: печатает {"turns":[...],"segments":[{start,end,text,speaker}]}.

use dub_asr::{diarize, Asr};

fn main() {
    let mut wav = None;
    let mut tdt = None;
    let mut sortformer = None;
    let mut do_diarize = false;
    let mut lang = "auto".to_string();

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--wav" => wav = it.next(),
            "--tdt" => tdt = it.next(),
            "--sortformer" => sortformer = it.next(),
            "--diarize" => do_diarize = true,
            "--lang" => lang = it.next().unwrap_or(lang),
            other => {
                eprintln!("неизвестный флаг: {other}");
                std::process::exit(2);
            }
        }
    }

    let wav = wav.unwrap_or_else(|| die("нужен --wav"));
    let tdt = tdt.unwrap_or_else(|| die("нужен --tdt <каталог TDT-модели>"));

    let mut asr = Asr::new(&tdt);

    if do_diarize {
        let sf = sortformer
            .unwrap_or_else(|| die("--diarize требует --sortformer <sortformer.onnx>"));
        eprintln!("диаризация: {sf}");
        let turns = match diarize(&wav, &sf) {
            Ok(t) => t,
            Err(e) => die(&format!("диаризация не удалась: {e}")),
        };
        eprintln!("реплик: {}", turns.len());
        let segs = match asr.transcribe_turns(&wav, &turns) {
            Ok(s) => s,
            Err(e) => die(&format!("транскрипция не удалась: {e}")),
        };
        let out = serde_json::json!({ "turns": turns, "segments": segs });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        eprintln!("[asr] загрузка модели TDT из {tdt} + транскрипция {wav} ...");
        let t0 = std::time::Instant::now();
        let segs = match asr.transcribe(&wav, &lang) {
            Ok(s) => s,
            Err(e) => die(&format!("транскрипция не удалась: {e}")),
        };
        eprintln!("[asr] готово за {:.1}с, сегментов: {}", t0.elapsed().as_secs_f32(), segs.len());
        let out = serde_json::json!({ "segments": segs });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}
