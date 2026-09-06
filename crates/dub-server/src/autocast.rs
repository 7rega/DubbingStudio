//! Автоподбор голосов (Auto-Cast) из папки voices/ по спикерам и актёрам.
//!
//! Гибридный алгоритм:
//! 1. Определение пола по F0 (<165 Гц — male, >=165 Гц — female) из чистых вокалов (vocals16.wav).
//! 2. Тембральное сходство через 256-d эмбеддинги WeSpeaker ResNet34-LM (если доступна модель).
//! 3. Ранжирование по суммарной длительности речи: главные герои выбирают лучшие доноры первыми.
//! 4. Повтор доноров из пака при дефиците уникальных голосов (без фолбэка на клон).
//! 5. Учёт назначенных актёров (casting.json или segment.speaker).
//! 6. Двухуровневая иерархия: запись в proj.audio.voice.name (CSV), s.voice = None (сохраняя donor: и clone:).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use dub_core::Project;

use crate::wavio;

/// Порог основного тона для разделения полов (Гц).
pub const GENDER_F0_THRESHOLD_HZ: f32 = 160.0;
/// Порог основного тона для детских голосов в паках доноров без маркеров (Гц).
pub const CHILD_F0_THRESHOLD_HZ: f32 = 300.0;

/// Профиль одного голоса из библиотеки voices/.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PackVoiceProfile {
    pub name: String,
    pub rel_path: String,
    pub gender: String, // "male" | "female" | "boy" | "girl" | "child"
    pub f0: f32,
    #[serde(default)]
    pub embedding: Vec<f32>, // 256-d WeSpeaker (может быть пуст, если модель недоступна)
    pub mtime: u64,
}

/// Кэш базы голосов для исключения повторного анализа на каждом клике.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct VoiceCache {
    voices: HashMap<String, PackVoiceProfile>,
}

impl VoiceCache {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, data);
        }
    }
}

/// Расчёт медианы F0 через проверенный детектор crate::f0::median_f0 с нормированной
/// кросс-корреляцией и защитой от октавных ошибок (halving/doubling).
pub fn estimate_f0_median(samples: &[f32], sr: u32) -> Option<f32> {
    crate::f0::median_f0(samples, sr).map(|f| f as f32)
}

/// Скалярное произведение L2-нормированных векторов (косинусное сходство).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Получение mtime файла в миллисекундах.
fn file_mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Проверка наличия хотя бы одного токена или подстроки в нижнем регистре.
fn matches_voice_marker(lower: &str, exact_tokens: &[&str], substrings: &[&str]) -> bool {
    for sub in substrings {
        if lower.contains(sub) {
            return true;
        }
    }
    for part in lower.split(['_', '-', ' ', '.', '(', ')', '[', ']', '/', '\\', ':', ';', ',', '!']) {
        if part.is_empty() {
            continue;
        }
        for tok in exact_tokens {
            if part == *tok {
                return true;
            }
        }
    }
    false
}

/// Определение категории донорского голоса из пака voices/ (male, female, boy, girl, child).
/// Анализирует маркеры в имени файла или опирается на основной тон F0.
pub fn detect_voice_gender(name: &str, f0: f32) -> String {
    let lower = name.to_lowercase();

    // 1. Мальчик (Boy)
    if matches_voice_marker(
        &lower,
        &["boy", "мальчик", "пацан", "паренёк", "паренек", "сынок", "сын", "внук", "школьник"],
        &["_boy_", "_boy", "boy_", "ru_boy", "en_boy", "kid_m", "child_m"],
    ) {
        return "boy".to_string();
    }

    // 2. Девочка (Girl)
    if matches_voice_marker(
        &lower,
        &["girl", "девочка", "дочка", "дочь", "внучка", "школьница"],
        &["_girl_", "_girl", "girl_", "ru_girl", "en_girl", "kid_f", "child_f"],
    ) {
        return "girl".to_string();
    }

    // 3. Общий детский маркер (Child / Kid)
    if matches_voice_marker(
        &lower,
        &["child", "kid", "kids", "ребенок", "ребёнок", "дитя", "детский", "детск"],
        &["_child_", "_child", "child_", "ru_child", "en_child", "_kid_", "_kid", "kid_"],
    ) {
        return "child".to_string();
    }

    // 4. Взрослый женский голос (Female)
    if matches_voice_marker(
        &lower,
        &["female", "famale", "жен", "женщина", "девушка", "мать", "мама", "woman", "lady"],
        &["_fem_", "_fem", "fem_", "ru_fem", "en_fem", "female", "famale"],
    ) {
        return "female".to_string();
    }

    // 5. Взрослый мужской голос (Male)
    if matches_voice_marker(
        &lower,
        &["male", "муж", "мужчина", "парень", "отец", "папа", "man", "guy"],
        &["_male_", "_male", "male_", "_m_", "ru_male", "en_male"],
    ) {
        return "male".to_string();
    }

    // 6. Акустическое разделение по F0 (если маркеров в имени файла нет)
    if f0 >= CHILD_F0_THRESHOLD_HZ {
        "child".to_string()
    } else if f0 < GENDER_F0_THRESHOLD_HZ {
        "male".to_string()
    } else {
        "female".to_string()
    }
}

/// Определение категории спикера/персонажа из видео (male, female, boy, girl, child).
/// В отличие от доноров, персонаж видео может быть признан ребёнком ТОЛЬКО при наличии
/// детских маркеров в имени/роли («Мальчик», «Девочка», «Сын», «Kid»).
/// Обычные взрослые роли («Врач», «Игрок», «Спикер 0») всегда классифицируются как взрослые
/// (male/female), даже при эмоциональных выкриках с высокой частотой.
pub fn detect_speaker_gender(
    name: &str,
    f0: f32,
    casting_gender: Option<&str>,
    tract_gender: Option<&str>,
) -> String {
    let lower = name.to_lowercase();

    // 1. Проверяем явные детские маркеры в имени персонажа
    if matches_voice_marker(
        &lower,
        &["boy", "мальчик", "пацан", "паренёк", "паренек", "сынок", "сын", "внук", "школьник"],
        &["_boy_", "_boy", "boy_", "ru_boy", "en_boy", "kid_m", "child_m"],
    ) {
        return "boy".to_string();
    }
    if matches_voice_marker(
        &lower,
        &["girl", "девочка", "дочка", "дочь", "внучка", "школьница"],
        &["_girl_", "_girl", "girl_", "ru_girl", "en_girl", "kid_f", "child_f"],
    ) {
        return "girl".to_string();
    }
    if matches_voice_marker(
        &lower,
        &["child", "kid", "kids", "ребенок", "ребёнок", "дитя", "детский", "детск"],
        &["_child_", "_child", "child_", "ru_child", "en_child", "_kid_", "_kid", "kid_"],
    ) {
        return "child".to_string();
    }

    // 2. Если в casting.json задан пол персонажа (из распознавания лиц или кастинга)
    if let Some(cg) = casting_gender {
        let cg_lower = cg.trim().to_lowercase();
        if cg_lower == "female" || cg_lower == "жен" || cg_lower == "женский" {
            return "female".to_string();
        }
        if cg_lower == "male" || cg_lower == "муж" || cg_lower == "мужской" {
            return "male".to_string();
        }
    }

    // 3. Маркеры пола в имени/роли персонажа
    if matches_voice_marker(
        &lower,
        &["female", "famale", "жен", "женщина", "девушка", "мать", "мама", "woman", "lady", "дочь", "дочка", "сестра", "бабушка"],
        &["_fem_", "_fem", "fem_", "ru_fem", "en_fem", "female", "famale"],
    ) {
        return "female".to_string();
    }
    if matches_voice_marker(
        &lower,
        &["male", "муж", "мужчина", "парень", "отец", "папа", "man", "guy", "player", "игрок", "coach", "тренер", "дедушка", "брат", "боец", "солдат"],
        &["_male_", "_male", "male_", "_m_", "ru_male", "en_male"],
    ) {
        return "male".to_string();
    }

    // 4. Физическая частота связок F0:
    // Мужской диапазон в нормальной речи: до 160 Гц.
    // Физика вибрации мужских связок первична — спокойный голос <= 160 Гц не может принадлежать женщине!
    if f0 <= GENDER_F0_THRESHOLD_HZ {
        return "male".to_string();
    }

    // 5. Если частота выше 160 Гц:
    // Это либо взрослая женщина (норма 165-255 Гц), либо эмоционально кричащий взрослый мужчина (220-350 Гц).
    // Если WeSpeaker уверенно подтверждает мужской голосовой тракт — защищаем кричащего мужчину:
    if tract_gender == Some("male") {
        return "male".to_string();
    }

    // По умолчанию выше 160 Гц — взрослая женщина
    "female".to_string()
}

/// Определение пола по голосовому тракту WeSpeaker (форманты и тембр)
/// по отношению к профилям доноров пака.
/// Сравниваем топ-3 ближайших мужских и топ-3 ближайших женских доноров.
/// Возвращает Some("male") только при заметном перевесе мужского сходства над женским.
fn detect_tract_gender(emb: &[f32], pack: &[PackVoiceProfile]) -> Option<&'static str> {
    if emb.is_empty() {
        return None;
    }
    let mut fem_sims: Vec<f32> = Vec::new();
    let mut male_sims: Vec<f32> = Vec::new();

    for p in pack {
        if p.embedding.is_empty() {
            continue;
        }
        let sim = cosine_similarity(emb, &p.embedding);
        if p.gender == "female" || p.gender == "girl" {
            fem_sims.push(sim);
        } else if p.gender == "male" || p.gender == "boy" {
            male_sims.push(sim);
        }
    }

    if fem_sims.is_empty() || male_sims.is_empty() {
        return None;
    }

    fem_sims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    male_sims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let k = 3;
    let fem_top_k = &fem_sims[..fem_sims.len().min(k)];
    let male_top_k = &male_sims[..male_sims.len().min(k)];

    let fem_avg: f32 = fem_top_k.iter().sum::<f32>() / fem_top_k.len() as f32;
    let male_avg: f32 = male_top_k.iter().sum::<f32>() / male_top_k.len() as f32;

    let margin = male_avg - fem_avg;
    if margin > 0.03 {
        Some("male")
    } else if (fem_avg - male_avg) > 0.05 {
        Some("female")
    } else {
        None
    }
}

/// Быстрый линейный ресемпл в памяти (без I/O и изменений файлов на диске).
fn resample_linear(input: &[f32], src_sr: u32, dst_sr: u32) -> Vec<f32> {
    if input.is_empty() || src_sr == dst_sr {
        return input.to_vec();
    }
    let ratio = dst_sr as f64 / src_sr as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Сбор аудиофайлов из каталога (с опциональной рекурсией).
fn collect_audio_files(dir: &Path, rel_prefix: &str, recursive: bool, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if recursive {
                let next_prefix = if rel_prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{rel_prefix}/{name}")
                };
                collect_audio_files(&p, &next_prefix, true, out);
            }
        } else if p.is_file() {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            if ext == "wav" || ext == "mp3" || ext == "ogg" || ext == "flac" || ext == "m4a" {
                let rel = if rel_prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{rel_prefix}/{name}")
                };
                out.push((rel, p));
            }
        }
    }
}

/// Сканирование каталога голосов (с кэшированием в .voices_cache.json).
pub fn scan_pack_voices(
    voices_dir: &Path,
    subfolder: Option<&str>,
    models_root: &Path,
    tmp_dir: &Path,
) -> Vec<PackVoiceProfile> {
    let normalized = subfolder.unwrap_or("").trim();
    let (target_dir, rel_base, recursive) = if normalized.eq_ignore_ascii_case("all") {
        (voices_dir.to_path_buf(), "".to_string(), true)
    } else if normalized.is_empty() || normalized.eq_ignore_ascii_case("root") {
        (voices_dir.to_path_buf(), "".to_string(), false)
    } else {
        let clean_sub = normalized.replace('\\', "/");
        (voices_dir.join(&clean_sub), clean_sub, true)
    };

    let cache_file = voices_dir.join(".voices_cache.json");
    let mut cache = VoiceCache::load(&cache_file);
    let mut cache_modified = false;

    let mut audio_files = Vec::new();
    collect_audio_files(&target_dir, &rel_base, recursive, &mut audio_files);

    if audio_files.is_empty() {
        return Vec::new();
    }

    // Инициализируем WeSpeaker эмбеддер, если доступен
    let onnx_path = std::env::var("DUB_FACES_WESPEAKER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dub_faces::wespeaker_path(models_root));
    let mut embedder = if onnx_path.is_file() {
        dub_faces::VoiceEmbedder::load(&onnx_path).ok()
    } else {
        None
    };

    let mut profiles = Vec::new();

    for (rel_path, file_path) in audio_files {
        let stem = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if stem.is_empty() {
            continue;
        }

        let mtime = file_mtime(&file_path);

        if let Some(mut cached) = cache.voices.get(&rel_path).cloned() {
            if cached.mtime == mtime && (!cached.embedding.is_empty() || embedder.is_none()) {
                let fresh_gender = detect_voice_gender(&stem, cached.f0);
                if cached.gender != fresh_gender {
                    cached.gender = fresh_gender;
                    cache.voices.insert(rel_path.clone(), cached.clone());
                    cache_modified = true;
                }
                profiles.push(cached);
                continue;
            }
        }

        // Читаем аудио: wavio::read_mono_f32 читает PCM WAV, а при любых сбоях (tag 85, MP3-in-WAV)
        // автоматически декодирует через ffmpeg pipe в память. Если прямой pipe дал сбой — резервный media::trim.
        let samples_sr: Option<(Vec<f32>, u32)> = wavio::read_mono_f32(&file_path).ok().or_else(|| {
            let tmp_wav = tmp_dir.join(format!("scan_tmp_{}.wav", stem));
            let res = crate::media::trim(&file_path, &tmp_wav, 0.0, 15.0, 16_000)
                .ok()
                .and_then(|_| wavio::read_mono_f32(&tmp_wav).ok());
            let _ = std::fs::remove_file(&tmp_wav);
            res
        });

        let Some((samples, sr)) = samples_sr else {
            continue;
        };

        let f0 = estimate_f0_median(&samples, sr).unwrap_or(150.0);
        let gender = detect_voice_gender(&stem, f0);

        // Извлекаем 256-d WeSpeaker эмбеддинг через быстрый in-memory ресемплинг до 16 000 Гц
        let embedding = if let Some(ref mut emb) = embedder {
            let samples_16k = if sr == 16_000 {
                samples.clone()
            } else {
                resample_linear(&samples, sr, 16_000)
            };
            let max_len = 16_000 * 10;
            let slice = if samples_16k.len() > max_len {
                &samples_16k[..max_len]
            } else {
                &samples_16k
            };
            if !slice.is_empty() {
                emb.embed_samples(slice).unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let profile = PackVoiceProfile {
            name: stem,
            rel_path: rel_path.clone(),
            gender,
            f0,
            embedding,
            mtime,
        };

        cache.voices.insert(rel_path, profile.clone());
        cache_modified = true;
        profiles.push(profile);
    }

    if cache_modified {
        cache.save(&cache_file);
    }

    profiles
}

/// Информация о спикере/актёре для матчинга.
struct SpeakerCandidate {
    spk_id: String,
    actor_name: String,
    gender: String,
    f0: f32,
    embedding: Vec<f32>,
    line_count: usize,
    total_speech_sec: f64,
}

impl SpeakerCandidate {
    /// Приоритет при подборе: главные герои (много реплик + хронометраж) первыми!
    pub fn priority(&self) -> f64 {
        (self.line_count as f64) * 10.0 + self.total_speech_sec
    }
}

/// Сбор реплик конкретного спикера из вокала и вычисление его профиля.
fn extract_speaker_candidate(
    spk_id: &str,
    actor_name: String,
    casting_gender: Option<&str>,
    pack_profiles: &[PackVoiceProfile],
    proj: &Project,
    vocals_samples: &[f32],
    sr: u32,
    embedder: &mut Option<dub_faces::VoiceEmbedder>,
) -> SpeakerCandidate {
    let mut collected = Vec::new();
    let mut total_sec = 0.0f64;
    let mut line_count = 0usize;
    let max_sample_len = (sr as f64 * 15.0) as usize; // до 15с сэмплов для замера

    // Собираем отрезки речи спикера
    for s in &proj.segments {
        let spk = s.speaker.as_deref().unwrap_or("0");
        if spk == spk_id && s.end > s.start {
            let dur = s.end - s.start;
            total_sec += dur;
            line_count += 1;

            if collected.len() < max_sample_len {
                let start_idx = ((s.start * sr as f64).round() as usize).min(vocals_samples.len());
                let end_idx = ((s.end * sr as f64).round() as usize).min(vocals_samples.len());
                if end_idx > start_idx {
                    collected.extend_from_slice(&vocals_samples[start_idx..end_idx]);
                }
            }
        }
    }

    let embedding = if let Some(ref mut emb) = embedder {
        let samples_16k = if sr == 16_000 {
            collected.clone()
        } else {
            resample_linear(&collected, sr, 16_000)
        };
        if !samples_16k.is_empty() {
            emb.embed_samples(&samples_16k).unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let tract_gender = detect_tract_gender(&embedding, pack_profiles);

    let f0_measured = estimate_f0_median(&collected, sr);
    let f0 = match f0_measured {
        Some(val) => val,
        None => {
            // Если F0 не удалось замерить физически (тишина/шепот):
            if tract_gender == Some("female") {
                210.0
            } else {
                135.0
            }
        }
    };

    let gender = detect_speaker_gender(&actor_name, f0, casting_gender, tract_gender);

    SpeakerCandidate {
        spk_id: spk_id.to_string(),
        actor_name,
        gender,
        f0,
        embedding,
        line_count,
        total_speech_sec: total_sec,
    }
}

/// Вычисление скора совместимости спикера и донорского голоса.
fn compute_match_score(spk: &SpeakerCandidate, donor: &PackVoiceProfile) -> f32 {
    let mut score = 0.0f32;

    // 1. Тембральное сходство WeSpeaker (косинус [-1..1])
    let has_embs = !spk.embedding.is_empty() && !donor.embedding.is_empty();
    if has_embs {
        let cos = cosine_similarity(&spk.embedding, &donor.embedding);
        score += cos;
    }

    // 2. Штраф за разницу регистров F0 (высоты тона)
    let f0_diff = (donor.f0 - spk.f0).abs();
    let f0_penalty = (f0_diff / spk.f0.max(50.0)) * 0.25;
    score -= f0_penalty;

    // Если эмбеддингов нет вообще — скоринг чисто по минимальной дельте F0
    if !has_embs {
        score = -f0_diff;
    }

    score
}

/// Выполнить автоподбор голосов и обновить проект.
pub fn auto_assign_pack_voices_direct(
    proj: &mut Project,
    vocals16: &Path,
    voices_dir: &Path,
    models_root: &Path,
    tmp_dir: &Path,
    subfolder: Option<&str>,
) -> Result<Vec<String>, String> {
    // 1. Уникальные ID спикеров
    let sorted_spks: Vec<String> = proj
        .segments
        .iter()
        .map(|s| s.speaker.clone().unwrap_or_else(|| "0".to_string()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    if sorted_spks.is_empty() {
        return Ok(vec!["В проекте нет реплик со спикерами".into()]);
    }

    // 2. Сканируем пак доноров
    let pack_profiles = scan_pack_voices(voices_dir, subfolder, models_root, tmp_dir);
    if pack_profiles.is_empty() {
        return Err("В каталоге voices/ не найдены аудиофайлы (.wav / .mp3)".into());
    }

    // 3. Читаем вокал проекта
    let (vocals_samples, sr) = wavio::read_mono_f32(vocals16)
        .map_err(|e| format!("Не удалось прочитать {}: {e}", vocals16.display()))?;

    // 4. Подгружаем имена персонажей из casting.json (если кастинг проводился)
    let project_dir = vocals16.parent().unwrap_or_else(|| Path::new("."));
    let casting_path = if project_dir.is_file() {
        project_dir.to_path_buf()
    } else {
        project_dir.join("casting.json")
    };
    let casting_opt = dub_faces::load_casting(&casting_path);

    // 5. Инициализируем WeSpeaker для вокала проекта
    let onnx_path = std::env::var("DUB_FACES_WESPEAKER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dub_faces::wespeaker_path(models_root));
    let mut embedder = if onnx_path.is_file() {
        dub_faces::VoiceEmbedder::load(&onnx_path).ok()
    } else {
        None
    };

    // 6. Извлекаем профили спикеров/актёров
    let mut candidates: Vec<SpeakerCandidate> = Vec::new();
    for spk_id in &sorted_spks {
        let char_match = casting_opt.as_ref().and_then(|c| {
            c.characters
                .iter()
                .find(|ch| ch.speaker_ids.iter().any(|s| s == spk_id))
        });

        let actor_name = char_match
            .map(|ch| ch.name.trim())
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("Спикер {spk_id}"));

        let casting_gender = char_match
            .map(|ch| ch.gender.as_str())
            .filter(|g| !g.trim().is_empty());

        candidates.push(extract_speaker_candidate(
            spk_id,
            actor_name,
            casting_gender,
            &pack_profiles,
            proj,
            &vocals_samples,
            sr,
            &mut embedder,
        ));
    }

    // 7. Сортируем очередь подбора: главные герои (много реплик + хронометраж) первыми!
    candidates.sort_by(|a, b| {
        b.priority()
            .partial_cmp(&a.priority())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 8. Распределение доноров
    let mut assigned_voices: HashMap<String, String> = HashMap::new();
    let mut assigned_scores: HashMap<String, f32> = HashMap::new();
    let mut used_voices: BTreeSet<String> = BTreeSet::new();

    for spk in &candidates {
        // Фильтр по категории (пол / возраст) с каскадной иерархией приоритетов
        let mut pool: Vec<&PackVoiceProfile> = match spk.gender.as_str() {
            "boy" => {
                // 1. Точное совпадение: голоса мальчиков
                let exact: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "boy").collect();
                if !exact.is_empty() {
                    exact
                } else {
                    // 2. Нейтральные детские голоса
                    let kids: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "child").collect();
                    if !kids.is_empty() {
                        kids
                    } else {
                        // 3. Молодые женские голоса (травести)
                        let female: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "female").collect();
                        if !female.is_empty() {
                            female
                        } else {
                            pack_profiles.iter().collect()
                        }
                    }
                }
            }
            "girl" => {
                // 1. Точное совпадение: голоса девочек
                let exact: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "girl").collect();
                if !exact.is_empty() {
                    exact
                } else {
                    // 2. Нейтральные детские голоса
                    let kids: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "child").collect();
                    if !kids.is_empty() {
                        kids
                    } else {
                        // 3. Женские голоса
                        let female: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "female").collect();
                        if !female.is_empty() {
                            female
                        } else {
                            pack_profiles.iter().collect()
                        }
                    }
                }
            }
            "child" => {
                // Если пол ребенка не определен — объединяем всех детей (мальчики, девочки, нейтральные)
                // Нейросеть WeSpeaker выберет ближайший тембр
                let kids: Vec<_> = pack_profiles
                    .iter()
                    .filter(|p| p.gender == "boy" || p.gender == "girl" || p.gender == "child")
                    .collect();
                if !kids.is_empty() {
                    kids
                } else {
                    let female: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "female").collect();
                    if !female.is_empty() {
                        female
                    } else {
                        pack_profiles.iter().collect()
                    }
                }
            }
            "female" => {
                // 1. Взрослые женские голоса
                let exact: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "female").collect();
                if !exact.is_empty() {
                    exact
                } else {
                    // Если взрослых женских нет — пробуем девочек/детей, затем любые
                    let kids: Vec<_> = pack_profiles
                        .iter()
                        .filter(|p| p.gender == "girl" || p.gender == "child")
                        .collect();
                    if !kids.is_empty() {
                        kids
                    } else {
                        pack_profiles.iter().collect()
                    }
                }
            }
            "male" => {
                // 1. Взрослые мужские голоса
                let exact: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "male").collect();
                if !exact.is_empty() {
                    exact
                } else {
                    // Если взрослых мужских нет — пробуем мальчиков, затем любые
                    let boys: Vec<_> = pack_profiles.iter().filter(|p| p.gender == "boy").collect();
                    if !boys.is_empty() {
                        boys
                    } else {
                        pack_profiles.iter().collect()
                    }
                }
            }
            _ => {
                let exact: Vec<_> = pack_profiles.iter().filter(|p| p.gender == spk.gender).collect();
                if !exact.is_empty() {
                    exact
                } else {
                    pack_profiles.iter().collect()
                }
            }
        };

        // Если голосов нужной категории в паке вообще нет — берём любые
        if pool.is_empty() {
            pool = pack_profiles.iter().collect();
        }

        // Сортируем кандидатов:
        // 1. Сначала ещё не использованные голоса (is_used = false)
        // 2. Затем по максимальному скору совместимости
        pool.sort_by(|a, b| {
            let used_a = used_voices.contains(&a.name);
            let used_b = used_voices.contains(&b.name);
            let score_a = compute_match_score(spk, a);
            let score_b = compute_match_score(spk, b);

            used_a
                .cmp(&used_b)
                .then_with(|| score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal))
        });

        if let Some(best) = pool.first() {
            assigned_voices.insert(spk.spk_id.clone(), best.name.clone());
            assigned_scores.insert(spk.spk_id.clone(), compute_match_score(spk, best));
            used_voices.insert(best.name.clone());
        }
    }

    // Если casting.json существовал, синхронизируем dub_voice у персонажей кастинга
    if let Some(mut casting) = casting_opt {
        let mut casting_changed = false;
        for ch in &mut casting.characters {
            for spk_id in &ch.speaker_ids {
                if let Some(assigned) = assigned_voices.get(spk_id) {
                    if ch.dub_voice != *assigned {
                        ch.dub_voice = assigned.clone();
                        casting_changed = true;
                    }
                }
            }
        }
        if casting_changed {
            let _ = dub_faces::save_casting(&casting_path, &casting);
        }
    }

    // 9. Формируем позиционный CSV для proj.audio.voice.name в лексикографическом порядке спикеров
    let voice_names: Vec<String> = sorted_spks
        .iter()
        .map(|id| {
            assigned_voices
                .get(id)
                .cloned()
                .unwrap_or_else(|| pack_profiles[0].name.clone())
        })
        .collect();

    let csv = voice_names.join(", ");
    proj.audio.voice.mode = "voice".to_string();
    proj.audio.voice.name = Some(csv);
    proj.audio.mix_dirty = true;

    // 10. Очищаем s.voice у обычных реплик, сохраняя точечные оверрайды
    for s in &mut proj.segments {
        if let Some(v) = &s.voice {
            if !v.starts_with("donor:") && !v.starts_with("clone:") {
                s.voice = None;
            }
        }
        s.dirty = true; // Помечаем для ре-синтеза
    }

    // 11. Формируем человекочитаемый отчёт с именами актёров и статистикой
    let summary: Vec<String> = candidates
        .iter()
        .map(|spk| {
            let v = assigned_voices
                .get(&spk.spk_id)
                .map(|s| s.as_str())
                .unwrap_or("-");
            let gender_ru = match spk.gender.as_str() {
                "boy" => "мальчик",
                "girl" => "девочка",
                "child" => "ребёнок",
                "male" => "муж.",
                "female" => "жен.",
                _ => "голос",
            };
            format!(
                "{} ({} фраз, {:.1}с, {}, {:.0} Гц) -> {}",
                spk.actor_name, spk.line_count, spk.total_speech_sec, gender_ru, spk.f0, v
            )
        })
        .collect();

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_speaker_gender_rules() {
        // Парни (Elliot, Kane, Coach, Opponent) с глубокими мужскими голосами:
        assert_eq!(detect_speaker_gender("Elliot", 114.3, None, None), "male");
        assert_eq!(detect_speaker_gender("Kane", 116.8, None, None), "male");
        assert_eq!(detect_speaker_gender("Coach", 94.1, None, None), "male");
        assert_eq!(detect_speaker_gender("Opponent", 111.1, None, None), "male");

        // Женщины («Doctor», «Врач» ~163-220 Гц):
        assert_eq!(detect_speaker_gender("Doctor", 163.3, None, None), "female");
        assert_eq!(detect_speaker_gender("Врач", 210.0, None, None), "female");
        assert_eq!(detect_speaker_gender("Врач", 150.0, Some("female"), None), "female");

        // Кричащий взрослый мужчина (Player на 291 Гц):
        assert_eq!(detect_speaker_gender("Player", 290.9, None, Some("male")), "male");
        assert_eq!(detect_speaker_gender("Игрок", 260.0, None, Some("male")), "male");

        // Детские маркеры:
        assert_eq!(detect_speaker_gender("Мальчик", 240.0, None, None), "boy");
        assert_eq!(detect_speaker_gender("Девочка", 250.0, None, None), "girl");
        assert_eq!(detect_speaker_gender("Ребёнок", 280.0, None, None), "child");
        assert_eq!(detect_speaker_gender("Сын", 220.0, None, None), "boy");
        assert_eq!(detect_speaker_gender("Дочь", 220.0, None, None), "girl");
    }

    #[test]
    fn test_detect_voice_gender_library() {
        assert_eq!(detect_voice_gender("RU_Male_Dmitry", 120.0), "male");
        assert_eq!(detect_voice_gender("RU_Female_Anna", 220.0), "female");
        assert_eq!(detect_voice_gender("RU_Boy_Anton", 270.0), "boy");
        assert_eq!(detect_voice_gender("RU_Girl_Alisa", 280.0), "girl");
    }
}
