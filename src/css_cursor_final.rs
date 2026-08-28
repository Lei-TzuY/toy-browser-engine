// ============================================================
// css_cursor_final.rs — cursor-aware facade over the core CSS parser
// ============================================================

/// Parser facade. All ordinary CSS is delegated byte-for-byte to the previous
/// parser. Only complex `cursor:` declarations with a top-level comma are
/// encoded into one opaque keyword so the scalar Value AST does not discard
/// candidate fallbacks after the first `url(...)`.
pub mod parser {
    pub use crate::css_prev::parser::*;

    const CURSOR_RAW_FUNCTION: &str = "__cursor_raw";

    pub fn parse_css(input: &str) -> Stylesheet {
        crate::css_prev::parser::parse_css(&rewrite_cursor_declarations(input))
    }

    pub fn parse_declaration_block(input: &str) -> Vec<Declaration> {
        // Reuse the stylesheet scanner rather than maintaining a second CSS
        // lexer. The synthetic selector is removed before delegating the block.
        let wrapped = format!("x{{{input}}}");
        let rewritten = rewrite_cursor_declarations(&wrapped);
        let body = rewritten
            .strip_prefix("x{")
            .and_then(|text| text.strip_suffix('}'))
            .unwrap_or(input);
        crate::css_prev::parser::parse_declaration_block(body)
    }

    pub fn parse_single_value(input: &str) -> Value {
        crate::css_prev::parser::parse_single_value(input)
    }

    /// Recover a complex cursor value preserved by this facade.
    pub fn decode_preserved_cursor_value(value: &str) -> Option<String> {
        let value = value.trim();
        let inner = value
            .strip_prefix(CURSOR_RAW_FUNCTION)?
            .strip_prefix('(')?
            .strip_suffix(')')?;
        decode_hex(inner)
    }

    /// True when a scalar `Value::Keyword` contains a preserved cursor list.
    pub fn is_preserved_cursor_value(value: &str) -> bool {
        decode_preserved_cursor_value(value).is_some()
    }

    fn rewrite_cursor_declarations(input: &str) -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut output = String::with_capacity(input.len());
        let mut i = 0usize;
        let mut block_depth = 0usize;
        let mut declaration_start = false;

        while i < chars.len() {
            if starts_comment(&chars, i) {
                i = copy_comment(&chars, i, &mut output);
                continue;
            }
            if matches!(chars[i], '\'' | '"') {
                i = copy_string(&chars, i, &mut output);
                continue;
            }

            match chars[i] {
                '{' => {
                    block_depth += 1;
                    declaration_start = true;
                    output.push(chars[i]);
                    i += 1;
                    continue;
                }
                '}' => {
                    block_depth = block_depth.saturating_sub(1);
                    declaration_start = false;
                    output.push(chars[i]);
                    i += 1;
                    continue;
                }
                ';' if block_depth > 0 => {
                    declaration_start = true;
                    output.push(';');
                    i += 1;
                    continue;
                }
                _ => {}
            }

            if block_depth == 0 || !declaration_start {
                output.push(chars[i]);
                i += 1;
                continue;
            }

            if chars[i].is_whitespace() {
                output.push(chars[i]);
                i += 1;
                continue;
            }

            if !is_ident_char(chars[i]) {
                // This block is probably an at-rule container followed by a
                // selector (`.class { ... }`), not a declaration list.
                declaration_start = false;
                output.push(chars[i]);
                i += 1;
                continue;
            }

            let name_start = i;
            while i < chars.len() && is_ident_char(chars[i]) {
                i += 1;
            }
            let name: String = chars[name_start..i].iter().collect();
            output.push_str(&name);

            if !name.eq_ignore_ascii_case("cursor") {
                declaration_start = false;
                continue;
            }

            let suffix_start = i;
            let mut colon = i;
            while colon < chars.len() && chars[colon].is_whitespace() {
                colon += 1;
            }
            if colon >= chars.len() || chars[colon] != ':' {
                declaration_start = false;
                continue;
            }

            // Preserve whitespace and the colon exactly.
            for c in &chars[suffix_start..=colon] {
                output.push(*c);
            }
            let value_start = colon + 1;
            let scan = scan_declaration_value(&chars, value_start);

            // `cursor:hover { ... }` can appear as a selector in a nested
            // @media block. A top-level opening brace proves this was not a
            // declaration, so leave the source untouched.
            if scan.open_brace {
                declaration_start = false;
                i = value_start;
                continue;
            }

            if !scan.top_level_comma {
                declaration_start = false;
                i = value_start;
                continue;
            }

            let raw: String = chars[value_start..scan.end].iter().collect();
            let (value, important) = split_important(&raw);
            output.push(' ');
            output.push_str(CURSOR_RAW_FUNCTION);
            output.push('(');
            output.push_str(&encode_hex(value.trim().as_bytes()));
            output.push(')');
            if important {
                output.push_str(" !important");
            }
            i = scan.end;
            declaration_start = false;
        }

        output
    }

    struct ValueScan {
        end: usize,
        top_level_comma: bool,
        open_brace: bool,
    }

    fn scan_declaration_value(chars: &[char], start: usize) -> ValueScan {
        let mut i = start;
        let mut paren_depth = 0usize;
        let mut top_level_comma = false;
        while i < chars.len() {
            if starts_comment(chars, i) {
                i = skip_comment(chars, i);
                continue;
            }
            if matches!(chars[i], '\'' | '"') {
                i = skip_string(chars, i);
                continue;
            }
            match chars[i] {
                '(' => paren_depth += 1,
                ')' => paren_depth = paren_depth.saturating_sub(1),
                ',' if paren_depth == 0 => top_level_comma = true,
                '{' if paren_depth == 0 => {
                    return ValueScan {
                        end: i,
                        top_level_comma,
                        open_brace: true,
                    };
                }
                ';' | '}' if paren_depth == 0 => {
                    return ValueScan {
                        end: i,
                        top_level_comma,
                        open_brace: false,
                    };
                }
                _ => {}
            }
            i += 1;
        }
        ValueScan {
            end: i,
            top_level_comma,
            open_brace: false,
        }
    }

    fn split_important(raw: &str) -> (&str, bool) {
        let trimmed = raw.trim_end();
        let lower = trimmed.to_ascii_lowercase();
        if lower.ends_with("!important") {
            let cut = trimmed.len() - "!important".len();
            (&trimmed[..cut], true)
        } else {
            (raw, false)
        }
    }

    fn is_ident_char(c: char) -> bool {
        c.is_alphanumeric() || matches!(c, '-' | '_')
    }

    fn starts_comment(chars: &[char], i: usize) -> bool {
        chars.get(i) == Some(&'/') && chars.get(i + 1) == Some(&'*')
    }

    fn skip_comment(chars: &[char], mut i: usize) -> usize {
        i += 2;
        while i + 1 < chars.len() {
            if chars[i] == '*' && chars[i + 1] == '/' {
                return i + 2;
            }
            i += 1;
        }
        chars.len()
    }

    fn copy_comment(chars: &[char], start: usize, output: &mut String) -> usize {
        let end = skip_comment(chars, start);
        for c in &chars[start..end] {
            output.push(*c);
        }
        end
    }

    fn skip_string(chars: &[char], start: usize) -> usize {
        let quote = chars[start];
        let mut i = start + 1;
        while i < chars.len() {
            if chars[i] == '\\' {
                i = (i + 2).min(chars.len());
                continue;
            }
            if chars[i] == quote {
                return i + 1;
            }
            i += 1;
        }
        chars.len()
    }

    fn copy_string(chars: &[char], start: usize, output: &mut String) -> usize {
        let end = skip_string(chars, start);
        for c in &chars[start..end] {
            output.push(*c);
        }
        end
    }

    fn encode_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn decode_hex(text: &str) -> Option<String> {
        if text.len() % 2 != 0 {
            return None;
        }
        let mut bytes = Vec::with_capacity(text.len() / 2);
        for pair in text.as_bytes().chunks_exact(2) {
            let hi = hex(pair[0])?;
            let lo = hex(pair[1])?;
            bytes.push((hi << 4) | lo);
        }
        String::from_utf8(bytes).ok()
    }

    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn cursor_keyword(css: &str) -> String {
            let sheet = parse_css(css);
            match &sheet.rules[0].declarations[0].value {
                Value::Keyword(value) => value.clone(),
                other => panic!("expected keyword, got {other:?}"),
            }
        }

        #[test]
        fn preserves_complex_cursor_list_without_touching_simple_keyword() {
            let raw = cursor_keyword(
                "a { cursor: url(\"a.cur\"), url(data:image/png;base64,AA==) 4 5, pointer; }",
            );
            let decoded = decode_preserved_cursor_value(&raw).unwrap();
            assert_eq!(
                decoded,
                "url(\"a.cur\"), url(data:image/png;base64,AA==) 4 5, pointer"
            );

            let simple = cursor_keyword("a { cursor: pointer; }");
            assert_eq!(simple, "pointer");
        }

        #[test]
        fn preserves_important_and_inline_declaration_blocks() {
            let declarations = parse_declaration_block(
                "cursor: url(a.cur) 2 3, crosshair !important; color: red",
            );
            assert_eq!(declarations.len(), 2);
            assert!(declarations[0].important);
            let Value::Keyword(raw) = &declarations[0].value else {
                panic!("cursor should be preserved as keyword")
            };
            assert_eq!(
                decode_preserved_cursor_value(raw).as_deref(),
                Some("url(a.cur) 2 3, crosshair")
            );
        }

        #[test]
        fn cursor_named_selector_inside_media_is_not_rewritten() {
            let sheet = parse_css("@media screen { cursor:hover { color: red; } }");
            assert_eq!(sheet.rules.len(), 1);
            assert_eq!(sheet.rules[0].declarations[0].name, "color");
        }
    }
}

pub use parser::*;
