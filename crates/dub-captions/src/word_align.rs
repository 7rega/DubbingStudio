//! Пословные тайминги субтитра по реальной речи. Внутристрочная часть align_lyrics_words студий
//! (YuE2-Studio lyrics_sync.rs): каждое написанное слово берёт время услышанного — по совпадению
//! нормализованного слова или по префиксу-основе для слов длиннее 3 символов (посимвольно, безопасно
//! для кириллицы); курсор по услышанному идёт только вперёд. Несопоставленные слова раскладываются по
//! длине в промежутке между соседями. Адаптация: у услышанного слова есть и конец (ASR даёт start/end),
//! а результат — непрерывная раскладка [a, b] по словам, как у равномерной word_spans.

/// Меньше этой доли сопоставленных слов — это не та же речь (перевод поверх оригинала, другой язык):
/// случайные совпадения имён и чисел не должны тянуть слова, раскладка остаётся по длине.
const MIN_MATCHED: f64 = 0.5;

fn normalise(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A word without its last character, for tolerating inflected endings.
fn stem(word: &str) -> String {
    let count = word.chars().count();
    word.chars().take(count.saturating_sub(1)).collect()
}

/// Начало звучания каждого написанного слова на отрезке [a, b]. `heard` — услышанные слова (текст,
/// начало, конец) в тех же секундах. Возвращает (начало, конец) на каждое слово `written`: начала
/// неубывают, конец слова = начало следующего, последнее кончается в `b`; первое слово может начаться
/// позже `a` (пауза до первого слова).
pub fn align_words(written: &[&str], heard: &[(String, f64, f64)], a: f64, b: f64) -> Vec<(f64, f64)> {
    let n = written.len();
    if n == 0 {
        return Vec::new();
    }
    let b = b.max(a);
    let inside: Vec<(String, f64, f64)> = heard
        .iter()
        .filter(|(_, s, e)| *e > a - 0.05 && *s < b)
        .map(|(w, s, e)| (normalise(w), s.clamp(a, b), e.clamp(a, b)))
        .filter(|(w, _, _)| !w.is_empty())
        .collect();
    let mut placed: Vec<Option<(f64, f64)>> = vec![None; n];
    let mut cursor = 0usize;
    for (position, word) in written.iter().enumerate() {
        let expected = normalise(word);
        if expected.is_empty() {
            continue;
        }
        if let Some(found) = inside[cursor..].iter().position(|(heard_word, _, _)| {
            heard_word == &expected || (expected.chars().count() > 3 && heard_word.starts_with(&stem(&expected)))
        }) {
            let (_, s, e) = &inside[cursor + found];
            placed[position] = Some((*s, e.max(*s)));
            cursor += found + 1;
        }
    }
    let matched = placed.iter().filter(|p| p.is_some()).count();
    if (matched as f64) < MIN_MATCHED * n as f64 {
        placed = vec![None; n];
    }

    // Несопоставленные — по длине в своём промежутке: от конца предыдущего услышанного слова до
    // начала следующего сопоставленного (или до b).
    let mut starts: Vec<f64> = vec![a; n];
    let mut previous_time = a;
    let mut position = 0usize;
    while position < n {
        if let Some((at, end)) = placed[position] {
            starts[position] = at.max(previous_time);
            previous_time = end.max(starts[position]);
            position += 1;
            continue;
        }
        let gap_start = position;
        while position < n && placed[position].is_none() {
            position += 1;
        }
        let next_time = placed.get(position).copied().flatten().map(|p| p.0).unwrap_or(b).max(previous_time);
        let span = next_time - previous_time;
        let weight: usize = written[gap_start..position].iter().map(|w| w.chars().count().max(1)).sum();
        let mut used = 0usize;
        for offset in gap_start..position {
            starts[offset] = previous_time + span * used as f64 / weight as f64;
            used += written[offset].chars().count().max(1);
        }
        previous_time = next_time;
    }
    (0..n)
        .map(|i| {
            let end = if i + 1 < n { starts[i + 1] } else { b };
            (starts[i], end.max(starts[i]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(w: &str, s: f64, e: f64) -> (String, f64, f64) {
        (w.to_string(), s, e)
    }

    #[test]
    fn every_word_gets_its_own_time() {
        let heard = vec![h("neon", 1.0, 1.3), h("arms", 1.4, 1.7), h("the", 1.8, 2.0), h("glass", 2.2, 2.7)];
        let t = align_words(&["Neon", "on", "the", "glass"], &heard, 0.8, 3.0);
        assert_eq!(t[0].0, 1.0);
        assert_eq!(t[2].0, 1.8);
        assert_eq!(t[3].0, 2.2);
        // «on» не услышано — ложится между соседями, а не на начало строки.
        assert!(t[1].0 > 1.0 && t[1].0 < 1.8, "on: {:?}", t[1]);
        assert_eq!(t[3].1, 3.0, "последнее слово тянется до конца экрана");
        assert_eq!(t[0].1, t[1].0, "раскладка непрерывна");
    }

    #[test]
    fn russian_words_align_without_slicing_a_letter_in_half() {
        let heard = vec![h("неон", 1.0, 1.4), h("дрожит", 1.5, 1.9), h("на", 2.0, 2.2), h("коже", 2.4, 2.8)];
        let t = align_words(&["Неон", "дрожит", "на", "мокрой", "коже."], &heard, 1.0, 3.0);
        assert_eq!(t.len(), 5);
        assert_eq!(t[1].0, 1.5);
        assert!(t[3].0 > 2.0 && t[3].0 < 2.4, "мокрой: {:?}", t[3]);
        assert_eq!(t[4].0, 2.4);
    }

    #[test]
    fn inflected_endings_still_match_by_stem() {
        let heard = vec![h("поехали", 0.2, 0.7), h("домой", 0.9, 1.3)];
        let t = align_words(&["Поехала", "домой!"], &heard, 0.0, 1.5);
        assert_eq!(t[0].0, 0.2);
        assert_eq!(t[1].0, 0.9);
    }

    #[test]
    fn a_pause_before_the_first_word_is_kept() {
        let heard = vec![h("hello", 0.6, 0.9), h("world", 1.0, 1.4)];
        let t = align_words(&["Hello", "world"], &heard, 0.0, 1.5);
        assert_eq!(t[0].0, 0.6);
    }

    #[test]
    fn other_speech_falls_back_to_length_spread() {
        // Перевод поверх оригинала: совпало только имя — это не та же речь.
        let heard = vec![h("hello", 0.0, 0.4), h("anna", 0.5, 0.9), h("how", 1.0, 1.2), h("are", 1.3, 1.5)];
        let written = ["Hola,", "Anna,", "¿cómo", "estás?"];
        let t = align_words(&written, &heard, 0.0, 2.0);
        let total: usize = written.iter().map(|w| w.chars().count()).sum();
        assert!((t[1].0 - 2.0 * 5.0 / total as f64).abs() < 1e-9, "{t:?}");
    }

    #[test]
    fn words_outside_the_screen_are_ignored_and_times_stay_inside() {
        let heard = vec![h("before", 0.0, 0.5), h("one", 2.1, 2.3), h("two", 2.5, 2.8), h("after", 9.0, 9.5)];
        let t = align_words(&["one", "two"], &heard, 2.0, 3.0);
        assert_eq!(t, vec![(2.1, 2.5), (2.5, 3.0)]);
    }
}
