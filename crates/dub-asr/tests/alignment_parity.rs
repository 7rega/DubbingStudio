//! Golden parity: full forced-alignment pipeline (ONNX CTC + acoustic edges) must
//! reproduce the approved experiment2 `acoustic.srt` bounds within tolerance.
//! Requires the pinned wav2vec2 ONNX model and the reference WAV; skipped (with a
//! clear message) when either is absent — CI has neither, dev machines may.
use dub_asr::forced::{Aligner, Input};
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct Fixture {
    segments: Vec<Segment>,
}
#[derive(serde::Deserialize)]
struct Segment {
    id: String,
    text: String,
    start: f64,
    end: f64,
    expect_start: f64,
    expect_end: f64,
}

fn resolve(var: &str, candidates: &[&str]) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(var) {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

#[test]
fn golden_bounds_match_experiment2_acoustic() {
    let fixture: Fixture = serde_json::from_slice(
        &std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(fixture.segments.len(), 53);

    let model_dir = resolve("ALIGN_MODEL_DIR", &[
        "F:/DubStudio/models/alignment/en",
        "D:/AI-CLI/temp/opencode/dub-alignment-lab/onnx",
    ]);
    let wav = resolve("ALIGN_GOLDEN_WAV", &[
        "F:/DubStudio/workspace/5eb3eef6308d/vocals16_clean.wav",
    ]);
    let (Some(model_dir), Some(wav)) = (model_dir, wav) else {
        eprintln!("SKIP golden parity: set ALIGN_MODEL_DIR / ALIGN_GOLDEN_WAV (model + reference audio are not in the repo)");
        return;
    };
    if !model_dir.join("model.onnx").is_file() {
        eprintln!("SKIP golden parity: {} has no model.onnx", model_dir.display());
        return;
    }

    let (samples, sr) = dub_asr::load_wav_16k_mono(&wav).expect("load golden wav");
    assert_eq!(sr, 16000);
    let inputs: Vec<Input> = fixture
        .segments
        .iter()
        .map(|s| Input { id: s.id.clone(), text: s.text.clone(), start: s.start, end: s.end })
        .collect();

    let mut aligner = Aligner::load(&model_dir).expect("load aligner");
    let outcomes = aligner.align(&inputs, &samples, &|_, _| {}).expect("align");
    assert_eq!(outcomes.len(), inputs.len());

    // The golden run aligned every segment; the acoustic edges may differ by at
    // most one 5 ms RMS hop plus rounding — allow 25 ms total, matching the
    // experiment2 consensus threshold (threshold_consensus_ms = 30).
    let tolerance = 0.025;
    let mut aligned = 0;
    for (seg, out) in fixture.segments.iter().zip(&outcomes) {
        assert_eq!(out.id, seg.id);
        let Some(a) = &out.aligned else {
            panic!("{} skipped: {:?}", seg.id, out.reason);
        };
        aligned += 1;
        assert!(
            (a.start - seg.expect_start).abs() <= tolerance,
            "{} start {} vs golden {}",
            seg.id, a.start, seg.expect_start
        );
        // Ends where the approved run used the +20 ms fallback (consensus failed)
        // are now governed by the approved decaying-tail walk (variant B); the
        // golden audio for them no longer exists, so compare only consensus ends.
        let lexical_end = a.words.last().map(|w| w.end).unwrap_or(a.end);
        let was_fallback = (seg.expect_end - (lexical_end + 0.020)).abs() <= 0.0011;
        if !was_fallback {
            assert!(
                (a.end - seg.expect_end).abs() <= tolerance,
                "{} end {} vs golden {}",
                seg.id, a.end, seg.expect_end
            );
        }
    }
    assert_eq!(aligned, 53);
}

#[test]
fn fixture_matches_approved_reference_hash() {
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden.json"));
    let bytes = std::fs::read(path).unwrap();
    // Checkout may rewrite line endings (core.autocrlf); the approved hash is over LF bytes.
    let normalized = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let hash = blake3::hash(normalized.as_bytes()).to_hex().to_string();
    // Regenerate with: python tools/alignment (results.json bounds.baseline/acoustic).
    // If this fails, the fixture was edited or re-derived — re-approve against
    // F:\DubStudio\workspace\5eb3eef6308d\alignment_experiment2_20260913.
    let expected = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden.sha3"))
        .expect("alignment_golden.sha3 missing");
    assert_eq!(hash, expected.trim());
}

/// Approved variant B (decaying tail + one bridged shout) on project 131a2e5dc8a5.
#[test]
fn golden_tail_bounds_match_approved_variant_b() {
    let fixture: Fixture = serde_json::from_slice(
        &std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden_tail.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(fixture.segments.len(), 34);

    let model_dir = resolve("ALIGN_MODEL_DIR", &[
        "F:/DubStudio/models/alignment/en",
        "D:/AI-CLI/temp/opencode/dub-alignment-lab/onnx",
    ]);
    let wav = resolve("ALIGN_GOLDEN_TAIL_WAV", &[
        "F:/DubStudio/workspace/131a2e5dc8a5/vocals16_clean.wav",
    ]);
    let (Some(model_dir), Some(wav)) = (model_dir, wav) else {
        eprintln!("SKIP tail parity: set ALIGN_MODEL_DIR / ALIGN_GOLDEN_TAIL_WAV");
        return;
    };
    if !model_dir.join("model.onnx").is_file() {
        eprintln!("SKIP tail parity: {} has no model.onnx", model_dir.display());
        return;
    }

    let (samples, sr) = dub_asr::load_wav_16k_mono(&wav).expect("load tail wav");
    assert_eq!(sr, 16000);
    let inputs: Vec<Input> = fixture
        .segments
        .iter()
        .map(|s| Input { id: s.id.clone(), text: s.text.clone(), start: s.start, end: s.end })
        .collect();

    let mut aligner = Aligner::load(&model_dir).expect("load aligner");
    let outcomes = aligner.align(&inputs, &samples, &|_, _| {}).expect("align");
    assert_eq!(outcomes.len(), inputs.len());

    let tolerance = 0.025;
    let mut aligned = 0;
    for (seg, out) in fixture.segments.iter().zip(&outcomes) {
        assert_eq!(out.id, seg.id);
        let Some(a) = &out.aligned else {
            panic!("{} skipped: {:?}", seg.id, out.reason);
        };
        aligned += 1;
        assert!((a.start - seg.expect_start).abs() <= tolerance, "{} start {} vs golden {}", seg.id, a.start, seg.expect_start);
        assert!((a.end - seg.expect_end).abs() <= tolerance, "{} end {} vs golden {}", seg.id, a.end, seg.expect_end);
    }
    assert_eq!(aligned, 34);
}

#[test]
fn tail_fixture_matches_approved_reference_hash() {
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden_tail.json"));
    let bytes = std::fs::read(path).unwrap();
    let normalized = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let hash = blake3::hash(normalized.as_bytes()).to_hex().to_string();
    let expected = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/alignment_golden_tail.sha3"))
        .expect("alignment_golden_tail.sha3 missing");
    assert_eq!(hash, expected.trim());
}
