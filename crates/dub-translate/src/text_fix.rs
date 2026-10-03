//! Починка ответа перевода: латинские двойники букв внутри кириллических слов («Tут», «oстов») — TTS на них
//! спотыкается. Порт cyrillic_homoglyphs студий.

/// Языки с кириллическим письмом (для них и чиним).
pub fn cyrillic_target(lang: &str) -> bool {
    let code = lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase();
    matches!(code.as_str(), "ru" | "uk" | "be" | "bg" | "sr" | "mk" | "kk" | "ky" | "tg" | "mn" | "ab" | "os" | "ba" | "tt")
}

/// Кириллическое слово с латинскими двойниками внутри. Меняются только буквы, одинаковые и на вид, и на
/// звук: латинская H бывает и Н, и Х — её оставляем. Слова без кириллицы (бренды, «iPhone») не трогаются.
pub fn cyrillic_homoglyphs(text: &str) -> String {
    let swap = |c: char| match c {
        'A' => 'А', 'C' => 'С', 'E' => 'Е', 'K' => 'К', 'M' => 'М', 'O' => 'О', 'P' => 'Р', 'T' => 'Т', 'X' => 'Х',
        'a' => 'а', 'c' => 'с', 'e' => 'е', 'o' => 'о', 'p' => 'р', 'x' => 'х', 'y' => 'у',
        other => other,
    };
    let cyrillic = |c: char| ('\u{0400}'..='\u{04FF}').contains(&c);
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.chars().any(cyrillic) && word.chars().any(|c| c.is_ascii_alphabetic()) {
            out.extend(word.chars().map(swap));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphabetic() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Починить перевод для целевого языка `lang`: для кириллических языков — двойники; для прочих — как есть.
pub fn fix_translation(text: &str, lang: &str) -> String {
    if cyrillic_target(lang) {
        cyrillic_homoglyphs(text)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_look_alikes_inside_cyrillic_words_become_cyrillic() {
        assert_eq!(cyrillic_homoglyphs("Tут"), "Тут");
        assert_eq!(cyrillic_homoglyphs("oстов"), "остов");
        assert_eq!(cyrillic_homoglyphs("Я пoшёл дoмoй, Kатя."), "Я пошёл домой, Катя.");
        let fixed = cyrillic_homoglyphs("Tут");
        assert!(fixed.chars().all(|c| ('\u{0400}'..='\u{04FF}').contains(&c)), "{fixed:?}");
    }

    #[test]
    fn latin_words_and_ambiguous_letters_are_left_alone() {
        assert_eq!(cyrillic_homoglyphs("CLAUDE"), "CLAUDE");
        assert_eq!(cyrillic_homoglyphs("Купил iPhone и MacBook"), "Купил iPhone и MacBook");
        assert_eq!(cyrillic_homoglyphs("Hет"), "Hет", "H is both Н and Х");
        assert_eq!(cyrillic_homoglyphs("OK, 42%"), "OK, 42%");
    }

    #[test]
    fn only_cyrillic_targets_are_fixed() {
        assert_eq!(fix_translation("Tут", "ru"), "Тут");
        assert_eq!(fix_translation("Tут", "uk-UA"), "Тут");
        assert_eq!(fix_translation("Tут", "en"), "Tут");
        assert!(cyrillic_target("bg") && !cyrillic_target("pl"));
    }
}
