//! Ответ модели без её рассуждений — для облака и своего сервера (порт content_of/without_thinking студий).
//! Локальный llama-server сюда не ходит: его ответ отдаётся как есть, а <think> снимает вызывающий
//! (`strip_think`, паритет с питоном).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Ответ из message чат-комплишена. Думающие модели кладут видимый ответ в `content`, а рассуждение — в
/// `reasoning_content` (llama.cpp, Ollama) или `reasoning` (OpenRouter); у части сборок `content` приходит
/// пустым и весь ответ оказывается в поле рассуждения. Пусто везде — пустая строка.
pub(crate) fn content_of(message: &Value) -> String {
    for field in ["content", "reasoning_content", "reasoning"] {
        if let Some(text) = message.get(field).and_then(Value::as_str) {
            let answer = without_thinking(text);
            if !answer.is_empty() {
                return answer;
            }
        }
    }
    String::new()
}

/// Ответ без рассуждений, которые модель в него вписала: целые блоки, незакрытый блок до конца и — когда
/// шаблон чата сам открыл блок, а модель рассуждала простым текстом — всё до последнего закрывающего тега.
pub fn without_thinking(text: &str) -> String {
    static BLOCKS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<(think|analysis|reasoning|reflection|thought)>.*?</(think|analysis|reasoning|reflection|thought)>|<\|channel>thought.*?<channel\|>")
            .expect("thinking blocks")
    });
    static UNCLOSED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)(<(think|analysis|reasoning|reflection|thought)>|<\|channel>thought).*").expect("unclosed thinking"));
    static CLOSER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"</(think|analysis|reasoning|reflection|thought)>").expect("closing tag"));
    static PROCESS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)^(\s*\*+\s*)?(Thinking Process|Thought Process|Thinking|Reasoning):\s*.*?(---|\*{3,}|={3,})\s*")
            .expect("thinking process")
    });
    let text = BLOCKS.replace_all(text, "");
    let text = UNCLOSED.replace(&text, "");
    let text = match CLOSER.find_iter(&text).last() {
        Some(closer) => text[closer.end()..].to_string(),
        None => text.into_owned(),
    };
    PROCESS.replace(&text, "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_is_taken_out_of_the_answer() {
        assert_eq!(without_thinking("<think>plan</think>1. Привет"), "1. Привет");
        assert_eq!(without_thinking("<THINK>x</THINK>ok"), "<THINK>x</THINK>ok");
        assert_eq!(without_thinking("1. Да\n<think>unfinished"), "1. Да");
        assert_eq!(without_thinking("reasoning in plain text</think>\n1. Ответ"), "1. Ответ");
        assert_eq!(without_thinking("<|channel>thought ... <channel|>1. Hola"), "1. Hola");
        assert_eq!(without_thinking("Thinking Process: step one\n---\n1. Bonjour"), "1. Bonjour");
        assert_eq!(without_thinking("  plain  "), "plain");
    }

    #[test]
    fn an_answer_left_in_the_reasoning_field_is_read_from_there() {
        assert_eq!(content_of(&json!({ "content": "1. a" })), "1. a");
        assert_eq!(content_of(&json!({ "content": "", "reasoning_content": "1. b" })), "1. b");
        assert_eq!(content_of(&json!({ "content": null, "reasoning": "<think>x</think>1. c" })), "1. c");
        assert_eq!(content_of(&json!({ "content": "" })), "");
    }
}
