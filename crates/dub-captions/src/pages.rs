//! Раскладка реплики на экраны субтитров по Netflix Timed Text Style Guide: до 42 символов в строке,
//! событие не дольше 7 с и не короче 5/6 с. Реплика дубляжа бывает до 8 с: длинный экран делится по
//! словам (по услышанным временам слов, если они есть). Сами реплики проекта не меняются — только то,
//! как их текст показан.

use std::ops::Range;

use crate::font;

/// Символов в строке (Netflix: 42 для латиницы и кириллицы).
pub const MAX_LINE_CHARS: usize = 42;
/// Самое долгое событие субтитра, сек.
pub const MAX_EVENT_SECS: f64 = 7.0;
/// Самое короткое событие субтитра, сек (20 кадров при 24 fps).
pub const MIN_EVENT_SECS: f64 = 5.0 / 6.0;

/// Экран субтитра: строки, время на экране и слова реплики, которые он показывает.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub lines: Vec<String>,
    pub a: f64,
    pub b: f64,
    pub words: Range<usize>,
}

/// Экраны текста `text` на [st, en]: строки по `max_chars`, по `max_lines` строк на экран. С временами
/// слов (`timed` — начало и конец каждого слова `text`) экран сменяется, когда голос доходит до его
/// первого слова, без них экраны делят время поровну. Экран дольше [`MAX_EVENT_SECS`] делится по словам
/// ближе к середине своего времени, если обе части не короче [`MIN_EVENT_SECS`].
pub fn plan(text: &str, max_chars: usize, max_lines: usize, st: f64, en: f64, timed: Option<&[(f64, f64)]>) -> Vec<Page> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let chunks = font::wrap_chars(text, max_chars);
    let per_page = max_lines.max(1);
    let groups: Vec<Vec<String>> = chunks.chunks(per_page).map(<[String]>::to_vec).collect();
    let mut ranges: Vec<Range<usize>> = Vec::with_capacity(groups.len());
    let mut w0 = 0usize;
    for g in &groups {
        let n: usize = g.iter().map(|l| l.split_whitespace().count()).sum();
        ranges.push(w0..w0 + n);
        w0 += n;
    }
    let count = groups.len();
    let per = (en - st) / count as f64;
    let mut out = Vec::with_capacity(count);
    for (gi, lines) in groups.into_iter().enumerate() {
        let (a, b) = match timed {
            Some(t) => {
                let a = if gi == 0 { st } else { t.get(ranges[gi].start).map(|w| w.0).unwrap_or(st) };
                let b = if gi == count - 1 { en } else { t.get(ranges[gi + 1].start).map(|w| w.0).unwrap_or(en) };
                (a, b.max(a))
            }
            None => {
                let a = st + gi as f64 * per;
                let b = if gi == count - 1 { en } else { st + (gi as f64 + 1.0) * per };
                (a, b)
            }
        };
        let page = Page { lines, a, b, words: ranges[gi].clone() };
        split_long(page, &words, timed, max_chars, &mut out);
    }
    out
}

/// Начало каждого слова экрана: услышанное или по длине слов внутри его времени.
fn word_starts(page: &Page, words: &[&str], timed: Option<&[(f64, f64)]>) -> Vec<f64> {
    if let Some(t) = timed.filter(|t| t.len() >= page.words.end) {
        return page.words.clone().map(|i| t[i].0).collect();
    }
    let lens: Vec<usize> = page.words.clone().map(|i| words[i].chars().count().max(1)).collect();
    let total = lens.iter().sum::<usize>().max(1) as f64;
    let mut used = 0usize;
    lens.iter()
        .map(|len| {
            let at = page.a + (page.b - page.a) * used as f64 / total;
            used += len;
            at
        })
        .collect()
}

fn split_long(page: Page, words: &[&str], timed: Option<&[(f64, f64)]>, max_chars: usize, out: &mut Vec<Page>) {
    if page.b - page.a <= MAX_EVENT_SECS || page.words.len() < 2 {
        out.push(page);
        return;
    }
    let starts = word_starts(&page, words, timed);
    let mid = (page.a + page.b) / 2.0;
    let cut = (1..starts.len())
        .filter(|&k| starts[k] - page.a >= MIN_EVENT_SECS && page.b - starts[k] >= MIN_EVENT_SECS)
        .min_by(|&x, &y| (starts[x] - mid).abs().total_cmp(&(starts[y] - mid).abs()));
    let Some(k) = cut else {
        out.push(page);
        return;
    };
    let at = page.words.start + k;
    let lines_of = |r: Range<usize>| font::wrap_chars(&words[r].join(" "), max_chars);
    let left = Page { lines: lines_of(page.words.start..at), a: page.a, b: starts[k], words: page.words.start..at };
    let right = Page { lines: lines_of(at..page.words.end), a: starts[k], b: page.b, words: at..page.words.end };
    split_long(left, words, timed, max_chars, out);
    split_long(right, words, timed, max_chars, out);
}

/// Вторая строка (другой язык) по экранам основной: слово второй строки попадает на экран, где в той же
/// доле текста (по длине) стоят слова основной. Экран без слов второй строки — пустой Vec. На экране не
/// больше `max_lines` строк второго языка: длинный текст переносится шире, кегль под ширину кадра ужимает
/// раскладка.
pub fn share(primary: &str, pages: &[Page], secondary: &str, max_chars: usize, max_lines: usize) -> Vec<Vec<String>> {
    let weight = |w: &str| w.chars().count() as f64 + 1.0;
    let prim: Vec<f64> = primary.split_whitespace().map(weight).collect();
    let prim_total: f64 = prim.iter().sum::<f64>().max(1.0);
    let bounds: Vec<f64> = pages.iter().map(|p| prim[..p.words.start.min(prim.len())].iter().sum::<f64>() / prim_total).collect();
    let sec: Vec<&str> = secondary.split_whitespace().collect();
    let sec_total: f64 = sec.iter().map(|w| weight(w)).sum::<f64>().max(1.0);
    let mut per_page: Vec<Vec<&str>> = vec![Vec::new(); pages.len()];
    let mut before = 0.0;
    for w in sec {
        let mid = (before + weight(w) / 2.0) / sec_total;
        before += weight(w);
        if let Some(k) = bounds.iter().rposition(|&f| f <= mid) {
            per_page[k].push(w);
        }
    }
    per_page
        .into_iter()
        .map(|ws| if ws.is_empty() { Vec::new() } else { wrap_limited(&ws.join(" "), max_chars, max_lines) })
        .collect()
}

/// Перенос не больше чем в `max_lines` строк: ширина строки растёт, пока текст не уложится.
fn wrap_limited(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let total = text.chars().count();
    let mut width = max_chars.max(1);
    loop {
        let lines = font::wrap_chars(text, width);
        if lines.len() <= max_lines.max(1) || width >= total {
            return lines;
        }
        width += (width / 5).max(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(n: usize) -> String {
        (0..n).map(|i| format!("w{i:02}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn a_line_never_holds_more_than_42_characters() {
        let text = "This line of dubbed dialogue is quite a lot longer than forty two characters in total";
        let pages = plan(text, MAX_LINE_CHARS, 2, 0.0, 5.0, None);
        assert_eq!(pages.len(), 2, "87 символов — три строки, два экрана по две: {pages:?}");
        assert!(pages.iter().all(|p| p.lines.len() <= 2));
        assert!(pages.iter().flat_map(|p| &p.lines).all(|l| l.chars().count() <= MAX_LINE_CHARS), "{pages:?}");
    }

    #[test]
    fn an_eight_second_line_is_shown_as_two_events_under_seven_seconds() {
        let text = "Short words fill this line";
        let pages = plan(text, MAX_LINE_CHARS, 2, 10.0, 18.0, None);
        assert_eq!(pages.len(), 2, "{pages:?}");
        assert!(pages.iter().all(|p| p.b - p.a <= MAX_EVENT_SECS + 1e-9 && p.b - p.a >= MIN_EVENT_SECS));
        assert_eq!(pages[0].a, 10.0);
        assert_eq!(pages[1].b, 18.0);
        assert_eq!(pages[0].b, pages[1].a);
        assert_eq!(pages[0].words.end, pages[1].words.start);
        let shown: Vec<String> = pages.iter().flat_map(|p| p.lines.clone()).collect();
        assert_eq!(shown.join(" "), text);
    }

    #[test]
    fn a_long_event_is_cut_where_a_word_is_heard() {
        let text = "one two three four";
        let timed = [(0.0, 0.5), (0.6, 1.0), (5.0, 5.5), (5.6, 8.0)];
        let pages = plan(text, MAX_LINE_CHARS, 2, 0.0, 8.0, Some(&timed));
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].a, 5.0, "второе событие начинается со слова «three»");
        assert_eq!(pages[0].lines, ["one two"]);
        assert_eq!(pages[1].lines, ["three four"]);
    }

    #[test]
    fn a_split_that_would_leave_a_flash_is_not_made() {
        // Слова сказаны в самом начале: любой рез дал бы часть короче 5/6 с — экран остаётся целым.
        let timed = [(0.0, 0.1), (0.2, 0.3)];
        let pages = plan("one two", MAX_LINE_CHARS, 2, 0.0, 7.5, Some(&timed));
        assert_eq!(pages.len(), 1);
        let single = plan("word", MAX_LINE_CHARS, 2, 0.0, 7.9, None);
        assert_eq!(single.len(), 1, "одно слово не делится");
    }

    #[test]
    fn short_lines_keep_one_event() {
        let pages = plan("Hello there", MAX_LINE_CHARS, 2, 1.0, 3.0, None);
        assert_eq!(pages, vec![Page { lines: vec!["Hello there".into()], a: 1.0, b: 3.0, words: 0..2 }]);
    }

    #[test]
    fn pages_follow_max_lines() {
        let text = words(30);
        let pages = plan(&text, 12, 2, 0.0, 6.0, None);
        assert!(pages.len() >= 3);
        assert!(pages.iter().all(|p| p.lines.len() <= 2));
        assert_eq!(pages.last().unwrap().words.end, 30);
    }

    #[test]
    fn the_second_language_is_shared_by_the_same_share_of_text() {
        let primary = "Где ты был всю ночь? Я волновалась.";
        let pages = vec![
            Page { lines: vec![], a: 0.0, b: 2.0, words: 0..5 },
            Page { lines: vec![], a: 2.0, b: 4.0, words: 5..7 },
        ];
        let sec = share(primary, &pages, "Where were you all night? I was worried.", MAX_LINE_CHARS, 2);
        assert_eq!(sec.len(), 2);
        assert_eq!(sec[0], ["Where were you all night?"]);
        assert_eq!(sec[1], ["I was worried."]);
    }

    #[test]
    fn a_long_second_language_stays_within_the_lines_of_a_screen() {
        let pages = vec![Page { lines: vec![], a: 0.0, b: 3.0, words: 0..2 }];
        let german = "Wo bist du die ganze Nacht gewesen, ich habe mir furchtbare Sorgen um dich gemacht und dich überall gesucht";
        let sec = share("Где ты был", &pages, german, MAX_LINE_CHARS, 2);
        assert_eq!(sec[0].len(), 2, "{sec:?}");
        assert_eq!(sec[0].join(" "), german);
    }
}
