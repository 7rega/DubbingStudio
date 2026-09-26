//! dub-core — типы Project (serde) и EngineOpts, зеркало Pydantic-контракта dub-engine.

mod opts;
mod project;

pub use opts::EngineOpts;
pub use project::{
    Audio, Brand, BlurBox, CaptionOverride, Captions, Meta, Preset, Project, Render, Segment,
    SubStyle, Subs, Title, Voice,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_project_roundtrips() {
        let p = Project::default();
        let j = p.to_json().unwrap();
        let p2 = Project::from_json(&j).unwrap();
        assert_eq!(p2.mode, "dub");
        assert_eq!(p2.tgt_lang, "en");
        assert_eq!(p2.render.burn_cq, 24);
        assert_eq!(p2.audio.voice.mode, "clone");
    }

    #[test]
    fn unknown_fields_passthrough() {
        // extra="allow": неизвестные поля должны пережить round-trip.
        let src = r#"{"mode":"dub","tgt_lang":"ru","future_field":42,
            "segments":[{"id":"s0","start":0.0,"end":1.5,"src_text":"hi","gui_only":true}],
            "captions":{"blur_boxes":[{"x":1,"y":2,"w":3,"h":4}]}}"#;
        let p = Project::from_json(src).unwrap();
        assert_eq!(p.tgt_lang, "ru");
        assert!(p.extra.contains_key("future_field"));
        assert_eq!(p.segments[0].extra.get("gui_only").unwrap(), &serde_json::json!(true));
        let out = p.to_json().unwrap();
        assert_eq!(out.contains("future_field"), true);
        assert_eq!(out.contains("gui_only"), true);
    }

    #[test]
    fn audio_tts_settings_defaults_and_roundtrips() {
        let p = Project::default();
        assert_eq!(p.audio.fish_prompt, "");
        assert_eq!(p.audio.fish_temp, 0.8);
        assert_eq!(p.audio.fish_clean_ref, true);
        assert_eq!(p.audio.fish_seed, None);
        assert_eq!(p.audio.vox_prompt, "");
        assert_eq!(p.audio.vox_steps, 20);
        assert_eq!(p.audio.vox_cfg, 1.6);
        assert_eq!(p.audio.vox_seed, None);

        let j = p.to_json().unwrap();
        let p2 = Project::from_json(&j).unwrap();
        assert_eq!(p2.audio.fish_temp, 0.8);
        assert_eq!(p2.audio.fish_clean_ref, true);
        assert_eq!(p2.audio.vox_steps, 20);
        assert_eq!(p2.audio.vox_cfg, 1.6);
    }
}
