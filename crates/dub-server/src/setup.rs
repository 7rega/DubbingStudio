//! setup — «первый запуск»: манифест всех внешних компонентов (модели, движки-сайдкары, системные
//! библиотеки) + диагностика их наличия на диске + фоновая автозакачка одной кнопкой: свести
//! ручную установку к
//! единственному шагу (драйвер NVIDIA), убрав из README требование ставить CUDA Toolkit, VC++ и
//! качать веса вручную.
//!
//! Эталон паттернов — Higgs-Ultimate desktop/src-tauri/src/envdeps.rs (env_check / download_env_deps /
//! extract_dlls_from_wheel) и download.rs (прогресс-машина). Здесь адаптировано под axum-сервер: тот же
//! job-контракт SSE, что analyze/render, а прогресс идёт из download-loop в колбэк джобы.
//!
//! Классы источников (все URL — ровно те, что уже использованы; см. crates/README.md, PORT-CONTRACT.md,
//! Higgs voiceclean.rs):
//!   • модели — прямые файлы HF (higgs-q8_0/*, gemma-4 + mmproj, parakeet-tdt int8, sortformer, roformer
//!     voc_fv6-Q8_0);
//!   • сайдкары/движки — zip-релизы GitHub (BSRoformer.cpp, llama.cpp win-cuda-13.3, onnxruntime 1.24.2,
//!     ffmpeg BtbN) + audiocpp_engine.dll (HF);
//!   • CUDA-runtime — PyPI-wheel'ы NVIDIA (cudart 13.3.29 / cublas 13.6.0.2), распаковка *.dll плоско;
//!   • VC++ runtime + OCR-модели — БАНДЛ (кладутся в релиз рядом с exe, как VC++ в Higgs); не качаются,
//!     но статус показываем;
//!   • драйвер NVIDIA — детект (nvcuda.dll), «скачивание» = открыть сайт (кнопка во фронте).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{json, Value};

// ── Тип файла-члена компонента ──────────────────────────────────────────────

/// Одна закачиваемая единица компонента: URL + относительный путь назначения (от repo_root) + ожидаемый
/// размер (для проверки «докачано целиком» и для UI). Для zip/wheel — это временный архив, распаковка
/// раскладывает содержимое (см. `Extract`).
#[derive(Clone, Debug)]
pub struct FileSpec {
    pub url: &'static str,
    /// Куда лечь ФАЙЛУ (для прямых файлов) ИЛИ временный путь архива (для zip/wheel). Относительно repo_root.
    pub dest_rel: &'static str,
    /// Ожидаемый размер в байтах (0 = неизвестно/rolling-релиз). Для файлов-моделей — точный (сверен с диском).
    pub size: u64,
    pub extract: Extract,
}

/// Что делать со скачанным файлом.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extract {
    /// Прямой файл — оставить как есть (dest_rel = финальный путь).
    None,
    /// zip: распаковать плоско (только имена файлов) в каталог dest_rel-родителя, затем удалить архив.
    ZipFlat,
    /// zip: отобрать конкретные файлы по имени листа (ffmpeg.exe/ffprobe.exe) и положить плоско в каталог.
    ZipPick,
    /// zip: распаковать ВЕСЬ архив с сохранением поддерева в каталог. Для onnxruntime — чтобы получить
    /// `onnxruntime-win-x64-1.24.2/lib/onnxruntime.dll` ровно там, где его ищет dub-asr::ensure_ort_dylib.
    ZipTree,
    /// wheel (zip): достать все *.dll плоско в каталог, затем удалить архив (CUDA runtime).
    WheelDlls,
    /// 7z: распаковать с сохранением структуры каталогов (требует tar из Windows 10/11+)
    SevenZipTree,
}

/// Обязательность компонента для запуска пайплайна.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Requirement {
    /// Без него analyze/render не работает — блокирует «первый запуск».
    Required,
    /// Улучшает результат, но пайплайн деградирует gracefully (напр. OCR-блюр).
    Recommended,
    /// Альтернативный вариант/квант (качается по выбору в настройках, не преселектится, не гейтит).
    Optional,
}

/// Как компонент попадает на диск.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Качается сетью (есть file_specs с URL).
    Download,
    /// Идёт в комплекте релиза рядом с exe (VC++, OCR-модели). Не качаем; если нет — просим переустановить.
    Bundled,
    /// Часть системы, ставится вне приложения (драйвер NVIDIA). «Скачать» = открыть сайт.
    External,
}

/// Описание одного компонента манифеста.
#[derive(Clone, Debug)]
pub struct Component {
    pub id: &'static str,
    /// Человекочитаемое имя (i18n-ключ на фронте — `setup.comp.<id>.name`; здесь дефолт-строка RU для API).
    pub name: &'static str,
    /// Назначение (что сломается без него).
    pub purpose: &'static str,
    pub requirement: Requirement,
    pub delivery: Delivery,
    /// Совокупный размер компонента, байт (сумма файлов; для rolling-релизов — оценка).
    pub size: u64,
    /// Файлы к закачке (для Delivery::Download). Пусто у Bundled/External.
    pub files: &'static [FileSpec],
    /// Пути-«маркеры» существования (относительно repo_root). Компонент installed, когда ВСЕ маркеры на
    /// месте и (для файлов с известным размером) их размер совпадает.
    pub markers: &'static [Marker],
    /// URL внешней страницы (для Delivery::External — сайт драйвера).
    pub external_url: Option<&'static str>,
}

/// Маркер наличия: путь + опц. минимальный размер (0 = только существование).
#[derive(Clone, Copy, Debug)]
pub struct Marker {
    pub rel: &'static str,
    /// Ожидаемый точный размер (0 = не проверять). Файл «целый», если размер == expect (для докачки/резюме).
    pub expect: u64,
}

// ═══════════════════════════════════════════════════════════════════════════
//  URL-константы (источник истины; сверены HEAD-запросами, размеры = байт-в-байт с диском)
// ═══════════════════════════════════════════════════════════════════════════

// HF: Sortformer v2 диаризация (altunenes/parakeet-rs).
const HF_SORTFORMER: &str =
    "https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.onnx";
// HF: Mel-Band Roformer voc_fv6-Q8_0 (chenmozhijin/BSRoformer-GGUF).
const HF_ROFORMER: &str = "https://huggingface.co/chenmozhijin/BSRoformer-GGUF/resolve/main/GaboxR67/MelBandRoformers/melbandroformers/vocals/voc_fv6-Q8_0.gguf";
// GitHub: BSRoformer.cpp движок win-cuda-13.1.0 zip (chenmozhijin/BSRoformer.cpp v0.1.0).
const GH_BSROFORMER_ENGINE: &str =
    "https://github.com/chenmozhijin/BSRoformer.cpp/releases/download/v0.1.0/BSRoformer-windows-cuda-13.1.0.zip";
// GitHub: BSRoformer.cpp CPU-сборка (win-x64-msvc, без CUDA) — сепарация на процессоре: медленнее,
// но полная функция. Статический exe 671КБ; MSVC-рантайм уже вшит компонентом vcruntime.
const GH_BSROFORMER_ENGINE_CPU: &str =
    "https://github.com/chenmozhijin/BSRoformer.cpp/releases/download/v0.1.0/BSRoformer-windows-x64-msvc.zip";
// GitHub: llama.cpp win-cuda-13.3 (ggml-org/llama.cpp; пин на стабильный билд + cudart-компаньон).
const GH_LLAMA_BUILD: &str = "b9966";
const GH_LLAMA: &str =
    "https://github.com/ggml-org/llama.cpp/releases/download/b9966/llama-b9966-bin-win-cuda-13.3-x64.zip";
// GitHub: onnxruntime 1.24.2 win-x64 (microsoft/onnxruntime) — строго 1.24.2 (rc.12 ABI; иначе дедлок).
const GH_ORT: &str =
    "https://github.com/microsoft/onnxruntime/releases/download/v1.24.2/onnxruntime-win-x64-1.24.2.zip";
// GitHub: onnxruntime 1.24.2 GPU-сборка под CUDA 13 (gpu_cuda13) — CUDA-EP для Parakeet/Sortformer на
// GPU. Вариант cuda13 переиспользует наши _13-DLL (cudart/cublas), нужен только cuDNN 9 (WHEEL_CUDNN).
// Содержит onnxruntime.dll(GPU) + onnxruntime_providers_cuda.dll + onnxruntime_providers_shared.dll.
const GH_ORT_GPU: &str =
    "https://github.com/microsoft/onnxruntime/releases/download/v1.24.2/onnxruntime-win-x64-gpu_cuda13-1.24.2.zip";
// GitHub: ffmpeg static win64 GPL (BtbN/FFmpeg-Builds) — тот же источник, что install.bat.
const GH_FFMPEG: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
// PyPI-wheel'ы NVIDIA CUDA 13 runtime (те же, что Higgs envdeps.rs — байт-в-байт с локальным CUDA 13.3).
const WHEEL_CUDART: &str = "https://files.pythonhosted.org/packages/d2/27/b53a5e0397842a5c11f0e1a39d4e5b2f22638a4126e83b3c4e196f62c969/nvidia_cuda_runtime-13.3.29-py3-none-win_amd64.whl";
const WHEEL_CUBLAS: &str = "https://files.pythonhosted.org/packages/08/8f/890a96ea1ff615100296977cce23296052dcb8c114d4e451201ec39df9bf/nvidia_cublas-13.6.0.2-py3-none-win_amd64.whl";
// PyPI: cuDNN 9 под CUDA 13 (nvidia-cudnn-cu13) — нужен для CUDA-EP onnxruntime (Parakeet/Sortformer на
// GPU). Даёт cudnn64_9.dll + split-либы. ≈389 МБ. cudart/cublas _13 уже есть (cuda-runtime выше).
const WHEEL_CUDNN: &str = "https://files.pythonhosted.org/packages/18/d4/c09b11336981836c3183f28a6ca309e08ad080311edb6ff6c28cecdb5f24/nvidia_cudnn_cu13-9.25.0.15-py3-none-win_amd64.whl";
// NVIDIA CUDA-13 redist (официальный): cuFFT 12.0.0.61 → даёт cufft64_12.dll. onnxruntime_providers_cuda.dll
// (сборка cuda13) импортит именно cufft64_12.dll — без него CUDA-EP не грузится («CUDA not enabled»). cudart/
// cublas _13 у нас уже есть (cuda-runtime), cuFFT сохраняет soname 12 даже в CUDA 13. Извлекается как *.dll плоско.
const REDIST_CUFFT: &str = "https://developer.download.nvidia.com/compute/cuda/redist/libcufft/windows-x86_64/libcufft-windows-x86_64-12.0.0.61-archive.zip";
// CUDA-либы для whisper-faster r192.3 (CTranslate2, собран под CUDA 11!): нужны РЯДОМ с exe
// (cublas64_11 + cudnn64_8), иначе GPU-режим Whisper падает «cublas64_11.dll not found» -> откат на CPU.
// ВАЖНО: не-XXL сборка = CUDA 11 (cublas64_11), XXL = CUDA 12 (cublas64_12) — РАЗНЫЕ. Версии те, что
// валидировал Purfview (cuBLAS 11.11.3.6 + cuDNN 8.9.7.29), но zip'ы у NVIDIA (WheelDlls тянет *.dll плоско).
const REDIST_WHISPER_CUBLAS: &str = "https://developer.download.nvidia.com/compute/cuda/redist/libcublas/windows-x86_64/libcublas-windows-x86_64-11.11.3.6-archive.zip";
const REDIST_WHISPER_CUDNN: &str = "https://developer.download.nvidia.com/compute/cudnn/redist/cudnn/windows-x86_64/cudnn-windows-x86_64-8.9.7.29_cuda11-archive.zip";
// Страница драйверов NVIDIA (кнопка «Открыть сайт» — драйвер DLL-кой не ставится).
pub const NVIDIA_DRIVER_URL: &str = "https://www.nvidia.com/Download/index.aspx";

// ═══════════════════════════════════════════════════════════════════════════
//  МАНИФЕСТ
// ═══════════════════════════════════════════════════════════════════════════

/// Полный список компонентов. Порядок = порядок показа в панели «Первый запуск».
pub fn manifest() -> Vec<Component> {
    vec![
        // ── МОДЕЛИ ──────────────────────────────────────────────────────────
        Component {
            id: "higgs",
            name: "Higgs Audio v3 (Q8_0)",
            purpose: "Синтез дубляжа и клон голоса (TTS)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 5_534_363_733,
            files: &[
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/q8_0.gguf", dest_rel: "models/higgs-q8_0/q8_0.gguf", size: 5_519_235_296, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/config.json", dest_rel: "models/higgs-q8_0/config.json", size: 2_755, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/chat_template.jinja", dest_rel: "models/higgs-q8_0/chat_template.jinja", size: 2_427, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/tokenizer.json", dest_rel: "models/higgs-q8_0/tokenizer.json", size: 11_433_924, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/tokenizer_config.json", dest_rel: "models/higgs-q8_0/tokenizer_config.json", size: 1_937, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q8_0/higgs_audio_v2_tokenizer_config.json", dest_rel: "models/higgs-q8_0/higgs_audio_v2_tokenizer_config.json", size: 2_251, extract: Extract::None },
            ],
            markers: &[
                Marker { rel: "models/higgs-q8_0/q8_0.gguf", expect: 5_519_235_296 },
                Marker { rel: "models/higgs-q8_0/config.json", expect: 2_755 },
                Marker { rel: "models/higgs-q8_0/tokenizer.json", expect: 11_433_924 },
            ],
            external_url: None,
        },
        Component {
            id: "higgs-engine",
            name: "Higgs движок (audiocpp_engine.dll)",
            purpose: "Нативный TTS-движок Higgs (C-ABI)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 71_727_104,
            files: &[
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/engines/audiocpp_engine.dll", dest_rel: "models/higgs-engine/audiocpp_engine.dll", size: 71_727_104, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/higgs-engine/audiocpp_engine.dll", expect: 71_727_104 }],
            external_url: None,
        },
        Component {
            id: "gemma",
            name: "Gemma-4 12B QAT q4_0 + vision",
            purpose: "Перевод и vision-оркестратор субтитров/титров",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 7_150_992_992,
            files: &[
                FileSpec { url: "https://huggingface.co/google/gemma-4-12b-it-qat-q4_0-gguf/resolve/main/gemma-4-12b-it-qat-q4_0.gguf", dest_rel: "models/mt/gemma-4-12b-it-qat-q4_0.gguf", size: 6_975_877_728, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/google/gemma-4-12b-it-qat-q4_0-gguf/resolve/main/mmproj-gemma-4-12b-it-qat-q4_0.gguf", dest_rel: "models/mt/mmproj-gemma-4-12b-it-qat-q4_0.gguf", size: 175_115_264, extract: Extract::None },
            ],
            markers: &[
                Marker { rel: "models/mt/gemma-4-12b-it-qat-q4_0.gguf", expect: 6_975_877_728 },
                Marker { rel: "models/mt/mmproj-gemma-4-12b-it-qat-q4_0.gguf", expect: 175_115_264 },
            ],
            external_url: None,
        },
        // Альтернативные кванты Gemma (выбор в настройках; тяжелее q4_0, чуть точнее). Свой mmproj на квант.
        Component {
            id: "gemma-q5_0",
            name: "Gemma-4 12B (Q5_K_M) + vision",
            purpose: "Перевод и vision — точнее q4_0",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 8_588_690_400,
            files: &[
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/gemma-4-12b-it-Q5_K_M.gguf", dest_rel: "models/mt-q5_0/gemma-4-12b-it-Q5_K_M.gguf", size: 8_413_574_560, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/mmproj-F16.gguf", dest_rel: "models/mt-q5_0/mmproj-F16.gguf", size: 175_115_840, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/mt-q5_0/gemma-4-12b-it-Q5_K_M.gguf", expect: 8_413_574_560 }, Marker { rel: "models/mt-q5_0/mmproj-F16.gguf", expect: 175_115_840 }],
            external_url: None,
        },
        Component {
            id: "gemma-q6_k",
            name: "Gemma-4 12B (Q6_K) + vision",
            purpose: "Перевод и vision — ещё точнее",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 9_961_137_120,
            files: &[
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/gemma-4-12b-it-Q6_K.gguf", dest_rel: "models/mt-q6_k/gemma-4-12b-it-Q6_K.gguf", size: 9_786_021_280, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/mmproj-F16.gguf", dest_rel: "models/mt-q6_k/mmproj-F16.gguf", size: 175_115_840, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/mt-q6_k/gemma-4-12b-it-Q6_K.gguf", expect: 9_786_021_280 }, Marker { rel: "models/mt-q6_k/mmproj-F16.gguf", expect: 175_115_840 }],
            external_url: None,
        },
        Component {
            id: "gemma-q8_0",
            name: "Gemma-4 12B (Q8_0) + vision",
            purpose: "Перевод и vision — максимальная точность",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 12_844_762_080,
            files: &[
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/gemma-4-12b-it-Q8_0.gguf", dest_rel: "models/mt-q8_0/gemma-4-12b-it-Q8_0.gguf", size: 12_669_646_240, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/main/mmproj-F16.gguf", dest_rel: "models/mt-q8_0/mmproj-F16.gguf", size: 175_115_840, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/mt-q8_0/gemma-4-12b-it-Q8_0.gguf", expect: 12_669_646_240 }, Marker { rel: "models/mt-q8_0/mmproj-F16.gguf", expect: 175_115_840 }],
            external_url: None,
        },
        Component {
            id: "parakeet",
            name: "Parakeet-TDT 0.6B v3 (int8)",
            purpose: "Распознавание речи со словными таймстемпами (ASR)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 688_819_567,
            files: &[
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx", dest_rel: "models/tdt/encoder-model.int8.onnx", size: 652_183_999, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.int8.onnx", dest_rel: "models/tdt/decoder_joint-model.int8.onnx", size: 18_202_004, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/nemo128.onnx", dest_rel: "models/tdt/nemo128.onnx", size: 139_764, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt", dest_rel: "models/tdt/vocab.txt", size: 93_939, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/config.json", dest_rel: "models/tdt/config.json", size: 97, extract: Extract::None },
            ],
            markers: &[
                Marker { rel: "models/tdt/encoder-model.int8.onnx", expect: 652_183_999 },
                Marker { rel: "models/tdt/decoder_joint-model.int8.onnx", expect: 18_202_004 },
                Marker { rel: "models/tdt/vocab.txt", expect: 93_939 },
            ],
            external_url: None,
        },
        // Альтернативные кванты TTS Higgs (выбор в настройках; своя папка на квант, aux те же).
        Component {
            id: "higgs-q6_k",
            name: "Higgs Audio v3 (Q6_K)",
            purpose: "Синтез дубляжа и клон голоса (TTS) — вариант полегче Q8_0",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 5_035_000_000,
            files: &[
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/q6_k.gguf", dest_rel: "models/higgs-q6_k/q6_k.gguf", size: 5_023_637_248, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/config.json", dest_rel: "models/higgs-q6_k/config.json", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/chat_template.jinja", dest_rel: "models/higgs-q6_k/chat_template.jinja", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/tokenizer.json", dest_rel: "models/higgs-q6_k/tokenizer.json", size: 11_433_924, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/tokenizer_config.json", dest_rel: "models/higgs-q6_k/tokenizer_config.json", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q6_k/higgs_audio_v2_tokenizer_config.json", dest_rel: "models/higgs-q6_k/higgs_audio_v2_tokenizer_config.json", size: 0, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/higgs-q6_k/q6_k.gguf", expect: 5_023_637_248 }, Marker { rel: "models/higgs-q6_k/tokenizer.json", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "higgs-q4_k_m",
            name: "Higgs Audio v3 (Q4_K_M)",
            purpose: "Синтез дубляжа и клон голоса (TTS) — самый лёгкий вариант",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 4_098_000_000,
            files: &[
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/q4_k_m.gguf", dest_rel: "models/higgs-q4_k_m/q4_k_m.gguf", size: 4_086_922_976, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/config.json", dest_rel: "models/higgs-q4_k_m/config.json", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/chat_template.jinja", dest_rel: "models/higgs-q4_k_m/chat_template.jinja", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/tokenizer.json", dest_rel: "models/higgs-q4_k_m/tokenizer.json", size: 11_433_924, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/tokenizer_config.json", dest_rel: "models/higgs-q4_k_m/tokenizer_config.json", size: 0, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/drbaph/Higgs-Audio-v3-Studio/resolve/main/models/higgs-q4_k_m/higgs_audio_v2_tokenizer_config.json", dest_rel: "models/higgs-q4_k_m/higgs_audio_v2_tokenizer_config.json", size: 0, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/higgs-q4_k_m/q4_k_m.gguf", expect: 4_086_922_976 }, Marker { rel: "models/higgs-q4_k_m/tokenizer.json", expect: 0 }],
            external_url: None,
        },
        // Альтернативный квант ASR: fp32 (точнее, тяжелее int8). Отдельная папка (fp32 приоритетнее int8).
        Component {
            id: "parakeet-fp32",
            name: "Parakeet-TDT 0.6B v3 (fp32)",
            purpose: "Распознавание речи (ASR) — полная точность fp32",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 2_560_000_000,
            files: &[
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx", dest_rel: "models/tdt-fp32/encoder-model.onnx", size: 41_770_866, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.onnx.data", dest_rel: "models/tdt-fp32/encoder-model.onnx.data", size: 2_435_420_160, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.onnx", dest_rel: "models/tdt-fp32/decoder_joint-model.onnx", size: 72_520_893, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/nemo128.onnx", dest_rel: "models/tdt-fp32/nemo128.onnx", size: 139_764, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt", dest_rel: "models/tdt-fp32/vocab.txt", size: 93_939, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/config.json", dest_rel: "models/tdt-fp32/config.json", size: 0, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/tdt-fp32/encoder-model.onnx", expect: 41_770_866 }, Marker { rel: "models/tdt-fp32/encoder-model.onnx.data", expect: 2_435_420_160 }, Marker { rel: "models/tdt-fp32/vocab.txt", expect: 93_939 }],
            external_url: None,
        },
        // ── АЛЬТЕРНАТИВНЫЙ ASR-ДВИЖОК: Whisper (Purfview standalone faster-whisper) ──────────
        // Бинарь-onefile (CTranslate2 CPU из коробки; GPU опц. с CUDA11-либами). Выбор в настройках:
        // движок Parakeet/Whisper + РАЗНЫЕ модели (tiny…large-v3-turbo) + РАЗНЫЕ кванты (compute_type).
        Component {
            id: "whisper-engine",
            name: "Whisper-Faster (движок ASR)",
            purpose: "Альтернативный движок распознавания речи (faster-whisper) вместо Parakeet",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 87_654_143,
            files: &[
                FileSpec { url: "https://github.com/Purfview/whisper-standalone-win/releases/download/faster-whisper/Whisper-Faster_r192.3_windows.zip", dest_rel: "tools/whisper/_whisper.zip", size: 0, extract: Extract::ZipFlat },
            ],
            markers: &[Marker { rel: "tools/whisper/whisper-faster.exe", expect: 0 }],
            external_url: None,
        },
        // CUDA-либы для GPU-режима Whisper (cuBLAS 12 + cuDNN 8) РЯДОМ с whisper-faster.exe. Без них
        // whisper-faster (CTranslate2) не видит CUDA и падает на CPU — «GPU выбран, а работает на CPU».
        // Качается по запросу, когда стартует Whisper-джоба на GPU (см. ensure_job_components).
        Component {
            id: "whisper-cuda",
            name: "CUDA-ускорение Whisper (cuBLAS + cuDNN)",
            purpose: "GPU-инференс Whisper (иначе распознавание идёт на CPU, в разы медленнее)",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 1_125_090_089,
            files: &[
                FileSpec { url: REDIST_WHISPER_CUBLAS, dest_rel: "tools/whisper/_wcublas.zip", size: 0, extract: Extract::WheelDlls },
                FileSpec { url: REDIST_WHISPER_CUDNN, dest_rel: "tools/whisper/_wcudnn.zip", size: 0, extract: Extract::WheelDlls },
            ],
            markers: &[
                Marker { rel: "tools/whisper/cublas64_11.dll", expect: 0 },
                Marker { rel: "tools/whisper/cudnn64_8.dll", expect: 0 },
            ],
            external_url: None,
        },
        Component {
            id: "whisper-xxl",
            name: "Whisper-Faster XXL (Purfview)",
            purpose: "Умная батч-нарезка для тяжелых моделей (VAD)",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 1_428_059_000,
            files: &[
                FileSpec { 
                    url: "https://github.com/Purfview/whisper-standalone-win/releases/download/Faster-Whisper-XXL/Faster-Whisper-XXL_r245.4_windows.7z", 
                    dest_rel: "tools/Faster-Whisper-XXL_r245.4_windows.7z", 
                    size: 0, 
                    extract: Extract::SevenZipTree
                },
            ],
            markers: &[Marker { rel: "tools/Faster-Whisper-XXL/faster-whisper-xxl.exe", expect: 0 }],
            external_url: Some("https://github.com/Purfview/whisper-standalone-win/releases"),
        },
        Component {
            id: "whisper-tiny",
            name: "Whisper tiny (модель ASR)",
            purpose: "ASR Whisper — самая лёгкая и быстрая модель",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 78_203_619,
            files: &[
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-tiny/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-tiny/model.bin", size: 75_538_270, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-tiny/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-tiny/config.json", size: 2_249, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-tiny/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-tiny/tokenizer.json", size: 2_203_239, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-tiny/resolve/main/vocabulary.txt", dest_rel: "models/whisper/faster-whisper-tiny/vocabulary.txt", size: 459_861, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-tiny/model.bin", expect: 75_538_270 }, Marker { rel: "models/whisper/faster-whisper-tiny/tokenizer.json", expect: 2_203_239 }],
            external_url: None,
        },
        Component {
            id: "whisper-base",
            name: "Whisper base (модель ASR)",
            purpose: "ASR Whisper — лёгкая модель, точнее tiny",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 147_882_941,
            files: &[
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-base/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-base/model.bin", size: 145_217_532, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-base/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-base/config.json", size: 2_309, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-base/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-base/tokenizer.json", size: 2_203_239, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-base/resolve/main/vocabulary.txt", dest_rel: "models/whisper/faster-whisper-base/vocabulary.txt", size: 459_861, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-base/model.bin", expect: 145_217_532 }, Marker { rel: "models/whisper/faster-whisper-base/tokenizer.json", expect: 2_203_239 }],
            external_url: None,
        },
        Component {
            id: "whisper-small",
            name: "Whisper small (модель ASR)",
            purpose: "ASR Whisper — сбалансированная модель",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 486_212_372,
            files: &[
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-small/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-small/model.bin", size: 483_546_902, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-small/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-small/config.json", size: 2_370, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-small/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-small/tokenizer.json", size: 2_203_239, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-small/resolve/main/vocabulary.txt", dest_rel: "models/whisper/faster-whisper-small/vocabulary.txt", size: 459_861, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-small/model.bin", expect: 483_546_902 }, Marker { rel: "models/whisper/faster-whisper-small/tokenizer.json", expect: 2_203_239 }],
            external_url: None,
        },
        Component {
            id: "whisper-medium",
            name: "Whisper medium (модель ASR)",
            purpose: "ASR Whisper — высокая точность",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 1_530_571_735,
            files: &[
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-medium/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-medium/model.bin", size: 1_527_906_378, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-medium/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-medium/config.json", size: 2_257, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-medium/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-medium/tokenizer.json", size: 2_203_239, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-medium/resolve/main/vocabulary.txt", dest_rel: "models/whisper/faster-whisper-medium/vocabulary.txt", size: 459_861, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-medium/model.bin", expect: 1_527_906_378 }, Marker { rel: "models/whisper/faster-whisper-medium/tokenizer.json", expect: 2_203_239 }],
            external_url: None,
        },
        Component {
            id: "whisper-large-v3",
            name: "Whisper large-v3 (модель ASR)",
            purpose: "ASR Whisper — максимальная точность (large-v3)",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 3_090_835_702,
            files: &[
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-large-v3/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-large-v3/model.bin", size: 3_087_284_237, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-large-v3/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-large-v3/config.json", size: 2_394, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-large-v3/resolve/main/preprocessor_config.json", dest_rel: "models/whisper/faster-whisper-large-v3/preprocessor_config.json", size: 340, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-large-v3/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-large-v3/tokenizer.json", size: 2_480_617, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Systran/faster-whisper-large-v3/resolve/main/vocabulary.json", dest_rel: "models/whisper/faster-whisper-large-v3/vocabulary.json", size: 1_068_114, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-large-v3/model.bin", expect: 3_087_284_237 }, Marker { rel: "models/whisper/faster-whisper-large-v3/tokenizer.json", expect: 2_480_617 }],
            external_url: None,
        },
        Component {
            id: "whisper-large-v3-turbo",
            name: "Whisper large-v3-turbo (модель ASR)",
            purpose: "ASR Whisper — почти large-v3, но заметно быстрее (turbo)",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 1_621_665_983,
            files: &[
                FileSpec { url: "https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/resolve/main/model.bin", dest_rel: "models/whisper/faster-whisper-large-v3-turbo/model.bin", size: 1_617_884_929, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/resolve/main/config.json", dest_rel: "models/whisper/faster-whisper-large-v3-turbo/config.json", size: 2_263, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/resolve/main/preprocessor_config.json", dest_rel: "models/whisper/faster-whisper-large-v3-turbo/preprocessor_config.json", size: 340, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/resolve/main/tokenizer.json", dest_rel: "models/whisper/faster-whisper-large-v3-turbo/tokenizer.json", size: 2_710_337, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/resolve/main/vocabulary.json", dest_rel: "models/whisper/faster-whisper-large-v3-turbo/vocabulary.json", size: 1_068_114, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/whisper/faster-whisper-large-v3-turbo/model.bin", expect: 1_617_884_929 }, Marker { rel: "models/whisper/faster-whisper-large-v3-turbo/tokenizer.json", expect: 2_710_337 }],
            external_url: None,
        },
        Component {
            id: "sortformer",
            name: "Sortformer v2 (диаризация)",
            purpose: "Разделение спикеров (кто когда говорит)",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 492_243_002,
            files: &[
                FileSpec { url: HF_SORTFORMER, dest_rel: "models/sortformer/diar_streaming_sortformer_4spk-v2.onnx", size: 492_243_002, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/sortformer/diar_streaming_sortformer_4spk-v2.onnx", expect: 492_243_002 }],
            external_url: None,
        },
        Component {
            id: "roformer",
            name: "Mel-Band Roformer voc_fv6 (Q8_0)",
            purpose: "Модель вокал/инструментал сепарации",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 251_707_744,
            files: &[
                FileSpec { url: HF_ROFORMER, dest_rel: "models/bsroformer/voc_fv6-Q8_0.gguf", size: 251_707_744, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/bsroformer/voc_fv6-Q8_0.gguf", expect: 251_707_744 }],
            external_url: None,
        },
        // Альтернативные кванты сепарации (выбор в настройках; лёгкие, качество чуть ниже Q8_0).
        Component {
            id: "roformer-q5",
            name: "Mel-Band Roformer voc_fv6 (Q5_0)",
            purpose: "Сепарация — вариант полегче Q8_0",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 167_303_008,
            files: &[
                FileSpec { url: "https://huggingface.co/chenmozhijin/BSRoformer-GGUF/resolve/main/GaboxR67/MelBandRoformers/melbandroformers/vocals/voc_fv6-Q5_0.gguf", dest_rel: "models/bsroformer/voc_fv6-Q5_0.gguf", size: 167_303_008, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/bsroformer/voc_fv6-Q5_0.gguf", expect: 167_303_008 }],
            external_url: None,
        },
        Component {
            id: "roformer-q4",
            name: "Mel-Band Roformer voc_fv6 (Q4_0)",
            purpose: "Сепарация — самый лёгкий вариант",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 139_168_096,
            files: &[
                FileSpec { url: "https://huggingface.co/chenmozhijin/BSRoformer-GGUF/resolve/main/GaboxR67/MelBandRoformers/melbandroformers/vocals/voc_fv6-Q4_0.gguf", dest_rel: "models/bsroformer/voc_fv6-Q4_0.gguf", size: 139_168_096, extract: Extract::None },
            ],
            markers: &[Marker { rel: "models/bsroformer/voc_fv6-Q4_0.gguf", expect: 139_168_096 }],
            external_url: None,
        },
        // ── КАСТИНГ ПЕРСОНАЖЕЙ (#115): лица + голос ──────────────────────────
        // Полный набор моделей кастинга. Резолвятся dub_faces (<models>/faces/…). ВСЕ из первоисточников
        // (сверено по точному размеру файла). Без них кастинг мёртв: «лиц привязано 0», все спикеры SPK0.
        //   • det_10g (SCRFD-10GF) — детектор реальных лиц (insightface buffalo_l → immich-app/buffalo_l).
        //   • LVFace-L_Glint360K — эмбеддер реального лица (bytedance-research/LVFace).
        //   • ccip model_feat — эмбеддер нарисованного персонажа (deepghs/ccip_onnx, caformer-24).
        //   • anime_face model — детектор аниме/мульт-лиц (deepghs/anime_face_detection v1.4_s).
        //   • xseg_1 — окклюдер лица для чистого аватар-кропа (facefusion/models-3.1.0).
        //   • WeSpeaker ResNet34-LM — голосовой эмбеддинг: cross-episode матч голосов (на Xet-CAS).
        Component {
            id: "casting",
            name: "Модели кастинга персонажей (лица + голос)",
            purpose: "Детект/эмбеддинг лиц (реальные + аниме) + голосовой эмбеддинг для кастинга #115",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 1_331_548_084,
            files: &[
                FileSpec { url: "https://huggingface.co/immich-app/buffalo_l/resolve/main/detection/model.onnx", dest_rel: "models/faces/det_10g.onnx", size: 16_923_827, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/bytedance-research/LVFace/resolve/main/LVFace-L_Glint360K/LVFace-L_Glint360K.onnx", dest_rel: "models/faces/LVFace-L_Glint360K.onnx", size: 1_022_938_188, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepghs/ccip_onnx/resolve/main/ccip-caformer-24-randaug-pruned/model_feat.onnx", dest_rel: "models/faces/ccip/model_feat.onnx", size: 150_248_245, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/deepghs/anime_face_detection/resolve/main/face_detect_v1.4_s/model.onnx", dest_rel: "models/faces/anime_face/model.onnx", size: 44_583_229, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/facefusion/models-3.1.0/resolve/main/xseg_1.onnx", dest_rel: "models/faces/occluder/xseg_1.onnx", size: 70_324_286, extract: Extract::None },
                FileSpec { url: "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/main/voxceleb_resnet34_LM.onnx", dest_rel: "models/faces/wespeaker/voxceleb_resnet34_LM.onnx", size: 26_530_309, extract: Extract::None },
            ],
            markers: &[
                Marker { rel: "models/faces/det_10g.onnx", expect: 16_923_827 },
                Marker { rel: "models/faces/LVFace-L_Glint360K.onnx", expect: 1_022_938_188 },
                Marker { rel: "models/faces/ccip/model_feat.onnx", expect: 150_248_245 },
                Marker { rel: "models/faces/anime_face/model.onnx", expect: 44_583_229 },
                Marker { rel: "models/faces/occluder/xseg_1.onnx", expect: 70_324_286 },
                Marker { rel: "models/faces/wespeaker/voxceleb_resnet34_LM.onnx", expect: 26_530_309 },
            ],
            external_url: None,
        },
        // ── СAЙДКАРЫ / ДВИЖКИ ───────────────────────────────────────────────
        Component {
            id: "bsroformer-engine",
            name: "BSRoformer.cpp движок (CUDA)",
            purpose: "Нативный движок сепарации (bs_roformer-cli + ggml-CUDA)",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 164_990_561,
            files: &[
                FileSpec { url: GH_BSROFORMER_ENGINE, dest_rel: "tools/bsroformer/_engine.zip", size: 164_990_561, extract: Extract::ZipFlat },
            ],
            markers: &[Marker { rel: "tools/bsroformer/bs_roformer-cli.exe", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "bsroformer-engine-cpu",
            name: "BSRoformer.cpp движок (CPU)",
            purpose: "Сепарация на процессоре — режим без NVIDIA (медленнее, полная функция)",
            requirement: Requirement::Optional,
            delivery: Delivery::Download,
            size: 671_031,
            files: &[
                FileSpec { url: GH_BSROFORMER_ENGINE_CPU, dest_rel: "tools/bsroformer-cpu/_engine.zip", size: 671_031, extract: Extract::ZipFlat },
            ],
            markers: &[Marker { rel: "tools/bsroformer-cpu/bs_roformer-cli.exe", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "llama",
            name: "llama.cpp сервер (CUDA 13.3)",
            purpose: "Сайдкар-сервер для Gemma (перевод/vision)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            // Размер сжатого zip (для прогресса закачки); распакованный footprint ~683 МБ.
            size: 162_331_298,
            files: &[
                FileSpec { url: GH_LLAMA, dest_rel: "tools/llama/_llama.zip", size: 0, extract: Extract::ZipFlat },
            ],
            markers: &[Marker { rel: "tools/llama/llama-server.exe", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "onnxruntime",
            name: "ONNX Runtime 1.24.2",
            purpose: "Рантайм ASR/OCR/диаризации (строго 1.24.2)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 74_075_355,
            files: &[
                FileSpec { url: GH_ORT, dest_rel: "models/runtime/_ort.zip", size: 74_075_355, extract: Extract::ZipTree },
            ],
            // dub-asr::ensure_ort_dylib ищет ровно этот путь под models/runtime.
            markers: &[Marker { rel: "models/runtime/onnxruntime-win-x64-1.24.2/lib/onnxruntime.dll", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "onnxruntime-gpu",
            name: "ONNX Runtime 1.24.2 GPU (CUDA)",
            purpose: "CUDA-провайдер для диаризации/Parakeet на GPU (режим local_backend=gpu)",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 288_348_147,
            files: &[
                FileSpec { url: GH_ORT_GPU, dest_rel: "models/runtime/_ort_gpu.zip", size: 288_348_147, extract: Extract::ZipTree },
            ],
            markers: &[Marker { rel: "models/runtime/onnxruntime-win-x64-gpu-1.24.2/lib/onnxruntime.dll", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "ffmpeg",
            name: "FFmpeg (static win64)",
            purpose: "Декод/энкод видео и аудио (NVENC)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 168_601_393,
            files: &[
                FileSpec { url: GH_FFMPEG, dest_rel: "tools/ffmpeg/_ffmpeg.zip", size: 0, extract: Extract::ZipPick },
            ],
            markers: &[Marker { rel: "tools/ffmpeg/ffmpeg.exe", expect: 0 }],
            external_url: None,
        },
        // ── СИСТЕМНОЕ ────────────────────────────────────────────────────────
        Component {
            id: "cuda-runtime",
            name: "CUDA 13 runtime (cudart + cuBLAS + cuFFT)",
            purpose: "Редистрибутивные CUDA-DLL для движков и CUDA-EP onnxruntime (без CUDA Toolkit)",
            requirement: Requirement::Required,
            delivery: Delivery::Download,
            size: 628_433_797,
            files: &[
                FileSpec { url: WHEEL_CUDART, dest_rel: "models/higgs-engine/_cudart.whl", size: 0, extract: Extract::WheelDlls },
                FileSpec { url: WHEEL_CUBLAS, dest_rel: "models/higgs-engine/_cublas.whl", size: 0, extract: Extract::WheelDlls },
                // cuFFT (cufft64_12.dll) — обязателен для CUDA-EP onnxruntime (диаризация/Parakeet на GPU).
                FileSpec { url: REDIST_CUFFT, dest_rel: "models/higgs-engine/_cufft.zip", size: 0, extract: Extract::WheelDlls },
            ],
            markers: &[
                Marker { rel: "models/higgs-engine/cudart64_13.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/cublas64_13.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/cublasLt64_13.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/cufft64_12.dll", expect: 0 },
            ],
            external_url: None,
        },
        Component {
            id: "cudnn",
            name: "cuDNN 9 (CUDA 13)",
            purpose: "Нужен CUDA-провайдеру onnxruntime для диаризации/Parakeet на GPU",
            requirement: Requirement::Recommended,
            delivery: Delivery::Download,
            size: 408_000_000,
            files: &[
                FileSpec { url: WHEEL_CUDNN, dest_rel: "models/higgs-engine/_cudnn.whl", size: 0, extract: Extract::WheelDlls },
            ],
            markers: &[Marker { rel: "models/higgs-engine/cudnn64_9.dll", expect: 0 }],
            external_url: None,
        },
        Component {
            id: "vcruntime",
            name: "Visual C++ Runtime (2015–2022)",
            purpose: "Системные DLL движков (идут в комплекте)",
            requirement: Requirement::Required,
            delivery: Delivery::Bundled,
            size: 1_084_896,
            files: &[],
            markers: &[
                Marker { rel: "models/higgs-engine/MSVCP140.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/VCRUNTIME140.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/VCRUNTIME140_1.dll", expect: 0 },
                Marker { rel: "models/higgs-engine/VCOMP140.DLL", expect: 0 },
            ],
            external_url: None,
        },
        Component {
            id: "ocr",
            name: "OCR-модели (PP-OCR ONNX)",
            purpose: "Детекция вшитого текста → блюр (идут в комплекте)",
            requirement: Requirement::Recommended,
            delivery: Delivery::Bundled,
            size: 31_726_193,
            files: &[],
            markers: &[
                Marker { rel: "models/ocr/det.onnx", expect: 0 },
                Marker { rel: "models/ocr/cls.onnx", expect: 0 },
                Marker { rel: "models/ocr/rec_cyrillic.onnx", expect: 0 },
                Marker { rel: "models/ocr/rec_cyrillic.dict.txt", expect: 0 },
            ],
            external_url: None,
        },
        Component {
            id: "nvidia-driver",
            name: "Драйвер NVIDIA",
            purpose: "GPU-ускорение (ставится отдельно, не приложением)",
            requirement: Requirement::Required,
            delivery: Delivery::External,
            size: 0,
            files: &[],
            markers: &[],
            external_url: Some(NVIDIA_DRIVER_URL),
        },
    ]
}

// ── Статус одного компонента ────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub requirement: Requirement,
    pub delivery: Delivery,
    /// Ожидаемый совокупный размер, байт.
    pub size: u64,
    /// Установлен ли (для External — детект драйвера; иначе — все маркеры на месте с верным размером).
    pub installed: bool,
    /// Сколько байт уже на диске (для докачки/UI прогресса частично скачанного).
    pub bytes_on_disk: u64,
    /// Список отсутствующих/битых маркеров (для диагностики).
    pub missing: Vec<String>,
    /// Для External: версия драйвера, если задетектена.
    pub detail: Option<String>,
    /// URL внешней страницы (драйвер).
    pub external_url: Option<String>,
    /// Оценка VRAM при загрузке модели, байт (0 для движков/рантаймов без весов).
    pub vram: u64,
}

/// Оценка VRAM загруженной модели по id (движки/рантаймы = 0). Грубо, для показа в UI.
fn vram_estimate(id: &str) -> u64 {
    let gb = |g: f64| (g * 1024.0 * 1024.0 * 1024.0) as u64;
    match id {
        "higgs" => gb(5.6),
        "higgs-q6_k" => gb(5.1),
        "higgs-q4_k_m" => gb(4.2),
        "gemma" => gb(8.5),
        "gemma-q5_0" => gb(9.9),
        "gemma-q6_k" => gb(11.2),
        "gemma-q8_0" => gb(14.0),
        "parakeet" => gb(1.1),
        "parakeet-fp32" => gb(2.7),
        "sortformer" => gb(0.6),
        "roformer" => gb(0.5),
        "roformer-q5" => gb(0.45),
        "roformer-q4" => gb(0.4),
        _ => 0,
    }
}

/// Существует ли маркер и «целый» ли он (размер совпадает, если expect != 0).
fn marker_ok(repo_root: &Path, m: &Marker) -> bool {
    let p = repo_root.join(m.rel);
    match std::fs::metadata(&p) {
        // Файл на месте и не оборван. Размер сверяем С ДОПУСКОМ (≥97% expect), а не точным ==: апстрим-веса
        // на HF могут слегка отличаться от зашитого expect (переезд/переупаковка) -> точное == давало ложный
        // «не установлено» → ready навсегда false → «Скачать всё» в бесконечном цикле (баг-репорт беты).
        // Частичная закачка живёт в .part и сюда не попадает (download финализирует только полный файл),
        // поэтому ≥97% ловит реальные обрывки, но терпит дрейф размера в обе стороны.
        Ok(meta) if meta.is_file() => {
            m.expect == 0 || meta.len().saturating_mul(100) >= m.expect.saturating_mul(97)
        }
        _ => false,
    }
}

fn marker_bytes(repo_root: &Path, m: &Marker) -> u64 {
    std::fs::metadata(repo_root.join(m.rel)).map(|x| x.len()).unwrap_or(0)
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = e.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// ffmpeg доступен в системном PATH? (Command::new("ffmpeg") найдёт его при рендере.)
fn ffmpeg_on_path() -> bool {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Статус компонента на диске.
pub fn component_status(repo_root: &Path, c: &Component) -> ComponentStatus {
    let (installed, detail) = if c.delivery == Delivery::External {
        // Драйвер: детект по загрузке nvcuda.dll (часть драйвера). Версию не тянем (без NVML-зависимости).
        (detect_driver(), None)
    } else if c.id == "ffmpeg" {
        // ffmpeg дублирует пайплайн через PATH: если он уже в системе (Command::new("ffmpeg") найдёт) —
        // считаем установленным и НЕ навязываем закачку. Иначе — по маркеру в tools/ffmpeg.
        let by_marker = c.markers.iter().all(|m| marker_ok(repo_root, m));
        if by_marker {
            (true, Some("tools/ffmpeg".to_string()))
        } else if ffmpeg_on_path() {
            (true, Some("PATH".to_string()))
        } else {
            (false, None)
        }
    } else {
        let all = c.markers.iter().all(|m| marker_ok(repo_root, m));
        (all, None)
    };
    let missing: Vec<String> = c
        .markers
        .iter()
        .filter(|m| !marker_ok(repo_root, m))
        .map(|m| m.rel.to_string())
        .collect();
    let bytes_on_disk: u64 = if c.id == "whisper-xxl" {
        dir_size(&repo_root.join("tools").join("Faster-Whisper-XXL"))
    } else {
        c.markers.iter().map(|m| marker_bytes(repo_root, m)).sum()
    };
    ComponentStatus {
        id: c.id.to_string(),
        name: c.name.to_string(),
        purpose: c.purpose.to_string(),
        requirement: c.requirement,
        delivery: c.delivery,
        size: c.size,
        installed,
        bytes_on_disk,
        missing,
        detail,
        external_url: c.external_url.map(|s| s.to_string()),
        vram: vram_estimate(c.id),
    }
}

// ── Детект драйвера NVIDIA ───────────────────────────────────────────────────

/// Драйвер установлен, если грузится nvcuda.dll (Windows) / libcuda (Linux) — часть драйвера, не Toolkit.
/// Лёгкий детект без NVML-зависимости: пробуем LoadLibrary. На не-Windows — наличие libcuda в загрузке.
#[cfg(windows)]
pub fn detect_driver() -> bool {
    use std::os::windows::ffi::OsStrExt;
    // LoadLibraryW("nvcuda.dll"); успех => драйвер есть. FreeLibrary опускаем (процесс короткоживущий тут).
    let wide: Vec<u16> = std::ffi::OsStr::new("nvcuda.dll")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let h = LoadLibraryW(wide.as_ptr());
        !h.is_null()
    }
}

#[cfg(windows)]
extern "system" {
    fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
}

#[cfg(not(windows))]
pub fn detect_driver() -> bool {
    // На Linux драйвер = libcuda.so.1 в загрузчике. Для портатива на Windows это ветка не активна.
    std::path::Path::new("/usr/lib/x86_64-linux-gnu/libcuda.so.1").exists()
        || std::path::Path::new("/usr/lib/libcuda.so.1").exists()
}

// ── Импорт готовых моделей из выбранной папки ────────────────────────────────

/// Рекурсивно собрать карту basename(lower) -> [(path, size)] под dir (лимит глубины/файлов, чтоб не
/// уйти в бесконечность на большом диске).
fn index_dir(dir: &Path, map: &mut std::collections::HashMap<String, Vec<(PathBuf, u64)>>, depth: usize, budget: &mut usize) {
    if depth > 8 || *budget == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *budget == 0 {
            return;
        }
        let p = e.path();
        if p.is_dir() {
            index_dir(&p, map, depth + 1, budget);
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            map.entry(name.to_lowercase()).or_default().push((p, size)); // p дальше не нужен -> move
            *budget -= 1;
        }
    }
}

/// Импортировать готовые файлы компонентов из src_dir: для каждого маркера, которого нет на месте, ищем
/// в src_dir файл с тем же именем (и размером, если известен) и КОПИРУЕМ на ожидаемый путь. Возвращает
/// список импортированных id.
pub fn import_from_dir(repo_root: &Path, src_dir: &Path, only: Option<&str>) -> Vec<String> {
    let mut map = std::collections::HashMap::new();
    let mut budget = 200_000usize;
    index_dir(src_dir, &mut map, 0, &mut budget);
    let mut imported = Vec::new();
    for c in manifest() {
        if c.delivery != Delivery::Download {
            continue;
        }
        if let Some(id) = only {
            if c.id != id {
                continue;
            }
        }
        let mut any = false;
        for m in c.markers {
            let dest = repo_root.join(m.rel);
            if marker_ok(repo_root, m) {
                continue;
            }
            let base = Path::new(m.rel).file_name().and_then(|s| s.to_str()).map(|s| s.to_lowercase());
            let Some(base) = base else { continue };
            let Some(cands) = map.get(&base) else { continue };
            // предпочитаем точное совпадение размера, иначе первый попавшийся.
            let pick = cands
                .iter()
                .find(|(_, sz)| m.expect != 0 && *sz == m.expect)
                .or_else(|| cands.first());
            if let Some((src, _)) = pick {
                if let Some(parent) = dest.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::copy(src, &dest).is_ok() {
                    any = true;
                }
            }
        }
        if any && component_status(repo_root, &c).installed {
            imported.push(c.id.to_string());
        }
    }
    imported
}

// ── Полный статус (для GET /setup/status) ────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub components: Vec<ComponentStatus>,
    /// Всё ли обязательное на месте (тогда фронт показывает обычный hero, а не «первый запуск»).
    pub ready: bool,
    /// Совокупный размер того, что ещё надо скачать (обязательное+рекомендованное, что missing и Download).
    pub download_pending: u64,
    /// Драйвер NVIDIA найден.
    pub driver_ok: bool,
    /// build-строки для диагностики (llama-билд и т.п.).
    pub llama_build: String,
}

pub fn setup_status(repo_root: &Path) -> SetupStatus {
    let mut comps: Vec<ComponentStatus> = manifest()
        .iter()
        .map(|c| component_status(repo_root, c))
        .collect();
    // Облачный пресет (OpenRouter) снимает ОБЯЗАТЕЛЬНОСТЬ тяжёлых локальных движков: если стадия перевода/
    // TTS идёт через облако (флаг + ключ), её локальную модель качать НЕ обязательно — не гейтит ready и не
    // преселектится на первом запуске (юзер выбрал облако -> не тянет ненужные гигабайты Gemma/Higgs).
    let mroot = repo_root.join("models");
    let cloud_llm = crate::models::openrouter_stage_on(&mroot, "llm");
    let cloud_tts = crate::models::openrouter_stage_on(&mroot, "tts");
    let cloud_asr = crate::models::openrouter_asr_on(&mroot);
    for c in comps.iter_mut() {
        if c.requirement != Requirement::Required {
            continue;
        }
        let is_gemma = c.id.starts_with("gemma") || c.id == "llama";
        let is_higgs = c.id.starts_with("higgs");
        let is_asr = c.id.starts_with("parakeet") || c.id.starts_with("whisper");
        if (cloud_llm && is_gemma) || (cloud_tts && is_higgs) || (cloud_asr && is_asr) {
            c.requirement = Requirement::Optional;
        }
    }
    // Backend локальных стадий: в CPU-режиме (без NVIDIA) CUDA-компоненты не нужны — сепарация идёт
    // CPU-сборкой, тяжёлое в облаке. CPU-движок сепарации становится рекомендованным (преселект на
    // первом запуске), а CUDA-движок + CUDA-рантайм — необязательными (не тянем лишние гигабайты).
    // В GPU-режиме наоборот: CPU-движок не нужен.
    let backend = crate::models::local_backend(&mroot);
    for c in comps.iter_mut() {
        if backend == "cpu" {
            if c.id == "bsroformer-engine-cpu" {
                c.requirement = Requirement::Recommended;
            } else if c.id == "bsroformer-engine" || c.id == "cuda-runtime" || c.id == "onnxruntime-gpu" || c.id == "cudnn" {
                c.requirement = Requirement::Optional;
            }
        } else if c.id == "bsroformer-engine-cpu" {
            c.requirement = Requirement::Optional;
        }
    }
    // ready = всё СКАЧИВАЕМОЕ/бандл-обязательное на месте. External (драйвер NVIDIA) НЕ гейтит: его
    // detect_driver() (LoadLibraryW nvcuda.dll) даёт ложные негативы (нет NVIDIA / DLL не в пути / CPU-бокс)
    // -> раньше первый экран ВИСЕЛ на 100%, хотя всё скачано (баг-репорт). Драйвер остаётся строкой-
    // предупреждением (driver_ok ниже), но не блокирует вход в приложение.
    let ready = comps
        .iter()
        .filter(|c| c.requirement == Requirement::Required && c.delivery != Delivery::External)
        .all(|c| c.installed);
    let download_pending = comps
        .iter()
        .filter(|c| c.delivery == Delivery::Download && !c.installed)
        .map(|c| c.size.saturating_sub(c.bytes_on_disk))
        .sum();
    let driver_ok = comps.iter().find(|c| c.id == "nvidia-driver").map(|c| c.installed).unwrap_or(false);
    SetupStatus {
        components: comps,
        ready,
        download_pending,
        driver_ok,
        llama_build: GH_LLAMA_BUILD.to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ЗАКАЧКА (тело джобы; прогресс -> колбэк, как у analyze/render)
// ═══════════════════════════════════════════════════════════════════════════

/// Колбэк прогресса скачивания: сервер оборачивает его в SSE-событие джобы.
pub type ProgressCb<'a> = dyn Fn(Value) + 'a;

/// Путь манифеста завершённых чанков рядом с загружаемым файлом (<file>.done). Хранит offset'ы готовых
/// чанков (u64 LE) -> при следующем запуске резюмируем, пропуская их (докачка больших файлов).
fn done_manifest_path(dl_target: &Path) -> PathBuf {
    let mut s = dl_target.as_os_str().to_os_string();
    s.push(".done");
    PathBuf::from(s)
}

/// Скачать набор компонентов по id (идемпотентно: уже целые файлы пропускаем). Возвращает JSON-результат
/// с итоговым статусом каждого компонента. Тело синхронное (вызывается из spawn_blocking джобы).
pub fn download_components(
    repo_root: &Path,
    ids: &[String],
    cancel: &dyn Fn() -> bool,
    progress: &ProgressCb,
) -> Result<Value, String> {
    let all = manifest();
    let selected: Vec<&Component> = all
        .iter()
        .filter(|c| ids.iter().any(|x| x == c.id) && c.delivery == Delivery::Download)
        .collect();
    if selected.is_empty() {
        return Err("нет скачиваемых компонентов среди выбранных id".to_string());
    }

    // Один агент на весь job: если включён прокси — все GET (probe + чанки) идут через него. Клонируется в
    // каждый воркер (общий пул соединений). Живая смена прокси без рестарта: агент строится из active.json тут.
    let agent = dl_agent(repo_root);

    let tmp_dir = std::env::temp_dir().join("dub-studio-setup");
    let _ = std::fs::create_dir_all(&tmp_dir);

    // Файлы к загрузке (пропускаем уже целые прямые файлы). ci — индекс компонента в selected: прогресс
    // считаем ПОКОМПОНЕНТНО, чтобы бар каждой модели заполнялся отдельно (все параллельно).
    struct Planned {
        ci: usize,
        dest: PathBuf,
        target: PathBuf,
        extract: Extract,
        url: &'static str,
    }
    let mut planned: Vec<Planned> = Vec::new();
    for (ci, c) in selected.iter().enumerate() {
        for f in c.files {
            let dest = repo_root.join(f.dest_rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("создать {}: {e}", parent.display()))?;
            }
            if f.extract == Extract::None && f.size != 0 {
                if let Ok(meta) = std::fs::metadata(&dest) {
                    if meta.len() == f.size {
                        continue; // уже на месте
                    }
                }
            }
            let target = if f.extract != Extract::None {
                tmp_dir.join(
                    Path::new(f.dest_rel)
                        .file_name()
                        .map(|s| s.to_os_string())
                        .unwrap_or_else(|| "archive.tmp".into()),
                )
            } else {
                dest.clone()
            };
            planned.push(Planned { ci, dest, target, extract: f.extract, url: f.url });
        }
    }

    // Уже всё на месте — ничего не качаем.
    if planned.is_empty() {
        let results: Vec<Value> = selected
            .iter()
            .map(|c| {
                let st = component_status(repo_root, c);
                json!({ "id": c.id, "installed": st.installed, "missing": st.missing })
            })
            .collect();
        let overall = setup_status(repo_root);
        return Ok(json!({ "components": results, "ready": overall.ready }));
    }

    progress(json!({ "msg": "Скачиваю модели…", "stage": "download" }));

    // Чанки всех файлов в ОДНУ очередь; счётчик прогресса — на КАЖДЫЙ компонент (comp_done[ci]).
    enum Task {
        // done — манифест завершённых чанков (дозапись offset при успехе) для РЕЗЮМА при следующем запуске.
        Range { file: Arc<File>, url: &'static str, start: u64, end: u64, ci: usize, done: Arc<Mutex<File>> },
        Whole { url: &'static str, dest: PathBuf, ci: usize },
    }
    let ncomp = selected.len();
    let comp_done: Vec<Arc<AtomicU64>> = (0..ncomp).map(|_| Arc::new(AtomicU64::new(0))).collect();
    let mut comp_total = vec![0u64; ncomp];
    let mut tasks: Vec<Task> = Vec::new();
    let mut open_files: Vec<Arc<File>> = Vec::new(); // держим хендлы живыми до конца пула
    // Прямые файлы (Extract::None) качаем во ВРЕМЕННЫЙ <target>.part и переименовываем в финал ТОЛЬКО после
    // валидации (размер + GGUF-магия). Иначе set_len создаёт файл полного размера сразу -> маркер по размеру
    // считает его «установленным» ещё до докачки / при обрыве -> llama-server грузит нули ('????'). (part, финал, total).
    let mut parts: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    for p in &planned {
        if cancel() {
            return Err("отменено".to_string());
        }
        let (total, ranges_ok) = probe_size(&agent, p.url);
        comp_total[p.ci] += total;
        let dl_target: PathBuf = if p.extract == Extract::None {
            let mut s = p.target.clone().into_os_string();
            s.push(".part");
            let part = PathBuf::from(s);
            parts.push((part.clone(), p.target.clone(), total));
            part
        } else {
            p.target.clone() // архивы качаем в tmp напрямую — extract их сам валидирует
        };
        if ranges_ok && total > 0 {
            // РЕЗЮМ большого файла: .part уже нужного размера И рядом манифест .done -> дочитываем ТОЛЬКО
            // недостающие чанки (обрыв Xet на 12ГБ больше НЕ заставляет качать с нуля). Иначе — свежая закачка.
            let done_path = done_manifest_path(&dl_target);
            let resuming = std::fs::metadata(&dl_target).map(|m| m.len() == total).unwrap_or(false)
                && done_path.is_file();
            let completed: std::collections::HashSet<u64> = if resuming {
                std::fs::read(&done_path)
                    .ok()
                    .map(|b| {
                        b.chunks_exact(8) // рваный хвост (<8 байт при килле) chunks_exact игнорирует
                            .filter_map(|c| <[u8; 8]>::try_from(c).ok().map(u64::from_le_bytes))
                            // только валидные границы чанков в пределах файла (защита от мусора в манифесте)
                            .filter(|off| *off < total && off % CHUNK == 0)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                let _ = std::fs::remove_file(&done_path); // свежая закачка -> старый манифест долой
                std::collections::HashSet::new()
            };
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(!resuming) // резюм -> НЕ обнуляем уже скачанное
                .open(&dl_target)
                .map_err(|e| format!("создать {}: {e}", dl_target.display()))?;
            if !resuming {
                file.set_len(total).map_err(|e| format!("set_len: {e}"))?;
            }
            let file = Arc::new(file);
            open_files.push(file.clone());
            let done = Arc::new(Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&done_path)
                    .map_err(|e| format!("манифест {}: {e}", done_path.display()))?,
            ));
            let mut start = 0u64;
            while start < total {
                let end = (start + CHUNK - 1).min(total - 1);
                if completed.contains(&start) {
                    comp_done[p.ci].fetch_add(end - start + 1, Ordering::Relaxed); // учесть в прогрессе, не качать
                } else {
                    tasks.push(Task::Range { file: file.clone(), url: p.url, start, end, ci: p.ci, done: done.clone() });
                }
                start += CHUNK;
            }
        } else {
            tasks.push(Task::Whole { url: p.url, dest: dl_target, ci: p.ci });
        }
    }
    let grand_total: u64 = comp_total.iter().sum();

    // ── Общий пул: POOL_SLOTS воркеров разбирают ОДНУ очередь чанков всех файлов. Разные модели качаются
    //    одновременно, но суммарно не больше POOL_SLOTS соединений (не 8×N -> без бана HF CDN). ──
    let n = POOL_SLOTS.min(tasks.len()).max(1);
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(tasks)));
    let finished = Arc::new(AtomicUsize::new(0));
    let abort = Arc::new(AtomicBool::new(false));
    let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    std::thread::scope(|sc| {
        for _ in 0..n {
            let (queue, comp_done, finished, abort, error, agent) = (
                queue.clone(),
                comp_done.clone(),
                finished.clone(),
                abort.clone(),
                error.clone(),
                agent.clone(),
            );
            sc.spawn(move || {
                loop {
                    if abort.load(Ordering::Relaxed) {
                        break;
                    }
                    let task = { queue.lock().unwrap().pop_front() };
                    let task = match task {
                        Some(t) => t,
                        None => break, // очередь пуста
                    };
                    let res = match &task {
                        Task::Range { file, url, start, end, ci, done } => {
                            download_range(&agent, url, file, *start, *end, &comp_done[*ci], &abort, done)
                        }
                        Task::Whole { url, dest, ci } => {
                            download_whole(&agent, url, dest, &comp_done[*ci], &abort)
                        }
                    };
                    if let Err(e) = res {
                        if e != "отменено" {
                            abort.store(true, Ordering::Relaxed);
                            let mut slot = error.lock().unwrap();
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                        }
                        break;
                    }
                }
                finished.fetch_add(1, Ordering::SeqCst);
            });
        }
        // Главный поток: агрегатный + ПОКОМПОНЕНТНЫЙ прогресс (parts) + проверка отмены.
        let t0 = std::time::Instant::now();
        loop {
            if cancel() {
                abort.store(true, Ordering::Relaxed);
            }
            let got: u64 = comp_done.iter().map(|a| a.load(Ordering::Relaxed)).sum();
            let secs = t0.elapsed().as_secs_f64();
            let mbps = if secs > 0.0 { (got as f64 / 1_000_000.0) / secs } else { 0.0 };
            let overall = if grand_total > 0 { (got as f64 / grand_total as f64) * 100.0 } else { 0.0 };
            let parts: Vec<Value> = (0..ncomp)
                .filter(|&i| comp_total[i] > 0)
                .map(|i| {
                    let d = comp_done[i].load(Ordering::Relaxed);
                    let p = (d as f64 / comp_total[i] as f64 * 100.0).min(100.0);
                    json!({ "component": selected[i].id, "pct": p })
                })
                .collect();
            progress(json!({
                "stage": "download",
                "msg": "Скачиваю модели…",
                "downloaded": got,
                "total": grand_total,
                "speed_mbps": mbps,
                "pct": overall.min(100.0),
                "parts": parts,
            }));
            if finished.load(Ordering::SeqCst) >= n {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });

    drop(open_files); // закрыть хендлы до распаковки (иначе zip не откроет файл на чтение)

    // Сбой vs отмена. При СБОЕ (обрыв сети) НЕ удаляем .part и .done-манифест — следующий запуск ДОКАЧАЕТ
    // недостающие чанки (главный фикс для больших файлов на флаки-сети). При ОТМЕНЕ пользователем — чистим
    // всё (он не хочет продолжать). Готовые финальные файлы не трогаем в любом случае.
    let cleanup = |keep_for_resume: bool| {
        if keep_for_resume {
            return; // .part + .done остаются -> докачка при повторе
        }
        for (part, _, _) in &parts {
            let _ = std::fs::remove_file(part);
            let _ = std::fs::remove_file(done_manifest_path(part));
        }
        for p in &planned {
            if p.extract != Extract::None {
                let _ = std::fs::remove_file(&p.target);
                let _ = std::fs::remove_file(done_manifest_path(&p.target));
            }
        }
    };
    if let Some(e) = error.lock().unwrap().take() {
        cleanup(true); // сохранить прогресс для докачки
        return Err(e);
    }
    if cancel() {
        cleanup(false); // отмена -> удалить незавершённое
        return Err("отменено".to_string());
    }

    // Успех: валидируем каждый .part (размер == probed total; .gguf -> магия GGUF) и АТОМАРНО переименовываем
    // в финал. Битый/неполный .part -> удаляем + ошибка; финальный файл не появляется -> маркер честно «не
    // установлен» (не даём llama-server грузить файл с дырами). Ловит и Xet-обрыв, и прерванную докачку.
    for (part, target, total) in &parts {
        let sz = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if *total > 0 && sz != *total {
            let _ = std::fs::remove_file(part);
            return Err(format!("докачка {}: неполный размер {sz}/{total}", target.display()));
        }
        if target.extension().and_then(|s| s.to_str()) == Some("gguf") {
            let mut magic = [0u8; 4]; // трейт Read импортирован на уровне модуля

            let ok = std::fs::File::open(part)
                .and_then(|mut f| f.read_exact(&mut magic))
                .is_ok();
            if !ok || &magic != b"GGUF" {
                let _ = std::fs::remove_file(part);
                return Err(format!("докачка {}: не GGUF (magic={magic:02x?}) — файл битый", target.display()));
            }
        }
        std::fs::rename(part, target)
            .map_err(|e| format!("переименовать {}: {e}", target.display()))?;
        let _ = std::fs::remove_file(done_manifest_path(part)); // файл целиком собран -> манифест не нужен
    }

    // Распаковка/финализация архивов — последовательно, вне сети.
    for p in &planned {
        let dir = p.dest.parent().unwrap_or(repo_root);
        match p.extract {
            Extract::None => {}
            Extract::ZipFlat => {
                extract_zip_flat(&p.target, dir)?;
                let _ = std::fs::remove_file(&p.target);
            }
            Extract::ZipPick => {
                extract_zip_pick(&p.target, dir)?;
                let _ = std::fs::remove_file(&p.target);
            }
            Extract::ZipTree => {
                extract_zip_tree(&p.target, dir)?;
                let _ = std::fs::remove_file(&p.target);
            }
            Extract::WheelDlls => {
                extract_wheel_dlls(&p.target, dir)?;
                let _ = std::fs::remove_file(&p.target);
            }
            Extract::SevenZipTree => {
                extract_sevenzip_tree(&p.target, dir)?;
                let _ = std::fs::remove_file(&p.target);
            }
        }
    }

    let mut results = Vec::new();
    for c in &selected {
        let st = component_status(repo_root, c);
        // Скачанный вариант модели -> делаем активным (models/active.json). Резолв при следующей
        // генерации подхватит без рестарта; иначе скан взял бы дефолт (q8_0 первым) и альт бы не применился.
        if st.installed {
            for (engine, variant) in crate::models::component_selection(c.id) {
                let _ = crate::models::set_selection(&crate::models_root(repo_root), engine, &variant);
            }
        }
        results.push(json!({ "id": c.id, "installed": st.installed, "missing": st.missing }));
    }
    let overall = setup_status(repo_root);
    Ok(json!({ "components": results, "ready": overall.ready }))
}

// Общий пул соединений на ВСЁ задание: чанки всех файлов в одной очереди, POOL_SLOTS воркеров разбирают её.
// Разные модели качаются одновременно, но суммарно не больше POOL_SLOTS коннектов (не 8×N -> без бана HF CDN).
// 4, не 16: HF Xet-CAS (cas-bridge.xethub.hf.co — туда уехали все альт-кванты) роняет соединения при
// высокой параллели, файл собирается с дырами и молча бьётся. 4 коннекта Xet держит; обычный CDN и на
// 4 сатурирует канал. Плюс ретраи диапазонов (RANGE_RETRIES) добивают транзиентные дропы.
const POOL_SLOTS: usize = 4;
const CHUNK: u64 = 16 * 1024 * 1024; // 16МБ на задачу — балансирует очередь между большими и мелкими файлами

#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    f.seek_write(buf, off)
}
#[cfg(not(windows))]
fn write_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    f.write_at(buf, off)
}

/// Построить ureq-агента для закачки: включён прокси в active.json -> ВСЕ GET идут через него; иначе дефолтный
/// агент (Config::default сам подхватит HTTP(S)_PROXY из env, если он задан на старте). Некорректный URL прокси
/// -> лог + дефолт: закачка по прямому пути честно упадёт на заблокированном соединении, а не молча пойдёт мимо
/// прокси. Один агент на весь job (общий пул соединений) -> клонируется в воркеры (Agent = cheap Clone).
fn dl_agent(repo_root: &Path) -> ureq::Agent {
    if let Some(url) = crate::models::proxy_url(&repo_root.join("models")) {
        match ureq::Proxy::new(&url) {
            Ok(proxy) => return ureq::Agent::config_builder().proxy(Some(proxy)).build().into(),
            Err(e) => tracing::warn!("некорректный URL прокси ({e}) — закачка без прокси; проверьте настройки"),
        }
    }
    ureq::Agent::new_with_defaults()
}

/// Размер файла + поддержка byte-range: 1-байтовый ranged-пробник. HF CDN (в т.ч. Xet-CAS) отдаёт 206 +
/// content-range на ureq-запрос (reqwest/curl-UA CAS душит 403). Порт Higgs probe_size.
fn probe_size(agent: &ureq::Agent, url: &str) -> (u64, bool) {
    match agent.get(url).header("Range", "bytes=0-0").call() {
        Ok(resp) => {
            if resp.status().as_u16() == 206 {
                let total = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.rsplit('/').next())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                (total, total > 0)
            } else {
                let total = resp
                    .headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                (total, false)
            }
        }
        Err(_) => (0, false),
    }
}

/// Сколько раз повторяем ОДИН диапазон при сбое. HF Xet-CAS (cas-bridge.xethub.hf.co) роняет соединения
/// под параллелью (Peer disconnected) ИЛИ отдаёт неполный range — это ТРАНЗИЕНТНО, ретрай спасает. 8 (не 6):
/// на больших файлах (Gemma Q8 ~12ГБ, 750 чанков) вероятность транзиентного дропа выше; плюс есть докачка.
const RANGE_RETRIES: u32 = 8;

/// Скачать ОДИН диапазон [start,end] в общий файл по офсету (write_at, без seek-гонок). abort -> стоп всех.
/// Порт Higgs download_range (ureq + Range) + РЕТРАИ (Xet-CAS дропает под параллелью). downloaded — общий
/// счётчик прогресса; на неудачной попытке откатываем её вклад, чтобы ретрай не задвоил прогресс.
/// Обязательна проверка полноты диапазона: 206 + РОВНО (end-start+1) байт, иначе дыра в файле = битый GGUF.
fn download_range(
    agent: &ureq::Agent,
    url: &str,
    file: &Arc<File>,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    abort: &Arc<AtomicBool>,
    done: &Arc<Mutex<File>>,
) -> Result<(), String> {
    let want = end - start + 1;
    let mut last = String::new();
    for attempt in 0..RANGE_RETRIES {
        if abort.load(Ordering::Relaxed) {
            return Err("отменено".into());
        }
        let mut got = 0u64;
        let res = download_range_once(agent, url, file, start, end, downloaded, abort, &mut got);
        match res {
            Ok(()) if got == want => {
                // WRITE-AHEAD DURABILITY: сначала fsync ДАННЫХ чанка в .part, ТОЛЬКО потом отметка в манифесте.
                // Иначе при жёстком килле/потере питания манифест мог бы опередить данные -> резюм пропустил бы
                // чанк, у которого на диске ДЫРА (нули) -> битый файл, не пойманный (size сходится по set_len,
                // GGUF-magic — только первый чанк). Порядок «данные на диск -> потом готово» гарантирует: если
                // в манифесте есть offset, его данные durable. sync_data (fdatasync) дешевле sync_all.
                let _ = file.sync_data();
                if let Ok(mut m) = done.lock() {
                    use std::io::Write;
                    let _ = m.write_all(&start.to_le_bytes());
                    let _ = m.sync_data(); // и сам манифест durable (8 байт — запись атомарна)
                }
                return Ok(());
            }
            Ok(()) => last = format!("неполный range: {got}/{want} байт"),
            Err(e) if e == "отменено" => return Err(e),
            Err(e) => last = e,
        }
        // откат прогресса этой попытки + бэкофф перед повтором (следующая попытка перезапишет диапазон)
        downloaded.fetch_sub(got.min(downloaded.load(Ordering::Relaxed)), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(400 * (attempt as u64 + 1)));
    }
    Err(format!("range {start}-{end} после {RANGE_RETRIES} попыток: {last}"))
}

/// Одна попытка скачать диапазон. Пишет got = сколько байт реально записано (для отката прогресса).
fn download_range_once(
    agent: &ureq::Agent,
    url: &str,
    file: &Arc<File>,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    abort: &Arc<AtomicBool>,
    got: &mut u64,
) -> Result<(), String> {
    let resp = agent
        .get(url)
        .header("Range", &format!("bytes={start}-{end}"))
        .call()
        .map_err(|e| format!("range {start}-{end}: {e}"))?;
    if resp.status().as_u16() != 206 {
        return Err(format!("range {start}-{end}: статус {} (ждали 206)", resp.status()));
    }
    let mut reader = resp.into_body().into_reader();
    let mut buf = [0u8; 262_144];
    let mut offset = start;
    loop {
        if abort.load(Ordering::Relaxed) {
            return Err("отменено".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("чтение range: {e}"))?;
        if n == 0 {
            break;
        }
        let mut w = 0;
        while w < n {
            let k = write_at(file, &buf[w..n], offset + w as u64).map_err(|e| format!("запись: {e}"))?;
            if k == 0 {
                return Err("short write".into());
            }
            w += k;
        }
        offset += n as u64;
        *got += n as u64;
        downloaded.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// Скачать файл ЦЕЛИКОМ в один поток (fallback: сервер без range), обновляя ОБЩИЙ счётчик пула. abort -> стоп.
fn download_whole(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    downloaded: &Arc<AtomicU64>,
    abort: &Arc<AtomicBool>,
) -> Result<(), String> {
    use std::io::Write;
    let resp = agent.get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    let mut reader = resp.into_body().into_reader();
    let mut file = File::create(dest).map_err(|e| format!("создать {}: {e}", dest.display()))?;
    let mut buf = [0u8; 262_144];
    loop {
        if abort.load(Ordering::Relaxed) {
            return Err("отменено".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("чтение: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("запись: {e}"))?;
        downloaded.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| format!("flush: {e}"))?;
    Ok(())
}

// ── Распаковка архивов ───────────────────────────────────────────────────────

/// zip: все файлы плоско (только имя) в dir. Для движков-сайдкаров (exe + DLL в одном уровне).
fn extract_zip_flat(zip_path: &Path, dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("открыть {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("не zip: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("запись zip: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let leaf = name.rsplit('/').next().unwrap_or(&name).to_string();
        if leaf.is_empty() {
            continue;
        }
        write_entry(&mut entry, &dir.join(&leaf))?;
    }
    Ok(())
}

/// zip: отобрать нужные файлы (onnxruntime.dll, ffmpeg.exe/ffprobe.exe) и положить плоско в dir.
/// onnxruntime-win-x64-1.24.2/lib/onnxruntime.dll -> dir/onnxruntime.dll ; ffmpeg .../bin/*.exe -> dir/*.exe.
fn extract_zip_pick(zip_path: &Path, dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("открыть {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("не zip: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    // Целевые: onnxruntime.dll (+.pdb не нужен), ffmpeg.exe, ffprobe.exe. Берём по имени листа.
    const WANT: &[&str] = &["onnxruntime.dll", "ffmpeg.exe", "ffprobe.exe"];
    let mut got = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("запись zip: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let leaf_raw = name.rsplit('/').next().unwrap_or(&name); // лист один раз (без повторного rsplit)
        let leaf = leaf_raw.to_ascii_lowercase();
        if WANT.iter().any(|w| *w == leaf) {
            let out_name = leaf_raw.to_string(); // исходный регистр имени выходного файла сохраняем
            write_entry(&mut entry, &dir.join(&out_name))?;
            got += 1;
        }
    }
    if got == 0 {
        return Err(format!("в архиве {} не найдено нужных файлов", zip_path.display()));
    }
    Ok(())
}

/// zip: распаковать весь архив с сохранением поддерева в dir (onnxruntime-win-x64-*/lib/…). Защита от
/// zip-slip: отбрасываем компоненты `..` и абсолютные пути.
fn extract_zip_tree(zip_path: &Path, dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("открыть {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("не zip: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("запись zip: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        // Санитизация пути: только нормальные компоненты, без `..`/корней.
        let mut rel = PathBuf::new();
        for comp in name.split('/') {
            if comp.is_empty() || comp == "." || comp == ".." {
                continue;
            }
            rel.push(comp);
        }
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dir.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("создать {}: {e}", parent.display()))?;
        }
        write_entry(&mut entry, &out)?;
    }
    Ok(())
}

/// wheel (zip): все *.dll плоско в dir (CUDA runtime — cudart/cublas/cublasLt).
fn extract_wheel_dlls(wheel_path: &Path, dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(wheel_path).map_err(|e| format!("открыть {}: {e}", wheel_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("wheel не zip: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    let mut written = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("запись wheel: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let leaf = name.rsplit('/').next().unwrap_or(&name).to_string();
        if !leaf.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        write_entry(&mut entry, &dir.join(&leaf))?;
        written += 1;
    }
    if written == 0 {
        return Err(format!("в wheel {} нет DLL", wheel_path.display()));
    }
    Ok(())
}

/// Записать элемент архива в файл через .part+rename (атомарно).
fn write_entry(entry: &mut zip::read::ZipFile<impl std::io::Read>, out: &Path) -> Result<(), String> {
    let tmp = out.with_extension(format!(
        "{}part",
        out.extension().and_then(|e| e.to_str()).map(|e| format!("{e}.")).unwrap_or_default()
    ));
    {
        let mut fout = std::fs::File::create(&tmp).map_err(|e| format!("создать {}: {e}", tmp.display()))?;
        std::io::copy(entry, &mut fout).map_err(|e| format!("распаковка {}: {e}", out.display()))?;
    }
    std::fs::rename(&tmp, out).map_err(|e| format!("финализация {}: {e}", out.display()))?;
    Ok(())
}

fn find_7z() -> Option<PathBuf> {
    // 1. Check C:\Program Files\7-Zip\7z.exe
    let p1 = PathBuf::from(r"C:\Program Files\7-Zip\7z.exe");
    if p1.is_file() {
        return Some(p1);
    }
    // 2. Check C:\Program Files (x86)\7-Zip\7z.exe
    let p2 = PathBuf::from(r"C:\Program Files (x86)\7-Zip\7z.exe");
    if p2.is_file() {
        return Some(p2);
    }
    // 3. Check in PATH
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let exe = dir.join("7z.exe");
            if exe.is_file() {
                return Some(exe);
            }
            let exe_no_ext = dir.join("7z");
            if exe_no_ext.is_file() {
                return Some(exe_no_ext);
            }
        }
    }
    None
}

fn find_winrar() -> Option<PathBuf> {
    // 1. Check C:\Program Files\WinRAR\WinRAR.exe
    let p1 = PathBuf::from(r"C:\Program Files\WinRAR\WinRAR.exe");
    if p1.is_file() {
        return Some(p1);
    }
    // 2. Check C:\Program Files (x86)\WinRAR\WinRAR.exe
    let p2 = PathBuf::from(r"C:\Program Files (x86)\WinRAR\WinRAR.exe");
    if p2.is_file() {
        return Some(p2);
    }
    // 3. Check in PATH
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let exe = dir.join("WinRAR.exe");
            if exe.is_file() {
                return Some(exe);
            }
            let exe_no_ext = dir.join("winrar");
            if exe_no_ext.is_file() {
                return Some(exe_no_ext);
            }
        }
    }
    None
}

fn run_tar(sz_path: &Path, dir: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new("tar");
    cmd.arg("-xf")
        .arg(sz_path)
        .arg("-C")
        .arg(dir);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let status = cmd.status().map_err(|e| format!("ошибка запуска tar: {e}"))?;
    if !status.success() {
        return Err(format!("tar вернул код ошибки: {}", status));
    }
    Ok(())
}

fn run_7z(sz_path: &Path, dir: &Path) -> Result<(), String> {
    let Some(exe) = find_7z() else {
        return Err("7-Zip не найден в системе".to_string());
    };

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("x")
        .arg("-y")
        .arg(sz_path)
        .arg(format!("-o{}", dir.to_string_lossy()));

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let status = cmd.status().map_err(|e| format!("ошибка запуска 7-Zip ({:?}): {e}", exe))?;
    if !status.success() {
        return Err(format!("7-Zip ({:?}) вернул код ошибки: {}", exe, status));
    }
    Ok(())
}

fn run_winrar(sz_path: &Path, dir: &Path) -> Result<(), String> {
    let Some(exe) = find_winrar() else {
        return Err("WinRAR не найден в системе".to_string());
    };

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("x")
        .arg("-y")
        .arg("-ibck")
        .arg(sz_path)
        .arg("*.*");

    let mut dest_str = dir.to_string_lossy().into_owned();
    if !dest_str.ends_with('\\') && !dest_str.ends_with('/') {
        dest_str.push('\\');
    }
    cmd.arg(&dest_str);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let status = cmd.status().map_err(|e| format!("ошибка запуска WinRAR ({:?}): {e}", exe))?;
    if !status.success() {
        return Err(format!("WinRAR ({:?}) вернул код ошибки: {}", exe, status));
    }
    Ok(())
}

/// 7z: распаковать с сохранением структуры каталогов (использует локальный 7-Zip, либо tar, либо системный 7-Zip/WinRAR)
fn extract_sevenzip_tree(sz_path: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("создание {}: {e}", dir.display()))?;

    // 1. Сначала пробуем локальный встроенный 7-Zip (7za.exe в tools/7Zip)
    let mut local_7z = dir.join("7Zip").join("7za.exe");
    if !local_7z.is_file() {
        local_7z = dir.join("tools").join("7Zip").join("7za.exe");
    }

    let local_res = if local_7z.is_file() {
        let mut cmd = std::process::Command::new(&local_7z);
        cmd.arg("x")
            .arg("-y")
            .arg(sz_path)
            .arg(format!("-o{}", dir.to_string_lossy()));

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        match cmd.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("локальный 7-Zip вернул код ошибки: {}", status)),
            Err(e) => Err(format!("ошибка запуска локального 7-Zip: {e}")),
        }
    } else {
        Err("локальный 7-Zip не найден в папке tools/7Zip".to_string())
    };

    if local_res.is_ok() {
        return Ok(());
    }
    let local_err = local_res.unwrap_err();
    tracing::warn!("{local_err}. Пробуем системный tar...");

    // 2. Пробуем системный tar
    match run_tar(sz_path, dir) {
        Ok(()) => return Ok(()),
        Err(tar_err) => {
            tracing::warn!("системный tar не справился с распаковкой 7z ({tar_err}). Пробуем системный 7-Zip...");
            
            // 3. Пробуем системный 7z
            match run_7z(sz_path, dir) {
                Ok(()) => return Ok(()),
                Err(sz_err) => {
                    tracing::warn!("системный 7-Zip не справился с распаковкой: {sz_err}. Пробуем WinRAR...");
                    
                    // 4. Пробуем WinRAR
                    match run_winrar(sz_path, dir) {
                        Ok(()) => return Ok(()),
                        Err(winrar_err) => {
                            let err_msg = format!(
                                "Не удалось автоматически распаковать архив 7z.\n\
                                Детали ошибок:\n\
                                - Локальный 7-Zip: {local_err}\n\
                                - Системный tar: {tar_err}\n\
                                - Системный 7-Zip: {sz_err}\n\
                                - WinRAR: {winrar_err}\n\n\
                                Решение проблемы:\n\
                                - Установите архиватор 7-Zip (https://www.7-zip.org/) на ваш компьютер.\n\
                                - Либо скачайте архив вручную: https://github.com/Purfview/whisper-standalone-win/releases/download/Faster-Whisper-XXL/Faster-Whisper-XXL_r245.4_windows.7z\n\
                                  и распакуйте его содержимое в папку: tools/Faster-Whisper-XXL/\n\
                                  (чтобы файл exe лежал прямо по пути: tools/Faster-Whisper-XXL/faster-whisper-xxl.exe)"
                            );
                            return Err(err_msg);
                        }
                    }
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_archivers() {
        let sz = find_7z();
        let winrar = find_winrar();
        println!("7-Zip path: {:?}", sz);
        println!("WinRAR path: {:?}", winrar);
    }

    #[test]
    fn manifest_ids_unique_and_nonempty() {
        let m = manifest();
        assert!(!m.is_empty());
        let mut ids: Vec<&str> = m.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "id компонентов должны быть уникальны");
    }

    #[test]
    fn download_components_have_files_and_markers() {
        for c in manifest() {
            match c.delivery {
                Delivery::Download => {
                    assert!(!c.files.is_empty(), "{}: Download без files", c.id);
                    assert!(!c.markers.is_empty(), "{}: Download без markers", c.id);
                    for f in c.files {
                        assert!(f.url.starts_with("https://"), "{}: не https URL", c.id);
                    }
                }
                Delivery::Bundled => {
                    assert!(c.files.is_empty(), "{}: Bundled не качается", c.id);
                    assert!(!c.markers.is_empty(), "{}: Bundled без markers", c.id);
                }
                Delivery::External => {
                    assert!(c.external_url.is_some(), "{}: External без url", c.id);
                }
            }
        }
    }

    #[test]
    fn all_installed_on_this_machine() {
        // Юнит из ТЗ (а): манифест против реального диска. На этой машине всё должно быть installed.
        // repo_root резолвим из DUB_STUDIO_ROOT (задаётся в CI/приёмке), иначе — CARGO_MANIFEST_DIR/../..
        let repo_root = std::env::var("DUB_STUDIO_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
            });
        if !repo_root.join("models").is_dir() {
            eprintln!("skip: нет {}/models (не приёмочная машина)", repo_root.display());
            return;
        }
        let st = setup_status(&repo_root);
        let mut broken = Vec::new();
        for c in &st.components {
            // Драйвер зависит от железа; на приёмочной машине RTX 4090 он есть, но в headless CI может не быть.
            if c.delivery == Delivery::External {
                continue;
            }
            // Опциональные компоненты (альт-кванты Gemma q5/q6/q8, Higgs q6/q4 — по 5-12ГБ каждый) —
            // это ВЗАИМОЗАМЕНЯЕМЫЕ альтернативы дефолту, их не качают все сразу. Проверяем лишь дефолтный
            // (Required) стек. Что альт-кванты реально скачиваются — проверено byte-range probe URL'ов.
            if c.requirement == Requirement::Optional {
                continue;
            }
            if !c.installed {
                broken.push(format!("{} (missing: {:?})", c.id, c.missing));
            }
        }
        assert!(broken.is_empty(), "не установлены компоненты: {broken:?}");
    }
}
