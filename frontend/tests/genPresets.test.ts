import { test } from "node:test";
import assert from "node:assert/strict";
import { extractPresetFromProject, applyPresetToProject, isProjectMatchingPreset } from "../src/lib/genPresets.ts";
import type { Project } from "../src/lib/api.ts";

function createMockProject(): Project {
  return {
    meta: { video: "v.mp4", duration: 10, width: 1920, height: 1080, fps: 30, src_codec: "h264" },
    mode: "dub",
    tgt_lang: "ru",
    audio: {
      keep_music: true,
      voice: { mode: "clone", name: null },
      drama_prompt: "[whisper]",
      drama_temp: 1.15,
      drama_seed: 777,
      drama_audio_only: true,
      vox_prompt: "calm tone",
      vox_steps: 25,
      vox_cfg: 1.8,
      vox_seed: 1234,
      higgs_temp: 0.85,
      higgs_seed: 5678,
      voice_gain_db: 1.5,
      music_gain_db: -2.0,
      gain_db: 0,
      dub_mix_mode: "separated",
      voiceover_gain_db: -12,
      voiceover_duck: "dynamic",
    },
    segments: [],
    subs: { mode: "none" },
    render: { blur: false, blur_sigma: 60 },
    captions: {
      preset: { name: "boxed" },
      sub_style: {
        font: "Montserrat",
        size_px: 36,
        bold: true,
        italic: false,
        uppercase: false,
        align: "center",
        color: "#FFFFFF",
        outline: "#000000",
        outline_w: 2,
        shadow_dir: null,
        plate: false,
      },
      overrides: [],
    },
  };
}

test("extractPresetFromProject preserves Drama 3 settings", () => {
  const p = createMockProject();
  const preset = extractPresetFromProject("MyDramaPreset", p, "drama3");

  assert.equal(preset.tts.engine, "drama3");
  assert.equal(preset.tts.drama_prompt, "[whisper]");
  assert.equal(preset.tts.drama_temp, 1.15);
  assert.equal(preset.tts.drama_seed, 777);
  assert.equal(preset.tts.drama_audio_only, true);
  assert.ok(isProjectMatchingPreset(p, preset));
});

test("isProjectMatchingPreset detects changes in Drama 3 settings", () => {
  const p = createMockProject();
  const preset = extractPresetFromProject("MyDramaPreset", p, "drama3");

  // Изменение prompt
  const pChangedPrompt = { ...p, audio: { ...p.audio, drama_prompt: "[laughs]" } };
  assert.equal(isProjectMatchingPreset(pChangedPrompt, preset), false);

  // Изменение temp
  const pChangedTemp = { ...p, audio: { ...p.audio, drama_temp: 1.25 } };
  assert.equal(isProjectMatchingPreset(pChangedTemp, preset), false);

  // Изменение seed
  const pChangedSeed = { ...p, audio: { ...p.audio, drama_seed: 999 } };
  assert.equal(isProjectMatchingPreset(pChangedSeed, preset), false);

  // Изменение audio_only
  const pChangedAudioOnly = { ...p, audio: { ...p.audio, drama_audio_only: false } };
  assert.equal(isProjectMatchingPreset(pChangedAudioOnly, preset), false);
});

test("applyPresetToProject restores Drama 3 settings", () => {
  const p = createMockProject();
  const preset = extractPresetFromProject("MyDramaPreset", p, "drama3");

  // Проект со сброшенными настройками
  const blankProject = {
    ...p,
    audio: {
      ...p.audio,
      drama_prompt: "",
      drama_temp: null,
      drama_seed: null,
      drama_audio_only: false,
    },
  };
  assert.equal(isProjectMatchingPreset(blankProject, preset), false);

  const restored = applyPresetToProject(blankProject, preset);
  assert.equal(restored.audio.drama_prompt, "[whisper]");
  assert.equal(restored.audio.drama_temp, 1.15);
  assert.equal(restored.audio.drama_seed, 777);
  assert.equal(restored.audio.drama_audio_only, true);
  assert.ok(isProjectMatchingPreset(restored, preset));
});
