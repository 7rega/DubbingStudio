# Dub Studio — Rust-порт (crates/)

Порт Dub Studio с Python/FastAPI на нативный стек Rust (Tauri 2 + axum). Фронт (`frontend/`) НЕ
переписывается — Rust-сервер отдаёт тот же REST/SSE контракт, что `backend/app.py` (см.
`docs/PORT-CONTRACT.md`). Python-код (`backend/`, `dub-engine/`) остаётся референсом до паритета.

## Крейты

| Крейт | Назначение | Референс (Python) |
|-------|-----------|-------------------|
| `audiocpp` | FFI над `audiocpp_engine.dll` — Higgs Audio v3 TTS + клон голоса. Самостоятельный (без Tauri). | `dubengine/tts.py`, `voices.py` |
| `dub-asr` | ASR со словными таймстемпами (Parakeet-TDT-v3) + диаризация (Sortformer v2) через parakeet-rs/ONNX. | `dubengine/asr.py`, `diarize.py` |
| `dub-core` | Типы `Project` (serde, extra="allow" round-trip) и `EngineOpts`. | `dubengine/project.py`, `opts.py` |
| `dub-sep` | Вокал/инструментал сепарация — Mel-Band Roformer voc_fv6-Q8_0 через BSRoformer.cpp (сайдкар). Инструментал = mix−vocals. | `dubengine/separate.py` (движок ЗАМЕНЁН приказом юзера) |
| `dub-captions` | ASS-субтитры (build: титры+дублированные субтитры, 26 пресетов) + ffmpeg/libass burn (gblur+оверлей, NVENC). Метрики — ab_glyph. | `dubengine/captions.py` |
| `dub-ocr` | Экранный OCR (PP-OCR DBNet det + CRNN rec ONNX) → блюр-боксы вшитого текста + субтитр-полоса. Свой ort-пайплайн (rc.12 load-dynamic). | `dubengine/text_detect.py`, `compose.py` |
| `dub-server` | axum: SPA, capabilities, upload, SSE-джобы, **analyze** (ASR+диар+перевод+vision+OCR), **render** (сепарация→TTS→сведение→burn→mux), output/original/dub, PATCH. | `backend/app.py`, `dubengine/pipeline.py` |

## Сборка

```powershell
# PowerShell (cargo виден сразу)
cargo build --workspace
```
```bash
# Git Bash: сначала прокинуть cargo в PATH
export PATH="$(cygpath -u "$USERPROFILE/.cargo/bin"):$PATH"
cargo build --workspace --examples
```

## Модели и рантайм (не коммитятся, лежат в `models/`)

- **onnxruntime.dll** — `dub-asr` собран с `ort/load-dynamic`: onnxruntime грузится в рантайме.
  Указывается через `ORT_DYLIB_PATH=<...>\onnxruntime.dll`. **КРИТИЧНО: строго onnxruntime 1.24.x**
  (ort 2.0-rc.12 собран под 1.24.2). DLL версий 1.22/1.23 вызывают ДЕДЛОК в `commit_from_file`
  (рассинхрон OrtApi, поток блокируется наглухо, без ошибки). Официальный билд:
  github.com/microsoft/onnxruntime releases → `onnxruntime-win-x64-1.24.2.zip` → `lib/onnxruntime.dll`.
- **Parakeet-TDT-0.6b-v3 (ONNX), int8** — дефолт Higgs-Ultimate `tdt-0.6b-v3-int8` (★ recommended,
  ~670МБ). HF `istupakov/parakeet-tdt-0.6b-v3-onnx`, файлы:
  `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `vocab.txt` (+`config.json`, `nemo128.onnx`).
  Каталог `models/tdt/`. parakeet-rs автоопределяет имена; когда в папке ТОЛЬКО .int8.onnx — грузится int8
  (тот же 8-битный дефолт, что в Python-референсе `asr.py` `quantization="int8"`). **int8 требует Level1**
  (см. ниже): дефолтный parakeet-rs Level3 виснет при создании CPU-сессии на квант-узлах
  (DynamicQuantizeLinear/MatMulInteger). GPU-провайдер (`--features cuda`) снимает ограничение.
- **Sortformer v2** — HF `altunenes/parakeet-rs`, `diar_streaming_sortformer_4spk-v2.onnx` → `models/sortformer/`.
- **Higgs Audio v3 веса, Q8_0** — дефолт Higgs-Ultimate `higgs-q8_0` (★ recommended, gguf ~5.4ГБ). HF
  `drbaph/Higgs-Audio-v3-Studio`, путь `models/higgs-q8_0/`: `q8_0.gguf` + `config.json`,
  `chat_template.jinja`, `tokenizer.json`, `tokenizer_config.json`, `higgs_audio_v2_tokenizer_config.json`.
  Локально `models/higgs-q8_0/` (`--model-root`). DLL движка — `models/higgs-engine/audiocpp_engine.dll`
  (из `drbaph/.../engines/audiocpp_engine.dll` или релиза Higgs-Ultimate).

## Примеры

### ASR (словные таймстемпы + опц. диаризация)
```bash
export ORT_DYLIB_PATH="D:\Projects\TEMP\dub-studio\models\runtime\onnxruntime.dll"
# транскрипция:
./target/release/examples/asr --wav in.wav --tdt models/tdt
# с диаризацией:
./target/release/examples/asr --wav in.wav --tdt models/tdt --diarize --sortformer models/sortformer/diar_streaming_sortformer_4spk-v2.onnx
```
Вывод: JSON `{"segments":[{start,end,text,words:[{word,start,end}]}]}` (или `{turns, segments}` с --diarize).

`DUB_ASR_OPT_LEVEL` (0..3, дефолт 1) — уровень оптимизации графа ONNX. **Важно:** на int8-кванте
дефолтный parakeet-rs Level3 виснет при создании CPU-сессии на минуты; крейт понижает до Level1.
GPU-провайдер (`--features cuda` + CUDA EP) снимает это ограничение.

### synth (TTS / клон голоса через Higgs DLL) — Q8_0 на CUDA
```bash
# движок ищет cuda/ggml/vcruntime DLL в своём каталоге -> держим их рядом с audiocpp_engine.dll
# (models/higgs-engine/: cudart64_13, cublas64_13, cublasLt64_13, MSVCP140/VCOMP140/VCRUNTIME140*)
./target/release/examples/synth --dll models/higgs-engine/audiocpp_engine.dll \
    --model-root models/higgs-q8_0 --text "Привет" --backend cuda --device 0 --out out.wav
# клон голоса:
./target/release/examples/synth --dll models/higgs-engine/audiocpp_engine.dll \
    --model-root models/higgs-q8_0 --text "..." \
    --ref-wav ref.wav --ref-text "референсный текст" --backend cuda --device 0 --out out.wav
```

### Сервер
```bash
DUB_STUDIO_ROOT=<repo> ORT_DYLIB_PATH=<...>/onnxruntime-1.24.dll ./target/release/dub-server
#   слушает 127.0.0.1:8765 (порт: env DUB_STUDIO_PORT). Раздаёт SPA (frontend/dist) + API.
```

### Десктоп (Tauri-оболочка)
```bash
# frontend собрать заранее: (cd frontend && npm ci && npm run build)
cd desktop && npx tauri build --no-bundle      # -> desktop/src-tauri/target/release/dub-studio-desktop.exe
```
Оболочка поднимает `dub-server` на 127.0.0.1:<свободный порт> и открывает окно на этот URL.
Портатив: рантайм (onnxruntime.dll, models/) держится рядом с exe; WEBVIEW2_USER_DATA_FOLDER там же.

## Статус (раунд 2)

Собрано и проверено (`cargo build --workspace --examples` + `cargo test --workspace` зелёные, 9 тестов):
- **Higgs Q8_0 синтез** реален: TTS русской фразы и voice_clone прогнаны на CUDA (RTX 4090), движок
  `audiocpp_engine.dll` v0.2.3 + self-contained CUDA13/vcruntime DLL. Артефакты — не тишина (-24dB).
- **analyze-ядро** (`POST /projects/{pid}/analyze`) как джоба: ffmpeg extract 16k mono → Sortformer
  turns (single-speaker деградация при <2 спикеров) → TDT int8 словные таймстемпы → Project. Прогнан
  на реальном ролике (docs/example_original.mp4): 11 сегментов, spk=0, слова, валидный русский UTF-8.
- **PATCH** segment/subpos/mode (атомарно tmp+rename), проверено на живом сервере.
- **Tauri-оболочка** desktop/src-tauri: `--no-bundle` собрана, запуск поднимает сервер на рандом-порту.

Следующий раунд (3): перевод/vision (llama.cpp + Gemma), separation, OCR, captions/render — карта в
`docs/PORT-CONTRACT.md`.

## Статус (раунд 1)

Каркас сервера (capabilities, upload, get project, SSE-джобы, SPA); `dub-core` round-trip и `dub-asr`
сегментация — тесты проходят; ASR-пример прогнан на реальном речевом WAV.
