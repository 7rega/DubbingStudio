//! Collision-resistant names for segment audio and reference clips.
use dub_core::{Project, Segment};
use serde_json::{json, Value};

pub fn key(id: &str) -> String {
    if !id.is_empty() && id.len() <= 96 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        id.to_owned()
    } else {
        // '-' cannot occur in the legacy branch. Never reuse punctuation-stripped caches.
        format!("id-{}", blake3::hash(id.as_bytes()).to_hex())
    }
}

/// Audio key for a segment: direct, clean name based on segment ID.
/// Keeps filenames human-readable (seg_s1.wav, seg_s1_fit.wav) and avoids garbage accumulation.
pub fn audio_key(segment: &Segment) -> String {
    key(&segment.id)
}

pub fn invalidate_audio(segment: &mut Segment) {
    segment.extra.remove("audio_revision");
    segment.ckpt = None;
    segment.dirty = true;
}

/// A frozen source interval survives regrouping and changes of numeric indexes.
/// It belongs to the voice override, and is ignored after a voice change.
pub fn donor(project: &Project, segment: &Segment) -> Option<Segment> {
    let voice = segment.voice.as_deref()?.trim();
    let spec = voice.strip_prefix("donor:").or_else(|| voice.strip_prefix("clone:"))?;
    if let Some(anchor) = segment.extra.get("donor_anchor") {
        if anchor.get("voice").and_then(Value::as_str) == Some(voice) {
            let start = anchor.get("start")?.as_f64()?;
            let end = anchor.get("end")?.as_f64()?;
            if start.is_finite() && end.is_finite() && start >= 0.0 && end > start {
                let live_tgt = project.segments.iter().find(|s| s.id == spec).map(|s| s.tgt_text.clone());
                return Some(Segment {
                    id: spec.into(), start, end,
                    src_text: anchor.get("src_text")?.as_str()?.into(),
                    tgt_text: live_tgt.unwrap_or_else(|| anchor.get("tgt_text").and_then(Value::as_str).unwrap_or("").into()),
                    ..Default::default()
                });
            }
        }
    }
    project.segments.iter().find(|s| s.id == spec).or_else(|| {
        spec.parse::<usize>().ok().and_then(|i| project.segments.get(i.saturating_sub(1)))
    }).cloned()
}

pub fn preserve_donors(before: &Project, after: &mut [Segment]) {
    for segment in after {
        if let Some(source) = donor(before, segment) {
            segment.extra.insert("donor_anchor".into(), json!({
                "voice": segment.voice.as_deref().unwrap_or_default().trim(),
                "start": source.start, "end": source.end,
                "src_text": source.src_text, "tgt_text": source.tgt_text,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ids_cannot_alias_legacy_audio_or_each_other() {
        let ids = ["s1.1", "s11", "s1.2", "s12", "s0+s1", "s0s1", "", "../s11", "реплика"];
        let keys: std::collections::HashSet<_> = ids.iter().map(|id| key(id)).collect();
        assert_eq!(keys.len(), ids.len());
        assert_eq!(key("s11"), "s11");
        assert_ne!(key(&key("s1.1")), key("s1.1"));
    }

    #[test]
    fn donor_keeps_original_interval_after_split_and_numeric_reordering() {
        let mut p = Project::default();
        p.segments = vec![
            Segment { id: "s0".into(), start: 1.0, end: 5.0, src_text: "original".into(), ..Default::default() },
            Segment { id: "s1".into(), voice: Some("donor:1".into()), ..Default::default() },
        ];
        let mut after = vec![p.segments[1].clone()];
        preserve_donors(&p, &mut after);
        p.segments = after;
        let source = donor(&p, &p.segments[0]).unwrap();
        assert_eq!((source.start, source.end), (1.0, 5.0));
        assert_eq!(source.src_text, "original");
        p.segments[0].voice = Some("actor.wav".into());
        assert!(donor(&p, &p.segments[0]).is_none());
    }

    #[test]
    fn audio_key_is_stable_and_clean() {
        let mut original = Segment { id: "s0".into(), ..Default::default() };
        assert_eq!(audio_key(&original), "s0");
        invalidate_audio(&mut original);
        assert!(original.dirty);
        assert_eq!(audio_key(&original), "s0");
    }
}
