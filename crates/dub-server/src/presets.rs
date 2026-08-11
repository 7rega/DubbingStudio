//! Пресеты настроек под железо: автоопределение GPU/VRAM (NVML) -> рекомендованный пресет квантов, но
//! юзер применяет любой сам. Пресет = бандл selection-ключей (tts/mt/asr_engine + or_*_on). «Облако»
//! выносит тяжёлые модели (LLM/vision/TTS) в OpenRouter — разгрузка GPU и скорость. Сепарация,
//! диаризация и локальный ASR остаются на GPU, поэтому NVIDIA всё равно нужна.
//!
//! Кванты: Higgs TTS q8_0>q6_k>q4_k_m, Gemma q8_0>q6_k>q5_0>q4_0. Больше VRAM -> выше квант.
//! backend cuda/cpu тут НЕ трогаем — он авто-детектится по наличию CUDA-либ.

use serde::Serialize;
use std::path::Path;

/// Один пресет: id (стабильный ключ), человекочитаемое имя, и набор selection-ключей к применению.
pub struct Preset {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    /// Минимум VRAM в ГБ для авто-рекомендации этого пресета (0 = облако/любой).
    pub min_vram_gb: f64,
    /// Ключи active.json, которые ставит пресет. «custom» -> пусто (ничего не трогаем).
    pub keys: &'static [(&'static str, &'static str)],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "rtx5090",
        title: "RTX 5090 (32 ГБ)",
        subtitle: "Максимальное качество — топовые кванты локально",
        min_vram_gb: 30.0,
        keys: &[("tts", "q8_0"), ("mt", "q8_0"), ("asr_engine", "parakeet"), ("local_backend", "gpu"), ("or_llm_on", "0"), ("or_vision_on", "0"), ("or_tts_on", "0")],
    },
    Preset {
        id: "rtx4090",
        title: "RTX 4090 (24 ГБ)",
        subtitle: "Максимальное качество — топовые кванты локально",
        min_vram_gb: 22.0,
        keys: &[("tts", "q8_0"), ("mt", "q8_0"), ("asr_engine", "parakeet"), ("local_backend", "gpu"), ("or_llm_on", "0"), ("or_vision_on", "0"), ("or_tts_on", "0")],
    },
    Preset {
        id: "gpu16",
        title: "GPU 16 ГБ",
        subtitle: "Высокое качество (4080/4070 Ti и подобные)",
        min_vram_gb: 15.0,
        keys: &[("tts", "q8_0"), ("mt", "q6_k"), ("asr_engine", "parakeet"), ("local_backend", "gpu"), ("or_llm_on", "0"), ("or_vision_on", "0"), ("or_tts_on", "0")],
    },
    Preset {
        id: "gpu12",
        title: "GPU 12 ГБ",
        subtitle: "Сбалансированно (3060/4070 и подобные)",
        min_vram_gb: 11.0,
        keys: &[("tts", "q6_k"), ("mt", "q5_0"), ("asr_engine", "parakeet"), ("local_backend", "gpu"), ("or_llm_on", "0"), ("or_vision_on", "0"), ("or_tts_on", "0")],
    },
    Preset {
        id: "gpu8",
        title: "GPU 8 ГБ",
        subtitle: "Экономный — лёгкие кванты (3060 Ti/4060)",
        min_vram_gb: 7.0,
        keys: &[("tts", "q4_k_m"), ("mt", "q4_0"), ("asr_engine", "parakeet"), ("local_backend", "gpu"), ("or_llm_on", "0"), ("or_vision_on", "0"), ("or_tts_on", "0")],
    },
    Preset {
        id: "weak-nvidia-cloud",
        title: "Слабая NVIDIA + облако",
        subtitle: "Тяжёлое (перевод/vision/озвучка) в OpenRouter, сепарация и ASR на вашей GPU",
        min_vram_gb: 0.0,
        keys: &[("local_backend", "gpu"), ("or_llm_on", "1"), ("or_vision_on", "1"), ("or_tts_on", "1")],
    },
    Preset {
        id: "cloud",
        title: "CPU + облако (без NVIDIA)",
        subtitle: "Тяжёлое в OpenRouter, локальное на процессоре — запускается без видеокарты (нужен ключ)",
        min_vram_gb: 0.0,
        keys: &[("local_backend", "cpu"), ("or_llm_on", "1"), ("or_vision_on", "1"), ("or_tts_on", "1")],
    },
    Preset {
        id: "custom",
        title: "Пользовательский",
        subtitle: "Настрою каждый параметр вручную",
        min_vram_gb: -1.0,
        keys: &[],
    },
];

/// Найти пресет по id.
pub fn by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Снимок железа + рекомендованный пресет (для UI).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareRecommendation {
    pub gpu_name: String,
    pub total_vram_gb: f64,
    pub total_ram_gb: f64,
    pub has_gpu: bool,
    pub recommended: &'static str,
    pub reason: String,
}

/// Рекомендовать пресет по железу: явное совпадение имени карты (5090/4090) приоритетнее, иначе по VRAM;
/// нет NVIDIA GPU или <7 ГБ -> облако (локально не потянет комфортно).
pub fn recommend() -> HardwareRecommendation {
    let hw = crate::hw::snapshot();
    let vram_gb = hw.total_vram as f64 / (1024.0 * 1024.0 * 1024.0);
    let ram_gb = hw.total_ram as f64 / (1024.0 * 1024.0 * 1024.0);
    let has_gpu = hw.total_vram > 0;
    let name_lc = hw.gpu_name.to_lowercase();

    let (recommended, reason) = if !has_gpu {
        ("cloud", "NVIDIA GPU не найдена — режим «CPU + облако»: тяжёлое в OpenRouter, локальное на процессоре (медленнее, но работает)".to_string())
    } else if name_lc.contains("5090") {
        ("rtx5090", format!("Обнаружена {} — максимальные кванты", hw.gpu_name))
    } else if name_lc.contains("5080") || name_lc.contains("4090") {
        ("rtx4090", format!("Обнаружена {} — максимальные кванты", hw.gpu_name))
    } else if name_lc.contains("5070 ti") || name_lc.contains("4070 ti") || name_lc.contains("4080") {
        ("gpu16", format!("Обнаружена {} — высокое качество (GPU 16 ГБ)", hw.gpu_name))
    } else if name_lc.contains("5070") || name_lc.contains("4070") || name_lc.contains("3060") {
        ("gpu12", format!("Обнаружена {} — сбалансированные кванты (GPU 12 ГБ)", hw.gpu_name))
    } else {
        // По объёму VRAM: берём первый пресет, чей порог не выше фактического (PRESETS отсортированы по убыванию).
        let pick = PRESETS
            .iter()
            .filter(|p| p.min_vram_gb >= 0.0 && p.id != "cloud")
            .find(|p| vram_gb >= p.min_vram_gb);
        match pick {
            Some(p) => (p.id, format!("{} · {:.0} ГБ VRAM — {}", hw.gpu_name, vram_gb, p.title)),
            None => ("cloud", format!("{} · {:.0} ГБ VRAM маловато для локали — облако надёжнее", hw.gpu_name, vram_gb)),
        }
    };

    HardwareRecommendation { gpu_name: hw.gpu_name, total_vram_gb: vram_gb, total_ram_gb: ram_gb, has_gpu, recommended, reason }
}

/// Применить пресет: записать все его ключи в active.json. «custom» -> ничего (юзер сам). Возвращает
/// список применённых пар для показа/лога.
pub fn apply(models_root: &Path, id: &str) -> Result<Vec<(String, String)>, String> {
    let preset = by_id(id).ok_or_else(|| format!("неизвестный пресет: {id}"))?;
    let mut applied = Vec::new();
    crate::models::set_selection(models_root, "preset", id).map_err(|e| format!("запись preset: {e}"))?;
    applied.push(("preset".to_string(), id.to_string()));
    for (k, v) in preset.keys {
        crate::models::set_selection(models_root, k, v).map_err(|e| format!("запись {k}: {e}"))?;
        applied.push((k.to_string(), v.to_string()));
    }
    Ok(applied)
}
