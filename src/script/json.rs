// ============================================================
//  script/json.rs  —  JSON parsing and serialising
// ============================================================
//
//  One implementation shared by `JSON.parse`, `JSON.stringify` and
//  `response.json()`, so a fetched document and a literal string are read the
//  same way.
//
//  Parsing reports *why* it failed rather than returning `undefined`: that
//  message becomes the `SyntaxError` a `try`/`catch` sees, and the rejection
//  reason a `.catch()` on `response.json()` receives.

use std::cell::RefCell;
use std::rc::Rc;

use super::interp::{number_to_string, JsValue};

/// How deeply nested a document may be before it is refused.
///
/// Parsing is recursive, so this is what stands between a hostile document and
/// a blown Rust stack.
const MAX_DEPTH: usize = 64;

/// Parse JSON text into a runtime value.
pub fn parse(input: &str) -> Result<JsValue, String> {
    let mut parser = JsonParser {
        chars: input.chars().collect(),
        position: 0,
        depth: 0,
    };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.position < parser.chars.len() {
        return Err(format!(
            "SyntaxError: unexpected token in JSON at position {}",
            parser.position
        ));
    }
    Ok(value)
}

/// Serialise a runtime value as JSON.
///
/// Values with no JSON representation — a function, a DOM handle, a promise —
/// become `null`, which is what `JSON.stringify` does with them.
pub fn stringify(value: &JsValue) -> String {
    match value {
        JsValue::Undefined | JsValue::Null => "null".to_string(),
        JsValue::Bool(b) => b.to_string(),
        JsValue::Number(n) if n.is_finite() => number_to_string(*n),
        // Infinity and NaN have no JSON form.
        JsValue::Number(_) => "null".to_string(),
        JsValue::Str(s) => quote(s),
        JsValue::Array(items) => {
            let elements: Vec<String> = items.borrow().iter().map(stringify).collect();
            format!("[{}]", elements.join(","))
        }
        JsValue::Object(props) => {
            let fields: Vec<String> = props
                .borrow()
                .iter()
                .map(|(key, value)| format!("{}:{}", quote(key), stringify(value)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        _ => "null".to_string(),
    }
}

/// Write a JSON string literal, escaping what has to be escaped.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct JsonParser {
    chars: Vec<char>,
    position: usize,
    depth: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.position += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.position += 1;
        }
    }

    fn error(&self, what: &str) -> String {
        format!("SyntaxError: {what} in JSON at position {}", self.position)
    }

    /// Consume `word` if it is next.
    fn eat_word(&mut self, word: &str) -> bool {
        let end = self.position + word.chars().count();
        if end <= self.chars.len()
            && self.chars[self.position..end]
                .iter()
                .copied()
                .eq(word.chars())
        {
            self.position = end;
            return true;
        }
        false
    }

    fn parse_value(&mut self) -> Result<JsValue, String> {
        if self.depth >= MAX_DEPTH {
            return Err("SyntaxError: JSON is nested too deeply".to_string());
        }
        self.skip_whitespace();
        match self.peek() {
            None => Err(self.error("unexpected end of input")),
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsValue::Str),
            Some('t') if self.eat_word("true") => Ok(JsValue::Bool(true)),
            Some('f') if self.eat_word("false") => Ok(JsValue::Bool(false)),
            Some('n') if self.eat_word("null") => Ok(JsValue::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(self.error(&format!("unexpected token {c:?}"))),
        }
    }

    fn parse_object(&mut self) -> Result<JsValue, String> {
        self.position += 1; // '{'
        self.depth += 1;
        let mut props: Vec<(String, JsValue)> = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.position += 1;
            self.depth -= 1;
            return Ok(JsValue::Object(Rc::new(RefCell::new(props))));
        }

        loop {
            self.skip_whitespace();
            if self.peek() != Some('"') {
                return Err(self.error("expected a property name"));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.next() != Some(':') {
                return Err(self.error("expected ':' after a property name"));
            }
            let value = self.parse_value()?;
            // A repeated key keeps the last value, as JavaScript does.
            props.retain(|(existing, _)| existing != &key);
            props.push((key, value));

            self.skip_whitespace();
            match self.next() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
        self.depth -= 1;
        Ok(JsValue::Object(Rc::new(RefCell::new(props))))
    }

    fn parse_array(&mut self) -> Result<JsValue, String> {
        self.position += 1; // '['
        self.depth += 1;
        let mut items: Vec<JsValue> = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.position += 1;
            self.depth -= 1;
            return Ok(JsValue::Array(Rc::new(RefCell::new(items))));
        }

        loop {
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.next() {
                Some(',') => continue,
                Some(']') => break,
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
        self.depth -= 1;
        Ok(JsValue::Array(Rc::new(RefCell::new(items))))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.position += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.next() {
                None => return Err(self.error("unterminated string")),
                Some('"') => return Ok(out),
                Some('\\') => match self.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('b') => out.push('\u{08}'),
                    Some('f') => out.push('\u{0C}'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('u') => out.push(self.parse_unicode_escape()?),
                    _ => return Err(self.error("invalid escape sequence")),
                },
                // A raw control character is not allowed in a JSON string.
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error("unescaped control character"))
                }
                Some(c) => out.push(c),
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let end = self.position + 4;
        if end > self.chars.len() {
            return Err(self.error("truncated \\u escape"));
        }
        let hex: String = self.chars[self.position..end].iter().collect();
        self.position = end;
        let code = u32::from_str_radix(&hex, 16).map_err(|_| self.error("invalid \\u escape"))?;
        // Surrogate halves have no scalar value of their own; a lone one
        // becomes the replacement character rather than failing the parse.
        char::from_u32(code).ok_or_else(|| self.error("invalid \\u escape"))
    }

    fn parse_number(&mut self) -> Result<JsValue, String> {
        let start = self.position;
        if self.peek() == Some('-') {
            self.position += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.position += 1;
        }
        if self.peek() == Some('.') {
            self.position += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.position += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.position += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.position += 1;
            }
        }
        let text: String = self.chars[start..self.position].iter().collect();
        text.parse::<f32>()
            .map(JsValue::Number)
            .map_err(|_| self.error(&format!("invalid number {text:?}")))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::interp::to_string;

    fn field(value: &JsValue, name: &str) -> JsValue {
        match value {
            JsValue::Object(props) => props
                .borrow()
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or(JsValue::Undefined),
            _ => panic!("not an object: {value:?}"),
        }
    }

    #[test]
    fn parses_the_primitives() {
        assert!(matches!(parse("true"), Ok(JsValue::Bool(true))));
        assert!(matches!(parse("false"), Ok(JsValue::Bool(false))));
        assert!(matches!(parse("null"), Ok(JsValue::Null)));
        assert!(matches!(parse(" 42 "), Ok(JsValue::Number(n)) if n == 42.0));
        assert!(matches!(parse("-1.5e2"), Ok(JsValue::Number(n)) if n == -150.0));
        assert_eq!(to_string(&parse(r#""hi""#).unwrap()), "hi");
    }

    #[test]
    fn parses_an_object() {
        let value = parse(r#"{"message":"hello","count":3,"ok":true}"#).expect("parsed");
        assert_eq!(to_string(&field(&value, "message")), "hello");
        assert_eq!(to_string(&field(&value, "count")), "3");
        assert_eq!(to_string(&field(&value, "ok")), "true");
    }

    #[test]
    fn parses_nested_arrays_and_objects() {
        let value = parse(r#"{"items":[{"id":1},{"id":2}]}"#).expect("parsed");
        let JsValue::Array(items) = field(&value, "items") else {
            panic!("expected an array");
        };
        assert_eq!(items.borrow().len(), 2);
        assert_eq!(to_string(&field(&items.borrow()[1], "id")), "2");
    }

    #[test]
    fn parses_empty_containers() {
        assert!(matches!(parse("{}"), Ok(JsValue::Object(_))));
        assert!(matches!(parse("[]"), Ok(JsValue::Array(_))));
        let JsValue::Array(items) = parse("[ ]").unwrap() else {
            panic!()
        };
        assert!(items.borrow().is_empty());
    }

    #[test]
    fn handles_escapes_in_strings() {
        assert_eq!(
            to_string(&parse(r#""a\"b\\c\ndA""#).unwrap()),
            "a\"b\\c\ndA"
        );
    }

    #[test]
    fn rejects_malformed_documents_with_a_reason() {
        for bad in [
            "{",
            "{\"a\"}",
            "{\"a\":}",
            "[1,]",
            "[1 2]",
            "nul",
            "\"unterminated",
            "{} extra",
            "",
        ] {
            let error = parse(bad).unwrap_err();
            assert!(
                error.starts_with("SyntaxError"),
                "{bad:?} produced {error:?}"
            );
        }
    }

    #[test]
    fn refuses_a_document_that_is_nested_too_deeply() {
        let deep = "[".repeat(200) + &"]".repeat(200);
        let error = parse(&deep).unwrap_err();
        assert!(error.contains("too deeply"), "{error}");
    }

    #[test]
    fn a_repeated_key_keeps_the_last_value() {
        let value = parse(r#"{"a":1,"a":2}"#).expect("parsed");
        assert_eq!(to_string(&field(&value, "a")), "2");
    }

    #[test]
    fn stringify_round_trips_through_parse() {
        let source = r#"{"name":"toy","tags":["a","b"],"count":2,"ok":true,"nothing":null}"#;
        let value = parse(source).expect("parsed");
        assert_eq!(stringify(&value), source);
    }

    #[test]
    fn stringify_escapes_control_characters() {
        assert_eq!(
            stringify(&JsValue::Str("line\nbreak\t\"quoted\"".into())),
            r#""line\nbreak\t\"quoted\"""#
        );
    }

    #[test]
    fn values_with_no_json_form_become_null() {
        assert_eq!(stringify(&JsValue::Undefined), "null");
        assert_eq!(stringify(&JsValue::Number(f32::NAN)), "null");
        assert_eq!(stringify(&JsValue::Number(f32::INFINITY)), "null");
    }
}
