//! Офлайн-тюнинг порога голосовой AHC-кластеризации по кэшу эмбеддингов (casting/vc_embs.json).
//! Без сервера/лиц — мгновенный свип порогов + распределение пар. Запуск:
//!   cargo run -p dub-server --example vc_tune -- <path/to/vc_embs.json>

use std::path::PathBuf;

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut d, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        d += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    d / (na.sqrt() * nb.sqrt())
}

fn ahc_average(embs: &[Vec<f32>], threshold: f32) -> usize {
    let n = embs.len();
    if n == 0 {
        return 0;
    }
    let dim = embs[0].len();
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut centroids: Vec<Vec<f32>> = embs.to_vec();
    let mut active = vec![true; n];
    loop {
        let idxs: Vec<usize> = (0..centroids.len()).filter(|&i| active[i]).collect();
        let mut best = (usize::MAX, usize::MAX, f32::MIN);
        for a in 0..idxs.len() {
            for b in (a + 1)..idxs.len() {
                let c = cosine(&centroids[idxs[a]], &centroids[idxs[b]]);
                if c > best.2 {
                    best = (idxs[a], idxs[b], c);
                }
            }
        }
        if best.0 == usize::MAX || best.2 < threshold {
            break;
        }
        let (i, j) = (best.0, best.1);
        let jm = std::mem::take(&mut members[j]);
        members[i].extend(jm);
        active[j] = false;
        let mut c = vec![0.0f32; dim];
        for &pt in &members[i] {
            for d in 0..dim {
                c[d] += embs[pt][d];
            }
        }
        let cnt = members[i].len() as f32;
        for v in &mut c {
            *v /= cnt;
        }
        centroids[i] = c;
    }
    active.iter().filter(|&&a| a).count()
}

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("path to vc_embs.json"));
    let txt = std::fs::read_to_string(&path).expect("read");
    let v: serde_json::Value = serde_json::from_str(&txt).expect("json");
    let embs: Vec<Vec<f32>> = serde_json::from_value(v["embs"].clone()).expect("embs");
    println!("segments embedded: {}", embs.len());
    // распределение попарных косинусов (насколько вообще разделимы).
    let mut cs: Vec<f32> = Vec::new();
    for i in 0..embs.len() {
        for j in (i + 1)..embs.len() {
            cs.push(cosine(&embs[i], &embs[j]));
        }
    }
    cs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f32| cs[((cs.len() as f32 * p) as usize).min(cs.len() - 1)];
    println!("pairwise cos: min {:.2} p10 {:.2} p50 {:.2} p90 {:.2} p99 {:.2} max {:.2}", cs[0], pct(0.1), pct(0.5), pct(0.9), pct(0.99), cs[cs.len() - 1]);
    for t in [0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5].iter() {
        println!("AHC threshold {:.2} -> {} clusters", t, ahc_average(&embs, *t));
    }
}
