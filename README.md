<div align="center">

<img src="frontend/public/favicon.svg" width="72" alt="Dub Studio logo"/>

# Dub Studio

**Free, offline AI video dubbing studio for Windows — re-voice any video into another language with a cloned voice, translated captions, and on‑screen‑text localization. 100% local, zero Python: one native `.exe` (Rust + C++/CUDA); every model and engine downloads with a button.**

[![License](https://img.shields.io/github/license/timoncool/dub-studio?style=flat-square)](LICENSE)
[![Stars](https://img.shields.io/github/stars/timoncool/dub-studio?style=flat-square)](https://github.com/timoncool/dub-studio/stargazers)
[![Latest release](https://img.shields.io/github/v/release/timoncool/dub-studio?include_prereleases&style=flat-square)](https://github.com/timoncool/dub-studio/releases)
[![Downloads](https://img.shields.io/github/downloads/timoncool/dub-studio/total?style=flat-square)](https://github.com/timoncool/dub-studio/releases)
[![Last commit](https://img.shields.io/github/last-commit/timoncool/dub-studio?style=flat-square)](https://github.com/timoncool/dub-studio/commits)

**English** · [Русский](README.ru.md) · [中文](README.zh.md) · [Español](README.es.md) · [Português](README.pt.md) · [Français](README.fr.md)

### 🌐 [Live demo & before/after video showcase →](https://timoncool.github.io/dub-studio/)

![Dub Studio — AI video dubbing on Windows](docs/shots/mode-dub-ru.png)

</div>

## What it is

**Dub Studio** turns any video into a dubbed version in another language — **with the speaker's own voice cloned, captions translated, and on‑screen text localized right on the frame**. Drop a clip, and a smart auto‑pass builds the first draft; then a live editor puts **every caption, voice, blur box, font and title** under your control with an instant preview.

By default everything runs **locally on your machine** — no cloud, no subscription; your footage and your voiceprint never leave your computer. And if your PC is weak (can't run the local Gemma/Higgs) or you want more speed and quality, the heavy stages (translation, vision, TTS, transcription) can **optionally** be offloaded to the cloud via **OpenRouter** — each engine picked independently (local ↔ cloud), with voices auto-cast by speaker gender (beta). The key is stored locally; everything is off by default.

It's a **fully native rewrite**. No embeddable Python, no torch, no CUDA wheels. The whole pipeline is **Rust + native C++/CUDA engines (GGUF/ONNX)**: one process, fast startup, low VRAM. Models, engines, CUDA/VC++ runtime and ffmpeg are **downloaded and installed by the app itself** on first run. **NVIDIA is recommended but not required** — separation ships a CPU build, diarization and ASR run on CPU, and the heavy stages (translation, vision, TTS) go to the cloud, so a dub can be built on a machine with no NVIDIA at all.

## See it in action

**[▶ Watch the before/after video showcase →](https://timoncool.github.io/dub-studio/#showcase)** — real clips dubbed end‑to‑end on a local GPU: different videos, different modes, different languages, nothing left the machine.

| ![Full dub, Russian UI](docs/shots/mode-dub-ru.png) | ![Voice-over, Spanish UI](docs/shots/mode-voiceover-es.png) | ![Full dub, Chinese UI, CJK captions](docs/shots/mode-dub-zh.png) |
|:--:|:--:|:--:|
| 🎙️ **Full dub** · EN→RU | 🗣️ **Voice-over** · EN→ES | 🈶 **Full dub** · →中文, CJK on frame |
| ![Subtitles, Russian UI](docs/shots/mode-subtitles-ru.png) | ![Full dub, widescreen, French UI](docs/shots/mode-dub-cinema-fr.png) | ![Transcript, Portuguese UI](docs/shots/mode-transcribe-pt.png) |
| 📝 **Subtitles** · original-lang | 🎬 **Full dub** · widescreen 16:9 | 🔤 **Transcript** · diarized |

## Five modes, switchable on the fly

| Mode | What it does |
|------|--------------|
| 🎙️ **Dub** | Full re-voice into the target language with the **original timbre cloned** — auto-cast per speaker or pick a voice |
| 🗣️ **Voice-over** | Translated voice **over the ducked original** — the source is still audible underneath; balance is adjustable |
| 📝 **Subtitles** | Burn **original-language** captions, keep the original audio — no dubbing, no translation |
| ✨ **Funny remix** | Give a theme ("as a pirate", "as a news report") → the model **rewrites the whole script**, then re-dubs |
| 🎬 **Transcript** | Clean **diarized transcript** with per-speaker layout, karaoke play-along, one-click voice creation, `.srt`/`.txt` export |

Load a clip once and send it into any mode — right inside the editor.

## Features

- **Voice cloning** — the original timbre is cloned and speaks the new language (native [Higgs Audio v3](https://huggingface.co/bosonai) engine, GGUF). Auto-cast by speaker or bring your own voice from a pack.
- **Speaker diarization** — who speaks and when (NVIDIA **Sortformer** v2, up to 4 voices), a distinct voice per speaker.
- **Character casting (beta)** — a character is a **face + voice** pair. The app gathers faces across the whole video, recognizes the same person and **binds them to a speaker by co-occurrence** (the one on camera in close-up gets the voice, a background listener doesn't); it auto-picks the clearest avatar frame and **saves a casting profile for the whole series** — assign voices and character descriptions once, and the **next episode applies them automatically**. A **real-faces / cartoon·anime** toggle switches the face detection accordingly.
- **Choice of ASR engine** — transcribe with **Parakeet-TDT** (GPU, default) or **Whisper** ([Purfview faster-whisper standalone](https://github.com/Purfview/whisper-standalone-win), runs on CPU) — pick the model size (tiny … large-v3-turbo) and quant (compute type) right in settings.
- **Import ready-made subtitles** — bring your own `.srt`/`.ass` as the exact transcript: text and timing come straight from the file instead of auto-recognition (speakers are still auto-assigned by diarization). Tick **“subtitles already in the target language”** and translation is skipped too — an English clip + your Russian subs → a Russian dub straight from them, no ASR and no MT.
- **Multi-language export** — the **▾** next to Export sends one video into several languages at once; each inherits all your edits (subtitle layout, styles, blur boxes, cloned voice) — only the text is re-translated and re-voiced.
- **Save & reopen projects** — autosave, a list of recent projects on the start screen, and jump back into unfinished work in one click.
- **Searchable voice & language pickers** — type part of a name to filter hundreds of voices or 100+ languages; languages also match by their name in your UI language.
- **Composable pipeline** — independent toggles at the input: audio (original / dub / voiceover / transcript) × subtitles (none / original / translated) × burn-in on/off × funny remix. Any combination — dub without subtitles, translated subtitles without dubbing, funny dub with your own voices — in batch and in the editor too.
- **On-screen text localization** — OCR detects baked-in text (**PP-OCR** ONNX), **blurs the original** and prints a localized title on top in a matched style — a feature no other tool has.
- **Translation + vision style analysis** — the transcript is translated locally with **Gemma-4 12B** (GGUF, llama.cpp); a vision pass reads the frame layout: caption style, titles, brands, text zones.
- **SOTA vocal separation** — **Mel-Band Roformer** (native BSRoformer.cpp on CUDA) splits voice from music, so the backing track is **preserved** and the clone latches onto clean speech.
- **26 caption presets** — karaoke / word-by-word / hormozi / neon and more, rendered **on your own frame** (WYSIWYG, JASSUB over the same `.ass` that ffmpeg burns).
- **Karaoke transcript** — play the video and follow along as the current line and the current **word** light up in the transcript.
- **Live editor** — edit transcript, voices, caption style, blur boxes, titles; **~0.17 s/frame preview**, every change visible instantly.
- **Smart re-gen** — export re-synthesizes and recomputes **only the segments you changed**, not the whole clip.
- **Add your own lines** — insert custom phrases into the transcript; each is voiced in the speaker's cloned voice and shown in the subtitles.
- **Batch processing** — a queue of files, all run with one setup, per-file progress.
- **Before/after compare** — original and dub side by side.
- **100+ languages** — dub into any major language (Spanish, Chinese, Japanese, Arabic, Hindi and more), with source-language auto-detect.
- **Any video format** — MP4, MOV, MKV, WEBM, AVI and more (decoded via ffmpeg).
- **One-button setup + resumable downloads + in-app auto-update** — models, engines, CUDA/VC++ runtime and ffmpeg download on first run; large models (10 GB+) **resume from where they stopped** after a dropped connection instead of restarting; the app updates itself.
- **Run each stage where you want** — separation, diarization and ASR each switch independently between **GPU, CPU, and cloud**; combine them however you like. With the heavy models (translation, vision, TTS) offloadable to **OpenRouter**, the whole pipeline runs even on a machine with **no NVIDIA**.
- **Tune for your hardware** — every engine ships multiple quants (TTS Q8/Q6/Q4, translation Q4…Q8, ASR int8/fp32 or Whisper tiny…large-v3-turbo, separation Q8/Q5/Q4) — switch in settings; cap the **prefill batch** and **voice-reference length** to fit 8–12 GB GPUs and 32 GB RAM.
- **Fully portable** — nothing is written to your user profile; delete the folder and no trace remains.

## Screenshots

Home — five modes, a preview of the selected video, language pickers, any video format:

![Dub Studio home screen](docs/screenshot-home.png)

Transcript mode — diarized transcript with per-speaker layout, karaoke play-along and one-click voice creation from each speaker:

![Dub Studio transcript mode](docs/screenshot-transcribe.png)

## Requirements

- **OS:** Windows 10 / 11 (x64)
- **GPU:** NVIDIA with 8–16 GB VRAM recommended — **or none**: separation, diarization and ASR run on CPU, and the heavy models go to the cloud
- **WebView2** — preinstalled on Windows 11 (installs automatically on Windows 10)
- **Disk:** ~15 GB for models, engines and runtime (fetched on first run), plus room for your projects

On an NVIDIA machine the only thing you install by hand is a recent **[NVIDIA driver](https://www.nvidia.com/Download/index.aspx)**. Everything else — models (Higgs Audio v3, Gemma-4 12B + vision, Parakeet-TDT, Sortformer, Mel-Band Roformer), engines, CUDA runtime and ffmpeg — the app downloads with a button on first run.

## Quick start

1. **Download** the portable build from [Releases](https://github.com/timoncool/dub-studio/releases) and unzip anywhere (or install via `-setup.exe` / `.msi`).
2. **Run** `Dub Studio.exe`.
3. On the **First-run** panel press **Download all** — the app fetches models, engines and runtime (~15 GB, once). If the NVIDIA driver is missing, the button opens the download page.
4. **Drop a video**, pick a target language → the auto-pass makes the first draft. Fine-tune everything in the editor and hit **Export**.

> Everything downloads and lives **inside the app folder**. Models, caches and projects go nowhere else.

## How it works

`analyze()` is a fixed first pass: separation → ASR with word timings → diarization → context translation + vision (caption style / titles / brands) → OCR (layout / blur boxes). The result is an editable **Project** document. Each edit is a patch on that Project with a ~0.17 s/frame preview; export re-runs **only the dirtied stages**.

**Stack:** a native **Tauri 2 (Rust)** shell spawns `dub-server` (axum) on a local port and opens a window onto the SPA — React 19 + Vite + Tailwind + react-konva over JASSUB. Engines: Parakeet-TDT or Whisper (ASR) · Sortformer (diarization) · Gemma-4-12B GGUF (translation + vision, llama.cpp) · Higgs Audio v3 (TTS) · Mel-Band Roformer (separation, BSRoformer.cpp) · PP-OCR (ONNX) · ffmpeg/NVENC. **Not a single Python process at runtime.**

### Build from source

```bash
git clone https://github.com/timoncool/dub-studio.git
cd dub-studio

cd frontend && npm install && npm run build && cd ..   # 1) SPA
cargo build --release -p dub-server                     # 2) native server (axum)
cd desktop && npm install && npx tauri build            # 3) desktop shell (Tauri)
```

Needs Node 20+, Rust (MSVC toolchain) and WebView2. Native engines (`audiocpp_engine.dll`, llama.cpp, BSRoformer.cpp, ONNX Runtime) don't need rebuilding — the app downloads prebuilt binaries.

## More portable AI apps

| Project | What it is |
|--------|----------|
| [Higgs Ultimate](https://github.com/timoncool/Higgs-Ultimate) | Native speech synthesis & voice cloning (Higgs Audio v3) |
| [ACE-Step Studio](https://github.com/timoncool/ACE-Step-Studio) | AI music studio — songs, vocals, covers, clips |
| [Foundation Music Lab](https://github.com/timoncool/Foundation-Music-Lab) | Music generation + timeline editor |
| [Qwen3-TTS](https://github.com/timoncool/Qwen3-TTS_portable_rus) | Portable TTS with voice cloning |
| [VibeVoice ASR](https://github.com/timoncool/VibeVoice_ASR_portable_ru) | Portable speech recognition |
| [SuperCaption Qwen3-VL](https://github.com/timoncool/SuperCaption_Qwen3-VL) | Portable image captioning |

## Contributing & forks

**Collaborators are very welcome.** I'd be genuinely happy to see Dub Studio forked to other platforms and GPUs — the architecture is capable of it, I simply don't have the bandwidth to do the ports myself. If you want it on **AMD / Intel GPUs, macOS or Linux**, fork it and go — PRs welcome.

**Extra localizations** are just as welcome: the app and landing ship in 6 languages today — translate the locale files (`frontend/src/locales/` and the dict in `docs/index.html`) and open a PR to add yours.

## Authors

- **Nerual Dreming** — [Telegram](https://t.me/nerual_dreming) | [neuro-cartel.com](https://neuro-cartel.com) | founder of [ArtGeneration.me](https://artgeneration.me)
- **Neuro-Soft** — [Telegram](https://t.me/neuroport) | portable AI apps

## Credits

- **[Boson AI](https://huggingface.co/bosonai)** — the Higgs Audio v3 model, and **[drbaph / Higgs-Audio-v3-Studio](https://huggingface.co/drbaph/Higgs-Audio-v3-Studio)** — GGUF quants and the native `audiocpp_engine.dll`.
- **[NVIDIA Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)** and **[Sortformer](https://huggingface.co/nvidia)** — ASR and diarization; ONNX weights from [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) and [altunenes/parakeet-rs](https://github.com/altunenes/parakeet-rs).
- **[Google Gemma](https://huggingface.co/google/gemma-4-12b-it-qat-q4_0-gguf)** — Gemma-4 12B (translation + vision), via [llama.cpp](https://github.com/ggml-org/llama.cpp).
- **[chenmozhijin / BSRoformer.cpp](https://github.com/chenmozhijin/BSRoformer.cpp)** and **[GaboxR67](https://huggingface.co/GaboxR67)** — the native engine and Mel-Band Roformer model.

## Support the author

I build open-source software and do AI research — most of what I make is freely available. Donations let me build and research more.

**[All the ways to support](DONATE.md)** | **[dalink.to/nerual_dreming](https://dalink.to/nerual_dreming)** | **[boosty.to/neuro_art](https://boosty.to/neuro_art)**

- **BTC:** `1E7dHL22RpyhJGVpcvKdbyZgksSYkYeEBC`
- **ETH (ERC20):** `0xb5db65adf478983186d4897ba92fe2c25c594a0c`
- **USDT (TRC20):** `TQST9Lp2TjK6FiVkn4fwfGUee7NmkxEE7C`

## Star history

<a href="https://github.com/timoncool/dub-studio/stargazers">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="docs/stars-dark.svg" />
   <source media="(prefers-color-scheme: light)" srcset="docs/stars-light.svg" />
   <img alt="Star history chart" src="docs/stars-light.svg" />
 </picture>
</a>

## License

App code is [MIT](LICENSE). Model weights keep their own licenses (Higgs Audio v3 — Boson AI research/non-commercial; Gemma — Gemma Terms; etc.) — audited before every release.

<sub>AI video dubbing · voice cloning · video translation · automatic subtitles · speaker diarization · offline · local · open source · Windows · free lip-free dubbing · voice-over · transcription</sub>
