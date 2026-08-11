//! Пример separate: разделить WAV на вокал + инструментал через BSRoformer.cpp.
//!
//!   separate --cli <bs_roformer-cli.exe> --model <voc_fv6-Q8_0.gguf> --in mix.wav --out-dir <dir>
//!
//! Вход — WAV 44.1кГц (движок сам сообщит, если частота не совпала). На выходе — <dir>/vocals.wav и
//! <dir>/instrumental.wav. Печатает RMS-проверку (вокал/инструментал против микса).

use dub_sep::separate;
use std::path::PathBuf;

fn main() {
    let mut cli = None;
    let mut model = None;
    let mut input = None;
    let mut out_dir = PathBuf::from(".");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut next = || it.next().expect("флаг требует значение");
        match a.as_str() {
            "--cli" => cli = Some(PathBuf::from(next())),
            "--model" => model = Some(PathBuf::from(next())),
            "--in" => input = Some(PathBuf::from(next())),
            "--out-dir" => out_dir = PathBuf::from(next()),
            other => {
                eprintln!("неизвестный флаг: {other}");
                std::process::exit(2);
            }
        }
    }
    let cli = cli.expect("нужен --cli");
    let model = model.expect("нужен --model");
    let input = input.expect("нужен --in");

    match separate(&input, &out_dir, &cli, &model) {
        Ok(r) => {
            println!("vocals:       {}", r.vocals.display());
            println!("instrumental: {}", r.instrumental.display());
        }
        Err(e) => {
            eprintln!("сепарация не удалась: {e}");
            std::process::exit(1);
        }
    }
}
