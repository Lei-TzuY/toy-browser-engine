//! Shared Structured Field bare-item validation and ASCII string decoding.
//!
//! Reporting consumes a prefix; COEP requires a complete item. The byte length
//! keeps both existing callers on the same grammar without a public parser API.

fn valid_sf_string_char(ch: char) -> bool {
    ch.is_ascii() && matches!(ch as u8, 0x20..=0x7e)
}

pub(crate) fn structured_bare_item_len(value: &str) -> Option<usize> {
    let first = value.chars().next()?;
    match first {
        '"' => quoted_item_len(value),
        '?' => {
            if value.starts_with("?0") || value.starts_with("?1") {
                Some(2)
            } else {
                None
            }
        }
        ':' => binary_item_len(value),
        '@' => date_item_len(value),
        '%' => display_string_item_len(value),
        '-' | '0'..='9' => numeric_item_len(value),
        _ if valid_token_start(first) => {
            let len = value
                .char_indices()
                .take_while(|(_, ch)| valid_token_char(*ch))
                .map(|(index, ch)| index + ch.len_utf8())
                .last()?;
            Some(len)
        }
        _ => None,
    }
}

fn binary_item_len(value: &str) -> Option<usize> {
    let end = value[1..].find(':')? + 2;
    let payload = &value[1..end - 1];
    let bytes = payload.as_bytes();

    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'/' | b'='))
    {
        return None;
    }

    // Structured Fields Byte Sequences use RFC 4648 base64. Recipients are
    // encouraged to tolerate omitted padding, but malformed padding and input
    // lengths that cannot decode still fail parsing.
    let first_padding = bytes.iter().position(|byte| *byte == b'=');
    let data_len = first_padding.unwrap_or(bytes.len());
    let padding_len = bytes.len() - data_len;
    if padding_len > 2 || bytes[data_len..].iter().any(|byte| *byte != b'=') {
        return None;
    }

    if padding_len == 0 {
        if data_len % 4 == 1 {
            return None;
        }
    } else {
        if bytes.len() % 4 != 0 {
            return None;
        }
        let expected_padding = match data_len % 4 {
            2 => 2,
            3 => 1,
            _ => return None,
        };
        if padding_len != expected_padding {
            return None;
        }
    }

    Some(end)
}

fn quoted_item_len(value: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in value.char_indices().skip(1) {
        if escaped {
            if !matches!(ch, '"' | '\\') {
                return None;
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index + 1),
            _ if !valid_sf_string_char(ch) => return None,
            _ => {}
        }
    }
    None
}

fn numeric_item_len(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut index = usize::from(bytes.first() == Some(&b'-'));
    let digits_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let integer_digits = index - digits_start;
    if integer_digits == 0 {
        return None;
    }

    if bytes.get(index) == Some(&b'.') {
        // Structured Fields decimals have at most 12 digits before
        // the decimal point and at most three digits after it.
        if integer_digits > 12 {
            return None;
        }
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let fraction_digits = index - fraction_start;
        if fraction_digits == 0 || fraction_digits > 3 {
            return None;
        }
    } else if integer_digits > 15 {
        // sf-integer is bounded to 15 decimal digits (sign excluded).
        return None;
    }
    Some(index)
}

fn valid_token_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '*'
}

fn valid_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
                | ':'
                | '/'
        )
}

pub(crate) fn parse_quoted_string(value: &str) -> Option<String> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return None;
    }

    let inner = &value[1..value.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            if !matches!(ch, '"' | '\\') {
                return None;
            }
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return None,
            _ if !valid_sf_string_char(ch) => return None,
            _ => out.push(ch),
        }
    }
    if escaped {
        return None;
    }
    Some(out)
}

fn date_item_len(value: &str) -> Option<usize> {
    let integer = value.strip_prefix('@')?;
    let len = numeric_item_len(integer)?;
    if integer[..len].contains('.') {
        return None;
    }
    Some(len + 1)
}

fn display_string_item_len(value: &str) -> Option<usize> {
    if !value.starts_with("%\"") {
        return None;
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 2;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => {
                std::str::from_utf8(&decoded).ok()?;
                return Some(index + 1);
            }
            b'%' => {
                let high = lowercase_hex_value(*bytes.get(index + 1)?)?;
                let low = lowercase_hex_value(*bytes.get(index + 2)?)?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            0x20..=0x7e => {
                decoded.push(byte);
                index += 1;
            }
            _ => return None,
        }
    }
    None
}

fn lowercase_hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
