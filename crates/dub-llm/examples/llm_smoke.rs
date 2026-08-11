//! Прогрев и проверка сайдкара: поднять llama-server на Gemma, задать простой промпт, напечатать ответ.
//!
//! Запуск (Git Bash):
//!   export PATH="$(cygpath -u "$USERPROFILE/.cargo/bin"):$PATH"
//!   DUB_STUDIO_LLAMA_BIN=tools/llama/llama-server.exe \
//!   cargo run -p dub-llm --example llm_smoke -- models/mt/gemma-4-12b-it-qat-q4_0.gguf [mmproj.gguf]

use std::path::PathBuf;
use std::time::Instant;

use dub_llm::{ChatClient, LlamaServer, Message, Sampling, ServerOpts};

fn main() {
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "models/mt/gemma-4-12b-it-qat-q4_0.gguf".into()),
    );
    let mmproj = args.next().map(PathBuf::from);

    let bin = std::env::var("DUB_STUDIO_LLAMA_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tools/llama/llama-server.exe"));

    let mut opts = ServerOpts::new(bin, &model);
    if let Some(mm) = mmproj {
        opts = opts.with_mmproj(mm);
    }

    let t0 = Instant::now();
    println!("поднимаю llama-server (загрузка GGUF в VRAM)...");
    let srv = match LlamaServer::start(&opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("СБОЙ старта: {e}");
            std::process::exit(1);
        }
    };
    println!("готов за {:.1}с, base_url={}", t0.elapsed().as_secs_f64(), srv.base_url());

    let client = ChatClient::new(srv.base_url()).expect("client");
    let msgs = vec![
        Message::system("You are a terse assistant. Reply in one short line."),
        Message::user_text("Translate to English: Привет, как дела?"),
    ];
    let s = Sampling::new(0.2, 0.9, 64).top_k(64);
    let t1 = Instant::now();
    match client.chat(&msgs, &s) {
        Ok(txt) => {
            let clean = dub_llm::strip_think(&txt);
            println!("ответ ({:.1}с): {clean}", t1.elapsed().as_secs_f64());
        }
        Err(e) => {
            eprintln!("СБОЙ чата: {e}");
            std::process::exit(1);
        }
    }
    println!("OK");
}
