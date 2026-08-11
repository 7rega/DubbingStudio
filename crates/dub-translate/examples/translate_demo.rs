//! Демо перевода: поднять llama-server на Gemma, перевести пару русских сегментов на английский
//! через flat_run (плоский MT). Проверяет весь путь dub-translate -> dub-llm.
//!
//! Запуск (Git Bash):
//!   export PATH="$(cygpath -u "$USERPROFILE/.cargo/bin"):$PATH"
//!   DUB_STUDIO_LLAMA_BIN=tools/llama/llama-server.exe \
//!   cargo run -p dub-translate --example translate_demo -- models/mt/gemma-4-12b-it-qat-q4_0.gguf

use std::path::PathBuf;

use dub_llm::{ChatClient, LlamaServer, ServerOpts};
use dub_translate::{flat_run, Seg};

fn main() {
    let model = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "models/mt/gemma-4-12b-it-qat-q4_0.gguf".into()),
    );
    let bin = std::env::var("DUB_STUDIO_LLAMA_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tools/llama/llama-server.exe"));

    let opts = ServerOpts::new(bin, &model);
    println!("поднимаю llama-server...");
    let srv = LlamaServer::start(&opts).expect("start llama-server");
    println!("готов, base_url={}", srv.base_url());
    let client = ChatClient::new(srv.base_url()).expect("client");

    let mut segs = vec![
        Seg::new("Привет, сегодня я расскажу про новый рецепт.", 0),
        Seg::new("Берём три яйца и немного муки.", 0),
        Seg::new("Готово! Приятного аппетита.", 0),
    ];
    flat_run(&client, &mut segs, "ru", "en", true, "").expect("translate");
    for (i, s) in segs.iter().enumerate() {
        println!("{}. RU: {}", i + 1, s.text);
        println!("   EN: {}", s.tgt);
    }
    println!("OK");
}
