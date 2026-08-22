//! Импорт готовых субтитров (SRT / SSA/ASS) как основы вместо ASR.
//! Даёт точный текст + тайминг; спикеров назначает analyze по диаризации (overlap).
//! Возвращаем реплики (start, end, text) во временном порядке; пустые/битые блоки пропускаем.

/// Одна реплика субтитров: время в секундах + текст (уже без разметки) + опциональный спикер/актёр.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub speaker: Option<String>,
    pub text: String,
}

/// Распарсить содержимое файла субтитров. Формат — по расширению (srt | ass | ssa).
/// Неизвестное расширение пробуем как SRT (самый частый), при неудаче — как ASS.
pub fn parse(content: &str, ext: &str) -> Vec<Cue> {
    let e = ext.to_ascii_lowercase();
    let cues = match e.as_str() {
        "ass" | "ssa" => parse_ass(content),
        _ => parse_srt(content),
    };
    // Fallback: SRT-парсер ничего не дал, но текст похож на ASS.
    if cues.is_empty() && e != "ass" && e != "ssa" && content.contains("[Events]") {
        return finalize(parse_ass(content));
    }
    finalize(cues)
}

/// Отсортировать по времени, отфильтровать пустые/нулевые. Общий хвост для обоих форматов.
fn finalize(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.retain(|c| c.end > c.start && !c.text.trim().is_empty());
    cues.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    cues
}

/// Извлечь тег спикера в начале текста, например:
/// "[Геральт]: Привет" -> (Some("Геральт"), "Привет")
/// "[Лютик] Привет" -> (Some("Лютик"), "Привет")
/// "Йеннифэр: Привет" -> (Some("Йеннифэр"), "Привет")
fn extract_speaker_tag(raw: &str) -> (Option<String>, String) {
    let t = raw.trim();
    if t.starts_with('[') {
        if let Some(close_idx) = t.find(']') {
            let inside = t[1..close_idx].trim();
            let after = t[close_idx + 1..].trim();
            let after = after.strip_prefix(':').map(str::trim).unwrap_or(after);
            if !inside.is_empty() && !after.is_empty() && !inside.contains('\n') {
                return (Some(inside.to_string()), after.to_string());
            }
        }
    } else if let Some((spk, rest)) = t.split_once(':') {
        let spk = spk.trim();
        let rest = rest.trim();
        if !spk.is_empty()
            && spk.chars().count() <= 50
            && !spk.contains("http")
            && !spk.contains('/')
            && !spk.contains('\n')
            && !rest.is_empty()
            && !spk.chars().all(|c| c.is_ascii_digit())
        {
            return (Some(spk.to_string()), rest.to_string());
        }
    }
    (None, t.to_string())
}

// ─── SRT ─────────────────────────────────────────────────────────────────────
// Блок: [индекс] / "HH:MM:SS,mmm --> HH:MM:SS,mmm[ настройки]" / текст(ы).
// BOM и \r терпим; индекс необязателен (некоторые редакторы его опускают).
fn parse_srt(content: &str) -> Vec<Cue> {
    let mut out = Vec::new();
    let text = content.trim_start_matches('\u{feff}').replace('\r', "");
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        // Строка тайминга — первая, содержащая "-->" (пропускаем ведущий числовой индекс).
        let ti = match lines.iter().position(|l| l.contains("-->")) {
            Some(i) => i,
            None => continue,
        };
        let (start, end) = match parse_time_range(lines[ti], srt_ts) {
            Some(v) => v,
            None => continue,
        };
        let body = lines[ti + 1..].join(" ").trim().to_string();
        if body.is_empty() {
            continue;
        }
        let clean = strip_markup(&body);
        let (speaker, text) = extract_speaker_tag(&clean);
        if text.is_empty() {
            continue;
        }
        out.push(Cue { start, end, speaker, text });
    }
    out
}

/// "HH:MM:SS,mmm" -> секунды. Терпим '.' вместо ',' (иногда встречается).
fn srt_ts(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let (hms, frac) = match s.split_once('.') {
        Some((a, b)) => (a, b),
        None => (s.as_str(), "0"),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    let ms = format!("0.{frac}").parse::<f64>().unwrap_or(0.0);
    Some(h * 3600.0 + m * 60.0 + sec + ms)
}

// ─── SSA / ASS ───────────────────────────────────────────────────────────────
// Секция [Events]; строка Format: задаёт порядок полей; строки Dialogue: — реплики.
// Текст — ПОСЛЕДНЕЕ поле (может содержать запятые), поэтому режем на N-1 частей.
fn parse_ass(content: &str) -> Vec<Cue> {
    let text = content.trim_start_matches('\u{feff}').replace('\r', "");
    let mut in_events = false;
    let mut fmt: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let lt = line.trim();
        if lt.starts_with('[') && lt.ends_with(']') {
            in_events = lt.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(rest) = lt.strip_prefix("Format:") {
            fmt = rest.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
            continue;
        }
        let rest = match lt.strip_prefix("Dialogue:") {
            Some(r) => r,
            None => continue,
        };
        // Дефолтный порядок ASS, если Format не встретился.
        if fmt.is_empty() {
            fmt = ["layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        }
        let i_start = fmt.iter().position(|f| f == "start");
        let i_end = fmt.iter().position(|f| f == "end");
        let i_name = fmt.iter().position(|f| f == "name" || f == "actor");
        let i_text = fmt.iter().position(|f| f == "text").unwrap_or(fmt.len() - 1);
        // splitn на кол-во полей: последнее поле (text) забирает весь хвост с запятыми.
        let cols: Vec<&str> = rest.splitn(fmt.len(), ',').collect();
        let get = |idx: Option<usize>| idx.and_then(|i| cols.get(i)).map(|s| s.trim());
        let (s, e) = match (get(i_start), get(i_end)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let (start, end) = match (ass_ts(s), ass_ts(e)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let raw_name = get(i_name).unwrap_or("");
        let mut speaker = if !raw_name.is_empty()
            && !raw_name.eq_ignore_ascii_case("default")
            && !raw_name.eq_ignore_ascii_case("*default")
        {
            Some(raw_name.to_string())
        } else {
            None
        };
        let body = cols.get(i_text).copied().unwrap_or("");
        let clean = strip_markup(body);
        let (tag_spk, text) = if speaker.is_none() {
            extract_speaker_tag(&clean)
        } else {
            (None, clean)
        };
        if speaker.is_none() {
            speaker = tag_spk;
        }
        if text.is_empty() {
            continue;
        }
        out.push(Cue { start, end, speaker, text });
    }
    out
}

/// "H:MM:SS.cc" (ASS) -> секунды. Сотые доли (2 знака).
fn ass_ts(s: &str) -> Option<f64> {
    srt_ts(s) // тот же разбор H:M:S.frac; ',' уже не встречается, '.' поддержан
}

/// "HH:MM:SS --> HH:MM:SS" -> (начало, конец). Хвост после времени конца (SRT-настройки) отбрасываем.
fn parse_time_range(line: &str, ts: fn(&str) -> Option<f64>) -> Option<(f64, f64)> {
    let (a, b) = line.split_once("-->")?;
    let b = b.split_whitespace().next().unwrap_or(b.trim());
    Some((ts(a)?, ts(b)?))
}

/// Убрать разметку: ASS override-теги {\...}, переводы строк \N \n \h, HTML-теги <i>/<b>.
/// Схлопнуть пробелы. Порт того, что делает большинство саб-парсеров перед показом текста.
fn strip_markup(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                // пропустить до закрывающей '}' (ASS override block)
                for n in chars.by_ref() {
                    if n == '}' {
                        break;
                    }
                }
            }
            '\\' => {
                // \N \n -> перевод строки; \h -> пробел; иначе игнор символа
                match chars.peek() {
                    Some('N') | Some('n') => {
                        chars.next();
                        r.push(' ');
                    }
                    Some('h') => {
                        chars.next();
                        r.push(' ');
                    }
                    _ => {}
                }
            }
            '<' => {
                // HTML-тег <i>,<b>,<font ...> — пропустить до '>'
                let mut closed = false;
                for n in chars.by_ref() {
                    if n == '>' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    r.push('<');
                }
            }
            _ => r.push(c),
        }
    }
    r.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_basic() {
        let s = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n2\n00:00:05,500 --> 00:00:07,000\nSecond line";
        let c = parse(s, "srt");
        assert_eq!(c.len(), 2);
        assert!((c[0].start - 1.0).abs() < 1e-6);
        assert!((c[0].end - 4.0).abs() < 1e-6);
        assert_eq!(c[0].text, "Hello world");
        assert!((c[1].start - 5.5).abs() < 1e-6);
    }

    #[test]
    fn srt_multiline_and_tags() {
        let s = "1\n00:00:01,000 --> 00:00:02,000\n<i>Line one</i>\nLine two";
        let c = parse(s, "srt");
        assert_eq!(c[0].text, "Line one Line two");
    }

    #[test]
    fn ass_basic() {
        let s = "[Script Info]\nTitle: x\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,Hello, there {\\i1}world{\\i0}\n";
        let c = parse(s, "ass");
        assert_eq!(c.len(), 1);
        assert!((c[0].start - 1.0).abs() < 1e-6);
        assert!((c[0].end - 3.5).abs() < 1e-6);
        assert_eq!(c[0].text, "Hello, there world"); // запятая в тексте сохранена, теги убраны
        assert_eq!(c[0].speaker, None);
    }

    #[test]
    fn ass_with_actor_name() {
        let s = "[Script Info]\nTitle: x\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.50,Default,Геральт,0,0,0,,Привет, Лютик!\nDialogue: 0,0:00:04.00,0:00:06.00,Italics,Лютик,0,0,0,,Привет, друг!\n";
        let c = parse(s, "ass");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].speaker.as_deref(), Some("Геральт"));
        assert_eq!(c[0].text, "Привет, Лютик!");
        assert_eq!(c[1].speaker.as_deref(), Some("Лютик"));
        assert_eq!(c[1].text, "Привет, друг!");
    }

    #[test]
    fn srt_with_speaker_tags() {
        let s = "1\n00:00:01,000 --> 00:00:04,000\n[Геральт]: Привет, Лютик!\n\n2\n00:00:05,000 --> 00:00:08,000\nЛютик: Здравствуй!";
        let c = parse(s, "srt");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].speaker.as_deref(), Some("Геральт"));
        assert_eq!(c[0].text, "Привет, Лютик!");
        assert_eq!(c[1].speaker.as_deref(), Some("Лютик"));
        assert_eq!(c[1].text, "Здравствуй!");
    }

    #[test]
    fn ass_newline() {
        let s = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,A\\NB\n";
        let c = parse(s, "ass");
        assert_eq!(c[0].text, "A B");
    }

    #[test]
    fn skips_empty_and_bad() {
        let s = "1\n00:00:01,000 --> 00:00:01,000\nzero-length\n\n2\ngarbage\n\n3\n00:00:02,000 --> 00:00:04,000\nok";
        let c = parse(s, "srt");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "ok");
    }
}
