//! Модуль управления библиотекой голосов Dub Studio (voices/).
//!
//! Обеспечивает:
//! 1. Рекурсивный поиск аудиофайлов (.wav / .mp3) в voices/, voices/cast/ и любых подпапках-паках.
//! 2. Поиск сопутствующих транскрипций (.txt).
//! 3. Получение списка подкаталогов-паков для выбора в UI и автоподбора.
//! 4. Составление детального списка голосов с фильтрацией по подкаталогам.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct VoiceInfo {
    pub name: String,
    pub subfolder: Option<String>,
    pub rel_path: String,
}

/// Очистка входного имени от path-traversal (../ и т.д.)
fn clean_voice_query(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    // Защита от path traversal
    let mut parts = Vec::new();
    for seg in s.split(['/', '\\']) {
        let seg = seg.trim();
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        parts.push(seg);
    }
    parts.join("/")
}

/// Получение стема из имени (отсекая .wav/.mp3/.ogg/.flac/.m4a/.txt, если они присутствуют)
fn extract_stem(s: &str) -> &str {
    let s = s.trim();
    if let Some(pos) = s.rfind('.') {
        let ext = &s[pos + 1..];
        if ext.eq_ignore_ascii_case("wav")
            || ext.eq_ignore_ascii_case("mp3")
            || ext.eq_ignore_ascii_case("ogg")
            || ext.eq_ignore_ascii_case("flac")
            || ext.eq_ignore_ascii_case("m4a")
            || ext.eq_ignore_ascii_case("txt")
        {
            return &s[..pos];
        }
    }
    s
}

/// Поиск файла аудио-сэмпла голоса (.wav, .mp3, .ogg, .flac, .m4a) в библиотеке voices/.
///
/// Алгоритм:
/// 1. Проверяет папку пользовательских кастов: voices/cast/<name>.<ext>
/// 2. Проверяет корень библиотеки: voices/<name>.<ext>
/// 3. Если указан относительный путь с подпапкой: voices/<subfolder>/<stem>.<ext>
/// 4. Если прямой путь не найден — выполняет рекурсивный поиск по всем подпапкам voices/,
///    сравнивая стем файла без учёта регистра.
pub fn find_voice_file(voices_dir: &Path, name: &str) -> Option<PathBuf> {
    let clean = clean_voice_query(name);
    if clean.is_empty() {
        return None;
    }

    let stem = extract_stem(&clean);
    // Извлекаем только имя файла без подпапок для сравнения по стему
    let base_stem = stem.rsplit('/').next().unwrap_or(stem);

    // 1. Проверка в voices/cast/
    for ext in ["wav", "mp3", "ogg", "flac", "m4a"] {
        let p = voices_dir.join("cast").join(format!("{base_stem}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Проверка в корне voices/
    for ext in ["wav", "mp3", "ogg", "flac", "m4a"] {
        let p = voices_dir.join(format!("{base_stem}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }

    // 3. Проверка относительного пути (если clean содержал подпапку)
    if clean.contains('/') {
        for ext in ["wav", "mp3", "ogg", "flac", "m4a"] {
            let p = voices_dir.join(format!("{stem}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        let direct = voices_dir.join(&clean);
        if direct.is_file() {
            return Some(p_is_audio(&direct));
        }
    }

    // 4. Рекурсивный поиск по всему voices_dir
    find_file_recursive(voices_dir, base_stem, &["wav", "mp3", "ogg", "flac", "m4a"])
}

/// Поиск сопутствующего текстового файла (.txt) для голоса
pub fn find_voice_txt(voices_dir: &Path, name: &str) -> Option<PathBuf> {
    let clean = clean_voice_query(name);
    if clean.is_empty() {
        return None;
    }

    let stem = extract_stem(&clean);
    let base_stem = stem.rsplit('/').next().unwrap_or(stem);

    // 1. cast/
    let p_cast = voices_dir.join("cast").join(format!("{base_stem}.txt"));
    if p_cast.is_file() {
        return Some(p_cast);
    }

    // 2. корень
    let p_root = voices_dir.join(format!("{base_stem}.txt"));
    if p_root.is_file() {
        return Some(p_root);
    }

    // 3. относительный
    if clean.contains('/') {
        let p_rel = voices_dir.join(format!("{stem}.txt"));
        if p_rel.is_file() {
            return Some(p_rel);
        }
    }

    // 4. рекурсивный
    find_file_recursive(voices_dir, base_stem, &["txt"])
}

fn p_is_audio(p: &Path) -> PathBuf {
    p.to_path_buf()
}

/// Рекурсивный поиск файла по стему и списку допустимых расширений (без учёта регистра)
fn find_file_recursive(dir: &Path, target_stem: &str, exts: &[&str]) -> Option<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return None;
    };

    let mut subdirs = Vec::new();

    for entry in rd.flatten() {
        let p = entry.path();
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if fname.starts_with('.') {
            continue;
        }

        if p.is_file() {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if stem.eq_ignore_ascii_case(target_stem) {
                        return Some(p);
                    }
                }
            }
        } else if p.is_dir() {
            subdirs.push(p);
        }
    }

    // Рекурсивно спускаемся в подкаталоги
    for sub in subdirs {
        if let Some(found) = find_file_recursive(&sub, target_stem, exts) {
            return Some(found);
        }
    }

    None
}

/// Получение списка всех доступных подкаталогов-паков в voices/ (1-й уровень вложенности)
pub fn list_voice_subfolders(voices_dir: &Path) -> Vec<String> {
    let mut subfolders = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(voices_dir) else {
        return Vec::new();
    };

    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // Игнорируем скрытые папки (.git, .cache и т.д.) и служебную cast/
        if fname.starts_with('.') || fname.eq_ignore_ascii_case("cast") {
            continue;
        }
        subfolders.insert(fname.to_string());
    }

    subfolders.into_iter().collect()
}

/// Получение списка имён голосов (стемов), с опциональной фильтрацией по подкаталогу
pub fn list_voice_names(voices_dir: &Path, subfolder: Option<&str>) -> Vec<String> {
    let (names, _, _) = list_voices_detailed(voices_dir, subfolder);
    names
}

/// Сбор аудиофайлов из каталога с относительными путями (с опциональной рекурсией)
fn scan_audio_dir(
    dir: &Path,
    voices_root: &Path,
    current_subfolder: Option<&str>,
    recursive: bool,
    out: &mut Vec<VoiceInfo>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in rd.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }

        if p.is_dir() {
            if recursive {
                // Если мы в корне voices_dir и заходим в подкаталог, запоминаем имя подпапки
                let next_sub = match current_subfolder {
                    Some(sub) => sub.to_string(),
                    None => name.to_string(),
                };
                scan_audio_dir(&p, voices_root, Some(&next_sub), true, out);
            }
        } else if p.is_file() {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            if ext == "wav" || ext == "mp3" || ext == "ogg" || ext == "flac" || ext == "m4a" {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    let rel_path = p
                        .strip_prefix(voices_root)
                        .map(|rp| rp.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| name.to_string());

                    out.push(VoiceInfo {
                        name: stem.to_string(),
                        subfolder: current_subfolder.map(|s| s.to_string()),
                        rel_path,
                    });
                }
            }
        }
    }
}

/// Возвращает тройку (список_стемов, подробные_метаданные, список_подпапок)
pub fn list_voices_detailed(
    voices_dir: &Path,
    subfolder: Option<&str>,
) -> (Vec<String>, Vec<VoiceInfo>, Vec<String>) {
    let subfolders = list_voice_subfolders(voices_dir);

    let mut detailed = Vec::new();

    let normalized = subfolder.unwrap_or("").trim();
    if normalized.eq_ignore_ascii_case("all") {
        // Рекурсивно сканируем корень и все подпапки
        scan_audio_dir(voices_dir, voices_dir, None, true, &mut detailed);
    } else if normalized.is_empty() || normalized.eq_ignore_ascii_case("root") {
        // Строго корень voices/ (без подпапок!)
        scan_audio_dir(voices_dir, voices_dir, None, false, &mut detailed);
    } else {
        // Только конкретный выбранный подкаталог
        let clean_sub = normalized.replace('\\', "/");
        let p = voices_dir.join(&clean_sub);
        if p.is_dir() {
            scan_audio_dir(&p, voices_dir, Some(&clean_sub), true, &mut detailed);
        }
    }

    // Дедупликация и сортировка уникальных имён (стемов)
    let mut names_set = BTreeSet::new();
    for v in &detailed {
        names_set.insert(v.name.clone());
    }
    let names: Vec<String> = names_set.into_iter().collect();

    (names, detailed, subfolders)
}
