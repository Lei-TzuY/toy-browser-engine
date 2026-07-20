// ============================================================
//  html/tokenizer.rs  —  HTML5 Tokenizer (subset)
// ============================================================
//
//  Implements a character-by-character state machine that follows the
//  structure of the WHATWG HTML5 tokenisation spec
//  (https://html.spec.whatwg.org/multipage/parsing.html#tokenization)
//  but covers only the subset of states needed for typical HTML pages.
//
//  States implemented
//  ──────────────────
//  Data · TagOpen · EndTagOpen · TagName · SelfClosingStartTag
//  BeforeAttributeName · AttributeName · AfterAttributeName
//  BeforeAttributeValue · AttributeValueDoubleQuoted
//  AttributeValueSingleQuoted · AttributeValueUnquoted · AfterAttributeValue
//  MarkupDeclarationOpen · Comment{Start,StartDash} · Comment · CommentEndDash
//  CommentEnd · Doctype · BeforeDoctypeName · DoctypeName
//
//  Not implemented (error recovery, CDATA, script raw text, etc.)

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

/// Output tokens produced by the tokenizer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// `<!DOCTYPE html>`
    Doctype {
        name: String,
    },
    /// `<tag attr="val" …>` or `<tag />`
    StartTag {
        name: String,
        self_closing: bool,
        attributes: Vec<Attribute>,
    },
    /// `</tag>`
    EndTag {
        name: String,
    },
    /// A run of characters (may be a single char internally, but we merge runs).
    Character(char),
    /// `<!-- … -->`
    Comment(String),
    /// Logical end-of-file.
    Eof,
}

// ── Internal tokenizer state ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    SelfClosingStartTag,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentEndDash,
    CommentEnd,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
}

/// Temporary token being assembled before it is emitted.
#[derive(Debug, Default)]
struct CurrentToken {
    /// `true` → building a start tag; `false` → building an end tag.
    is_start: bool,
    name: String,
    self_closing: bool,
    attributes: Vec<Attribute>,
    /// Index into `attributes` of the attribute currently being built.
    current_attr: Option<usize>,
    /// Buffer for DOCTYPE name or comment text.
    buffer: String,
}

impl CurrentToken {
    fn push_attr_name_char(&mut self, c: char) {
        if let Some(i) = self.current_attr {
            self.attributes[i].name.push(c);
        }
    }

    fn push_attr_value_char(&mut self, c: char) {
        if let Some(i) = self.current_attr {
            self.attributes[i].value.push(c);
        }
    }

    fn start_new_attribute(&mut self) {
        self.attributes.push(Attribute {
            name: String::new(),
            value: String::new(),
        });
        self.current_attr = Some(self.attributes.len() - 1);
    }
}

// ── Public tokenizer ──────────────────────────────────────────────────────────

/// Tokenizes an HTML string into a flat `Vec<Token>`.
pub struct Tokenizer<'a> {
    input: &'a [char],
    pos: usize,
    state: State,
    current: CurrentToken,
    /// Internal queue; emitting multiple tokens at once is rare but happens.
    queue: Vec<Token>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(chars: &'a [char]) -> Self {
        Self {
            input: chars,
            pos: 0,
            state: State::Data,
            current: CurrentToken::default(),
            queue: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn consume(&mut self) -> Option<char> {
        let c = self.input.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn starts_with(&self, s: &str) -> bool {
        let bytes: Vec<char> = s.chars().collect();
        self.input[self.pos..].starts_with(&bytes)
    }

    fn consume_n(&mut self, n: usize) {
        self.pos += n;
    }

    fn emit_current_tag(&mut self) {
        let tok = if self.current.is_start {
            Token::StartTag {
                name: std::mem::take(&mut self.current.name),
                self_closing: self.current.self_closing,
                attributes: std::mem::take(&mut self.current.attributes),
            }
        } else {
            Token::EndTag {
                name: std::mem::take(&mut self.current.name),
            }
        };
        // Reset mutable fields
        self.current.self_closing = false;
        self.current.current_attr = None;
        self.queue.push(tok);
    }

    fn emit_comment(&mut self) {
        let text = std::mem::take(&mut self.current.buffer);
        self.queue.push(Token::Comment(text));
    }

    fn emit_doctype(&mut self) {
        let name = std::mem::take(&mut self.current.buffer);
        self.queue.push(Token::Doctype { name });
    }

    /// Advance the state machine one or more characters and push any newly
    /// emitted tokens onto `self.queue`.  Returns `false` when EOF is reached.
    fn step(&mut self) -> bool {
        match self.state.clone() {
            // ── Data ──────────────────────────────────────────────────────
            State::Data => match self.consume() {
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
                Some('<') => {
                    self.state = State::TagOpen;
                }
                Some('&') => {
                    let c = self.try_consume_char_ref().unwrap_or('&');
                    self.queue.push(Token::Character(c));
                }
                Some(c) => {
                    self.queue.push(Token::Character(c));
                }
            },

            // ── TagOpen ───────────────────────────────────────────────────
            State::TagOpen => match self.peek() {
                None => {
                    self.queue.push(Token::Character('<'));
                    self.queue.push(Token::Eof);
                    return false;
                }
                Some('!') => {
                    self.consume();
                    self.state = State::MarkupDeclarationOpen;
                }
                Some('/') => {
                    self.consume();
                    self.state = State::EndTagOpen;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.current = CurrentToken {
                        is_start: true,
                        ..Default::default()
                    };
                    self.state = State::TagName;
                }
                Some(c) => {
                    // Anything else: treat `<` as a literal character
                    self.queue.push(Token::Character('<'));
                    self.queue.push(Token::Character(c));
                    self.consume();
                    self.state = State::Data;
                }
            },

            // ── EndTagOpen ────────────────────────────────────────────────
            State::EndTagOpen => match self.peek() {
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.current = CurrentToken {
                        is_start: false,
                        ..Default::default()
                    };
                    self.state = State::TagName;
                }
                Some(c) => {
                    // Parse error; skip
                    self.consume();
                    let _ = c;
                    self.state = State::Data;
                }
            },

            // ── TagName ───────────────────────────────────────────────────
            State::TagName => match self.consume() {
                None | Some('>') => {
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some('/') => {
                    self.state = State::SelfClosingStartTag;
                }
                Some(c) if c.is_ascii_whitespace() => {
                    self.state = State::BeforeAttributeName;
                }
                Some(c) => {
                    self.current.name.push(c.to_ascii_lowercase());
                }
            },

            // ── SelfClosingStartTag ───────────────────────────────────────
            State::SelfClosingStartTag => match self.consume() {
                Some('>') => {
                    self.current.self_closing = true;
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some(c) => {
                    // parse error; treat as attribute name start
                    self.current.start_new_attribute();
                    self.current.push_attr_name_char(c.to_ascii_lowercase());
                    self.state = State::AttributeName;
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── BeforeAttributeName ───────────────────────────────────────
            State::BeforeAttributeName => match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.consume();
                }
                Some('/') | Some('>') | None => {
                    self.state = State::AfterAttributeName;
                }
                Some(_) => {
                    self.current.start_new_attribute();
                    self.state = State::AttributeName;
                }
            },

            // ── AttributeName ─────────────────────────────────────────────
            State::AttributeName => match self.consume() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.state = State::AfterAttributeName;
                }
                Some('/') => {
                    self.state = State::SelfClosingStartTag;
                }
                Some('>') => {
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some('=') => {
                    self.state = State::BeforeAttributeValue;
                }
                Some(c) => {
                    self.current.push_attr_name_char(c.to_ascii_lowercase());
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── AfterAttributeName ────────────────────────────────────────
            State::AfterAttributeName => match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.consume();
                }
                Some('/') => {
                    self.consume();
                    self.state = State::SelfClosingStartTag;
                }
                Some('=') => {
                    self.consume();
                    self.state = State::BeforeAttributeValue;
                }
                Some('>') => {
                    self.consume();
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
                Some(_) => {
                    self.current.start_new_attribute();
                    self.state = State::AttributeName;
                }
            },

            // ── BeforeAttributeValue ──────────────────────────────────────
            State::BeforeAttributeValue => match self.consume() {
                Some('"') => {
                    self.state = State::AttributeValueDoubleQuoted;
                }
                Some('\'') => {
                    self.state = State::AttributeValueSingleQuoted;
                }
                Some('>') => {
                    // parse error; attribute with empty value
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.push_attr_value_char(c);
                    self.state = State::AttributeValueUnquoted;
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── AttributeValueDoubleQuoted ────────────────────────────────
            State::AttributeValueDoubleQuoted => match self.consume() {
                Some('"') => {
                    self.state = State::AfterAttributeValueQuoted;
                }
                Some('&') => {
                    let c = self.try_consume_char_ref().unwrap_or('&');
                    self.current.push_attr_value_char(c);
                }
                Some(c) => {
                    self.current.push_attr_value_char(c);
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── AttributeValueSingleQuoted ────────────────────────────────
            State::AttributeValueSingleQuoted => match self.consume() {
                Some('\'') => {
                    self.state = State::AfterAttributeValueQuoted;
                }
                Some('&') => {
                    let c = self.try_consume_char_ref().unwrap_or('&');
                    self.current.push_attr_value_char(c);
                }
                Some(c) => {
                    self.current.push_attr_value_char(c);
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── AttributeValueUnquoted ────────────────────────────────────
            State::AttributeValueUnquoted => match self.consume() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.state = State::BeforeAttributeName;
                }
                Some('>') => {
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some('&') => {
                    let c = self.try_consume_char_ref().unwrap_or('&');
                    self.current.push_attr_value_char(c);
                }
                Some(c) => {
                    self.current.push_attr_value_char(c);
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── AfterAttributeValueQuoted ─────────────────────────────────
            State::AfterAttributeValueQuoted => match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.consume();
                    self.state = State::BeforeAttributeName;
                }
                Some('/') => {
                    self.consume();
                    self.state = State::SelfClosingStartTag;
                }
                Some('>') => {
                    self.consume();
                    self.emit_current_tag();
                    self.state = State::Data;
                }
                Some(_) => {
                    // parse error; reconsume in attribute name
                    self.current.start_new_attribute();
                    self.state = State::AttributeName;
                }
                None => {
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── MarkupDeclarationOpen ─────────────────────────────────────
            State::MarkupDeclarationOpen => {
                if self.starts_with("--") {
                    self.consume_n(2);
                    self.current.buffer.clear();
                    self.state = State::CommentStart;
                } else if self.starts_with("DOCTYPE") || self.starts_with("doctype") {
                    self.consume_n(7);
                    self.state = State::Doctype;
                } else {
                    // Bogus comment
                    self.current.buffer.clear();
                    self.state = State::Comment;
                }
            }

            // ── Comment states ────────────────────────────────────────────
            State::CommentStart => match self.consume() {
                Some('-') => {
                    self.state = State::CommentStartDash;
                }
                Some('>') => {
                    self.emit_comment();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.buffer.push(c);
                    self.state = State::Comment;
                }
                None => {
                    self.emit_comment();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::CommentStartDash => match self.consume() {
                Some('-') => {
                    self.state = State::CommentEnd;
                }
                Some('>') => {
                    self.emit_comment();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.buffer.push('-');
                    self.current.buffer.push(c);
                    self.state = State::Comment;
                }
                None => {
                    self.emit_comment();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::Comment => match self.consume() {
                Some('-') => {
                    self.state = State::CommentEndDash;
                }
                Some(c) => {
                    self.current.buffer.push(c);
                }
                None => {
                    self.emit_comment();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::CommentEndDash => match self.consume() {
                Some('-') => {
                    self.state = State::CommentEnd;
                }
                Some(c) => {
                    self.current.buffer.push('-');
                    self.current.buffer.push(c);
                    self.state = State::Comment;
                }
                None => {
                    self.emit_comment();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::CommentEnd => match self.consume() {
                Some('>') => {
                    self.emit_comment();
                    self.state = State::Data;
                }
                Some('-') => {
                    self.current.buffer.push('-');
                    // stay in CommentEnd
                }
                Some(c) => {
                    self.current.buffer.push_str("--");
                    self.current.buffer.push(c);
                    self.state = State::Comment;
                }
                None => {
                    self.emit_comment();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            // ── DOCTYPE ───────────────────────────────────────────────────
            State::Doctype => match self.consume() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.state = State::BeforeDoctypeName;
                }
                Some('>') => {
                    self.emit_doctype();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.buffer.push(c.to_ascii_lowercase());
                    self.state = State::DoctypeName;
                }
                None => {
                    self.emit_doctype();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::BeforeDoctypeName => match self.consume() {
                Some(c) if c.is_ascii_whitespace() => {
                    // stay
                }
                Some('>') => {
                    self.emit_doctype();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.buffer.push(c.to_ascii_lowercase());
                    self.state = State::DoctypeName;
                }
                None => {
                    self.emit_doctype();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },

            State::DoctypeName => match self.consume() {
                Some(c) if c.is_ascii_whitespace() => {
                    // ignore trailing whitespace in name
                }
                Some('>') => {
                    self.emit_doctype();
                    self.state = State::Data;
                }
                Some(c) => {
                    self.current.buffer.push(c.to_ascii_lowercase());
                }
                None => {
                    self.emit_doctype();
                    self.queue.push(Token::Eof);
                    return false;
                }
            },
        }

        true
    }

    // ── Character reference parsing ───────────────────────────────────────

    /// Try to parse a character reference starting at `self.pos` (right after `&`).
    /// On success advances `self.pos` past the reference and returns the char.
    /// On failure restores `self.pos` and returns `None`.
    fn try_consume_char_ref(&mut self) -> Option<char> {
        let restore = self.pos;
        let result = self.parse_char_ref_inner();
        if result.is_none() { self.pos = restore; }
        result
    }

    fn parse_char_ref_inner(&mut self) -> Option<char> {
        if self.peek() == Some('#') {
            self.pos += 1;
            let is_hex = matches!(self.peek(), Some('x') | Some('X'));
            if is_hex { self.pos += 1; }
            let start = self.pos;
            while self.peek().map_or(false, |c| {
                if is_hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }
            }) {
                self.pos += 1;
            }
            if self.pos == start { return None; }
            let digits: String = self.input[start..self.pos].iter().collect();
            if self.peek() == Some(';') { self.pos += 1; }
            let code: u32 = if is_hex {
                u32::from_str_radix(&digits, 16).ok()?
            } else {
                digits.parse().ok()?
            };
            char::from_u32(code)
        } else {
            let start = self.pos;
            while self.peek().map_or(false, |c| c.is_ascii_alphanumeric()) {
                self.pos += 1;
            }
            if self.pos == start { return None; }
            let name: String = self.input[start..self.pos].iter().collect();
            if self.peek() == Some(';') { self.pos += 1; }
            named_char_ref(&name)
        }
    }

    /// Tokenize the entire input and return all tokens.
    pub fn tokenize(mut self) -> Vec<Token> {
        let mut tokens: Vec<Token> = Vec::new();
        loop {
            // Drive the state machine until it emits at least one token.
            let mut reached_eof = false;
            while self.queue.is_empty() {
                if !self.step() {
                    reached_eof = true;
                    break;
                }
            }
            // Drain whatever was emitted (may include Token::Eof).
            tokens.extend(self.queue.drain(..));
            // Stop after EOF — do NOT call step() again, it would loop.
            if reached_eof {
                break;
            }
        }
        tokens
    }
}

// ── Named character reference table ──────────────────────────────────────────

fn named_char_ref(name: &str) -> Option<char> {
    Some(match name {
        // Essential
        "amp"    => '&',
        "lt"     => '<',
        "gt"     => '>',
        "quot"   => '"',
        "apos"   => '\'',
        "nbsp"   => '\u{00A0}',
        // Typography
        "mdash"  => '\u{2014}',
        "ndash"  => '\u{2013}',
        "hellip" => '\u{2026}',
        "lsquo"  => '\u{2018}',
        "rsquo"  => '\u{2019}',
        "ldquo"  => '\u{201C}',
        "rdquo"  => '\u{201D}',
        "laquo"  => '\u{00AB}',
        "raquo"  => '\u{00BB}',
        // Currency / symbols
        "copy"   => '\u{00A9}',
        "reg"    => '\u{00AE}',
        "trade"  => '\u{2122}',
        "euro"   => '\u{20AC}',
        "pound"  => '\u{00A3}',
        "yen"    => '\u{00A5}',
        "cent"   => '\u{00A2}',
        // Math / misc
        "deg"    => '\u{00B0}',
        "plusmn" => '\u{00B1}',
        "times"  => '\u{00D7}',
        "divide" => '\u{00F7}',
        "frac12" => '\u{00BD}',
        "frac14" => '\u{00BC}',
        "frac34" => '\u{00BE}',
        "sup2"   => '\u{00B2}',
        "sup3"   => '\u{00B3}',
        "infin"  => '\u{221E}',
        "pi"     => '\u{03C0}',
        "mu"     => '\u{03BC}',
        "alpha"  => '\u{03B1}',
        "beta"   => '\u{03B2}',
        "gamma"  => '\u{03B3}',
        "delta"  => '\u{03B4}',
        "sigma"  => '\u{03C3}',
        "Omega"  => '\u{03A9}',
        // Arrows
        "rarr"   => '\u{2192}',
        "larr"   => '\u{2190}',
        "uarr"   => '\u{2191}',
        "darr"   => '\u{2193}',
        "harr"   => '\u{2194}',
        // Misc symbols
        "check"  => '\u{2713}',
        "hearts" => '\u{2665}',
        "diams"  => '\u{2666}',
        "clubs"  => '\u{2663}',
        "spades" => '\u{2660}',
        "star"   => '\u{2605}',
        "bull"   => '\u{2022}',
        "middot" => '\u{00B7}',
        "dagger" => '\u{2020}',
        "Dagger" => '\u{2021}',
        "sect"   => '\u{00A7}',
        "para"   => '\u{00B6}',
        "permil" => '\u{2030}',
        "prime"  => '\u{2032}',
        "Prime"  => '\u{2033}',
        "oline"  => '\u{203E}',
        _ => return None,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(s: &str) -> Vec<Token> {
        let chars: Vec<char> = s.chars().collect();
        Tokenizer::new(&chars).tokenize()
    }

    fn non_eof(tokens: Vec<Token>) -> Vec<Token> {
        tokens.into_iter().filter(|t| *t != Token::Eof).collect()
    }

    #[test]
    fn simple_element() {
        let tokens = non_eof(tokenize("<p>hello</p>"));
        assert_eq!(
            tokens[0],
            Token::StartTag {
                name: "p".into(),
                self_closing: false,
                attributes: vec![]
            }
        );
        assert_eq!(tokens[1], Token::Character('h'));
        assert!(matches!(tokens.last(), Some(Token::EndTag { name }) if name == "p"));
    }

    #[test]
    fn attributes() {
        let tokens = non_eof(tokenize(r#"<a href="https://example.com" class='link'>text</a>"#));
        if let Token::StartTag { name, attributes, .. } = &tokens[0] {
            assert_eq!(name, "a");
            assert_eq!(attributes[0].name, "href");
            assert_eq!(attributes[0].value, "https://example.com");
            assert_eq!(attributes[1].name, "class");
            assert_eq!(attributes[1].value, "link");
        } else {
            panic!("expected StartTag");
        }
    }

    #[test]
    fn self_closing() {
        let tokens = non_eof(tokenize("<br />"));
        assert_eq!(
            tokens[0],
            Token::StartTag {
                name: "br".into(),
                self_closing: true,
                attributes: vec![]
            }
        );
    }

    #[test]
    fn comment() {
        let tokens = non_eof(tokenize("<!-- hello world -->"));
        assert_eq!(tokens[0], Token::Comment(" hello world ".into()));
    }

    #[test]
    fn doctype() {
        let tokens = non_eof(tokenize("<!DOCTYPE html>"));
        assert_eq!(tokens[0], Token::Doctype { name: "html".into() });
    }

    #[test]
    fn named_entity_amp() {
        let chars: Vec<char> = "a &amp; b".chars().collect();
        let tokens = non_eof(Tokenizer::new(&chars).tokenize());
        let text: String = tokens.iter().filter_map(|t| {
            if let Token::Character(c) = t { Some(*c) } else { None }
        }).collect();
        assert_eq!(text, "a & b");
    }

    #[test]
    fn named_entity_nbsp() {
        let chars: Vec<char> = "&nbsp;".chars().collect();
        let tokens = non_eof(Tokenizer::new(&chars).tokenize());
        assert_eq!(tokens[0], Token::Character('\u{00A0}'));
    }

    #[test]
    fn numeric_entity_decimal() {
        let chars: Vec<char> = "&#169;".chars().collect();
        let tokens = non_eof(Tokenizer::new(&chars).tokenize());
        assert_eq!(tokens[0], Token::Character('©'));
    }

    #[test]
    fn numeric_entity_hex() {
        let chars: Vec<char> = "&#x1F600;".chars().collect();
        let tokens = non_eof(Tokenizer::new(&chars).tokenize());
        assert_eq!(tokens[0], Token::Character('\u{1F600}'));
    }

    #[test]
    fn unknown_entity_emitted_literally() {
        let chars: Vec<char> = "&unknownxyz;".chars().collect();
        let tokens = non_eof(Tokenizer::new(&chars).tokenize());
        // Unknown entity: '&' emitted literally, then the name characters
        assert_eq!(tokens[0], Token::Character('&'));
    }
}
