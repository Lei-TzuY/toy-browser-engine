// ============================================================
//  script/lexer.rs  —  JavaScript tokenizer
// ============================================================
//
//  Converts source text into a flat `Vec<Tok>`.  Handles line and
//  block comments, single/double-quoted strings with escapes,
//  numeric literals, identifiers/keywords, template literals, and
//  the operator set understood by the parser.
//
//  Regular-expression literals are not supported, so `/` is always
//  division (or the start of a comment) — which keeps the lexer
//  context-free.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // Literals & names
    Num(f32),
    Str(String),
    Ident(String),

    // Keywords
    Let,
    Const,
    Var,
    Function,
    Return,
    If,
    Else,
    While,
    For,
    Of,
    In,
    Break,
    Continue,
    Throw,
    Try,
    Catch,
    Finally,
    New,
    True,
    False,
    Null,
    Undefined,
    Typeof,

    // Punctuation
    Dot,
    DotDotDot,
    Comma,
    Semi,
    Colon,
    Question,
    QuestionQuestion,
    QuestionDot,

    // Template literals
    /// A part of a template literal before a `${` interpolation.
    TemplatePart(String),
    /// The final part of a template literal (after the last `}`, or the
    /// whole string if there are no interpolations).
    TemplateEnd(String),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Arrow,

    // Assignment
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,

    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Inc,
    Dec,

    // Comparison
    Eq,
    StrictEq,
    NotEq,
    StrictNotEq,
    Lt,
    Gt,
    Le,
    Ge,

    // Logical
    AndAnd,
    OrOr,
    Not,

    Eof,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    /// Tokenize the whole input. The returned vector always ends with `Tok::Eof`.
    pub fn tokenize(mut self) -> Vec<Tok> {
        let mut out = Vec::new();
        loop {
            let tok = self.next_token();
            let done = tok == Tok::Eof;
            // Template literals: a `TemplatePart` means there is a `${…}`
            // interpolation ahead. Lex the expression tokens normally until
            // the matching `}`, then continue the template.
            if let Tok::TemplatePart(_) = &tok {
                out.push(tok);
                let mut depth = 1u32;
                loop {
                    let inner = self.next_token();
                    if inner == Tok::Eof {
                        out.push(Tok::TemplateEnd(String::new()));
                        out.push(Tok::Eof);
                        return out;
                    }
                    match &inner {
                        Tok::LBrace => depth += 1,
                        Tok::RBrace => {
                            depth -= 1;
                            if depth == 0 {
                                // End of interpolation — continue the template.
                                let cont = self.lex_template_continuation();
                                let is_end = matches!(&cont, Tok::TemplateEnd(_));
                                out.push(cont);
                                if is_end {
                                    break;
                                }
                                // Another TemplatePart → another interpolation follows.
                                depth = 1;
                                continue;
                            }
                        }
                        _ => {}
                    }
                    out.push(inner);
                }
                continue;
            }
            out.push(tok);
            if done {
                break;
            }
        }
        out
    }

    fn peek(&self) -> char {
        self.chars.get(self.pos).copied().unwrap_or('\0')
    }

    fn peek_at(&self, offset: usize) -> char {
        self.chars.get(self.pos + offset).copied().unwrap_or('\0')
    }

    fn bump(&mut self) -> char {
        let c = self.peek();
        if self.pos < self.chars.len() {
            self.pos += 1;
        }
        c
    }

    /// Consume the two-character operator `expected` if it is next.
    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == expected {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_whitespace() {
                self.bump();
            }
            if self.peek() == '/' && self.peek_at(1) == '/' {
                while self.peek() != '\0' && self.peek() != '\n' {
                    self.bump();
                }
            } else if self.peek() == '/' && self.peek_at(1) == '*' {
                self.bump();
                self.bump();
                while self.peek() != '\0' && !(self.peek() == '*' && self.peek_at(1) == '/') {
                    self.bump();
                }
                self.bump();
                self.bump();
            } else {
                return;
            }
        }
    }

    fn next_token(&mut self) -> Tok {
        self.skip_trivia();
        if self.pos >= self.chars.len() {
            return Tok::Eof;
        }
        let c = self.peek();

        if c.is_alphabetic() || c == '_' || c == '$' {
            return self.lex_word();
        }
        if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_ascii_digit()) {
            return self.lex_number();
        }
        if c == '"' || c == '\'' {
            return self.lex_string();
        }
        if c == '`' {
            return self.lex_template();
        }

        self.bump();
        match c {
            '.' => {
                if self.peek() == '.' && self.peek_at(1) == '.' {
                    self.bump();
                    self.bump();
                    Tok::DotDotDot
                } else {
                    Tok::Dot
                }
            }
            ',' => Tok::Comma,
            ';' => Tok::Semi,
            ':' => Tok::Colon,
            '?' => {
                if self.eat('?') {
                    Tok::QuestionQuestion
                } else if self.peek() == '.' && !self.peek_at(1).is_ascii_digit() {
                    self.bump();
                    Tok::QuestionDot
                } else {
                    Tok::Question
                }
            }
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            '%' => Tok::Percent,
            '+' => {
                if self.eat('+') {
                    Tok::Inc
                } else if self.eat('=') {
                    Tok::PlusAssign
                } else {
                    Tok::Plus
                }
            }
            '-' => {
                if self.eat('-') {
                    Tok::Dec
                } else if self.eat('=') {
                    Tok::MinusAssign
                } else {
                    Tok::Minus
                }
            }
            '*' => {
                if self.eat('=') {
                    Tok::StarAssign
                } else {
                    Tok::Star
                }
            }
            '/' => {
                if self.eat('=') {
                    Tok::SlashAssign
                } else {
                    Tok::Slash
                }
            }
            '=' => {
                if self.eat('=') {
                    if self.eat('=') {
                        Tok::StrictEq
                    } else {
                        Tok::Eq
                    }
                } else if self.eat('>') {
                    Tok::Arrow
                } else {
                    Tok::Assign
                }
            }
            '!' => {
                if self.eat('=') {
                    if self.eat('=') {
                        Tok::StrictNotEq
                    } else {
                        Tok::NotEq
                    }
                } else {
                    Tok::Not
                }
            }
            '<' => {
                if self.eat('=') {
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if self.eat('=') {
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            '&' => {
                self.eat('&');
                Tok::AndAnd
            }
            '|' => {
                self.eat('|');
                Tok::OrOr
            }
            // Unknown characters are skipped rather than aborting the parse.
            _ => self.next_token(),
        }
    }

    fn lex_word(&mut self) -> Tok {
        let mut s = String::new();
        while self.peek().is_alphanumeric() || self.peek() == '_' || self.peek() == '$' {
            s.push(self.bump());
        }
        match s.as_str() {
            "let" => Tok::Let,
            "const" => Tok::Const,
            "var" => Tok::Var,
            "function" => Tok::Function,
            "return" => Tok::Return,
            "if" => Tok::If,
            "else" => Tok::Else,
            "while" => Tok::While,
            "for" => Tok::For,
            "of" => Tok::Of,
            "in" => Tok::In,
            "break" => Tok::Break,
            "continue" => Tok::Continue,
            "throw" => Tok::Throw,
            "try" => Tok::Try,
            "catch" => Tok::Catch,
            "finally" => Tok::Finally,
            "new" => Tok::New,
            "true" => Tok::True,
            "false" => Tok::False,
            "null" => Tok::Null,
            "undefined" => Tok::Undefined,
            "typeof" => Tok::Typeof,
            _ => Tok::Ident(s),
        }
    }

    fn lex_number(&mut self) -> Tok {
        let mut s = String::new();
        while self.peek().is_ascii_digit() {
            s.push(self.bump());
        }
        if self.peek() == '.' && self.peek_at(1).is_ascii_digit() {
            s.push(self.bump());
            while self.peek().is_ascii_digit() {
                s.push(self.bump());
            }
        } else if self.peek() == '.' {
            // Trailing dot: `1.` — consume it only when not a member access on a number.
            if !self.peek_at(1).is_alphabetic() {
                s.push(self.bump());
            }
        }
        Tok::Num(s.parse().unwrap_or(0.0))
    }

    fn lex_string(&mut self) -> Tok {
        let quote = self.bump();
        let mut s = String::new();
        while self.peek() != '\0' && self.peek() != quote {
            let c = self.bump();
            if c == '\\' {
                s.push(self.lex_escape());
            } else {
                s.push(c);
            }
        }
        if self.peek() == quote {
            self.bump();
        }
        Tok::Str(s)
    }

    /// Lex a template literal starting at the opening backtick.
    ///
    /// Returns `TemplatePart(s)` for each `...${` segment and
    /// `TemplateEnd(s)` for the final segment before the closing backtick.
    /// When there are no interpolations the whole thing is a single
    /// `TemplateEnd`.
    fn lex_template(&mut self) -> Tok {
        self.bump(); // opening backtick
        self.lex_template_segment()
    }

    /// Resume lexing a template literal after `${expr}`.
    ///
    /// Called by the public `tokenize` loop whenever it has just emitted the
    /// tokens for an interpolation and needs the next template segment.
    fn lex_template_continuation(&mut self) -> Tok {
        self.lex_template_segment()
    }

    fn lex_template_segment(&mut self) -> Tok {
        let mut s = String::new();
        loop {
            match self.peek() {
                '\0' | '`' => {
                    if self.peek() == '`' {
                        self.bump();
                    }
                    return Tok::TemplateEnd(s);
                }
                '$' if self.peek_at(1) == '{' => {
                    self.bump(); // $
                    self.bump(); // {
                    return Tok::TemplatePart(s);
                }
                '\\' => {
                    self.bump();
                    s.push(self.lex_escape());
                }
                _ => s.push(self.bump()),
            }
        }
    }

    fn lex_escape(&mut self) -> char {
        let esc = self.bump();
        match esc {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            other => other,
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<Tok> {
        Lexer::new(src).tokenize()
    }

    #[test]
    fn lexes_keywords_and_identifiers() {
        assert_eq!(
            lex("let x"),
            vec![Tok::Let, Tok::Ident("x".into()), Tok::Eof]
        );
    }

    #[test]
    fn lexes_multi_char_operators() {
        assert_eq!(
            lex("a === b !== c <= d >= e && f || g"),
            vec![
                Tok::Ident("a".into()),
                Tok::StrictEq,
                Tok::Ident("b".into()),
                Tok::StrictNotEq,
                Tok::Ident("c".into()),
                Tok::Le,
                Tok::Ident("d".into()),
                Tok::Ge,
                Tok::Ident("e".into()),
                Tok::AndAnd,
                Tok::Ident("f".into()),
                Tok::OrOr,
                Tok::Ident("g".into()),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn lexes_increment_and_compound_assignment() {
        assert_eq!(
            lex("i++; i += 2"),
            vec![
                Tok::Ident("i".into()),
                Tok::Inc,
                Tok::Semi,
                Tok::Ident("i".into()),
                Tok::PlusAssign,
                Tok::Num(2.0),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn skips_line_and_block_comments() {
        assert_eq!(
            lex("// gone\n/* also gone */ 1"),
            vec![Tok::Num(1.0), Tok::Eof]
        );
    }

    #[test]
    fn lexes_string_escapes() {
        assert_eq!(lex(r#""a\nb""#), vec![Tok::Str("a\nb".into()), Tok::Eof]);
    }

    #[test]
    fn lexes_float_literals() {
        assert_eq!(lex("3.5"), vec![Tok::Num(3.5), Tok::Eof]);
    }

    #[test]
    fn lexes_arrow_token() {
        assert_eq!(lex("=>"), vec![Tok::Arrow, Tok::Eof]);
    }
}
