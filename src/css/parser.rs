// ============================================================
//  css/parser.rs  —  CSS Tokenizer + Parser
// ============================================================
//
//  Parses a CSS string into a Stylesheet (rules with selectors
//  and declarations).  Handles:
//    • Type, class, ID, pseudo-class, pseudo-element selectors
//    • Descendant ( ) and child (>) combinators
//    • Specificity-based sorting within a rule's selector list
//    • Values: hex colors, named colors, lengths (px/em/%),
//      keywords, var(), calc(), linear-gradient()
//    • CSS custom properties (--name: value)
//    • @media queries: (min/max-width/height)
//    • /* … */ comments and @-rule handling

use std::collections::HashMap;

// ── Public types ─────────────────────────────────────────────────────────────

/// A single keyframe step in an @keyframes rule (e.g. 0%, 50%, from, to).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyframeStep {
    /// Offset from 0.0 to 1.0.
    pub offset: f32,
    pub declarations: Vec<Declaration>,
}

/// A complete `@keyframes <name>` rule containing ordered keyframe steps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyframeRule {
    pub name: String,
    pub steps: Vec<KeyframeStep>,
}

#[derive(Debug, Default, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    pub keyframes: HashMap<String, KeyframeRule>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    /// `Some(mq)` means this rule only applies when `mq` is satisfied.
    pub media_query: Option<MediaQuery>,
}

/// How a `SelectorPart` relates to the part that precedes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Combinator {
    /// First part in the chain — no preceding part.
    #[default]
    Root,
    /// Whitespace: must be a descendant.
    Descendant,
    /// `>`: must be a direct child.
    Child,
    /// `+`: immediately following sibling.
    AdjacentSibling,
    /// `~`: any following sibling.
    GeneralSibling,
}

/// One simple selector component in a compound selector chain.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectorPart {
    pub combinator: Combinator,
    pub tag_name: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub pseudo_classes: Vec<PseudoClass>,
    pub attributes: Vec<(String, Option<String>)>,
    /// `::before`, `::after`, etc. (parsed but not yet rendered).
    pub pseudo_element: Option<String>,
}

impl SelectorPart {
    fn is_empty(&self) -> bool {
        self.tag_name.is_none()
            && self.id.is_none()
            && self.classes.is_empty()
            && self.pseudo_classes.is_empty()
            && self.attributes.is_empty()
            && self.pseudo_element.is_none()
    }
}

/// A compound selector (possibly with descendant/child combinators).
/// `parts` is ordered left-to-right; the last part matches the subject element.
#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub parts: Vec<SelectorPart>,
}

impl Selector {
    /// CSS specificity: (id_count, class_count, tag_count) — summed over all parts.
    /// Pseudo-classes count as class-level (b column).
    pub fn specificity(&self) -> (usize, usize, usize) {
        self.parts.iter().fold((0, 0, 0), |acc, part| {
            (
                acc.0 + usize::from(part.id.is_some()),
                acc.1 + part.classes.len() + part.pseudo_classes.len(),
                acc.2 + usize::from(part.tag_name.is_some()),
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Value,
    /// Set by `!important`; these declarations win over normal ones in the
    /// cascade regardless of specificity.
    pub important: bool,
}

impl Declaration {
    pub fn new(name: impl Into<String>, value: Value) -> Self {
        Self {
            name: name.into(),
            value,
            important: false,
        }
    }
}

/// One color stop inside a gradient function.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorStop {
    pub color: Color,
    /// Position in [0,1]. `None` → auto-distribute evenly.
    pub position: Option<f32>,
}

/// A parsed `linear-gradient(…)` value.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearGradient {
    /// CSS angle in degrees: 0 = to top, 90 = to right, 180 = to bottom (default).
    pub angle_deg: f32,
    pub stops: Vec<ColorStop>,
}

// ── Pseudo-class types ────────────────────────────────────────────────────────

/// The `An+B` expression used in `:nth-child(An+B)` and related selectors.
#[derive(Debug, Clone, PartialEq)]
pub struct NthExpr {
    pub a: i32,
    pub b: i32,
}

impl NthExpr {
    /// Returns `true` if `position` (1-indexed) satisfies `An+B` for some non-negative integer `n`.
    pub fn matches(&self, position: usize) -> bool {
        let k = position as i32;
        if self.a == 0 {
            k == self.b
        } else {
            let n = (k - self.b) as f32 / self.a as f32;
            n >= 0.0 && (n - n.round()).abs() < 1e-6
        }
    }
}

/// Pseudo-classes supported in selector matching.
#[derive(Debug, Clone, PartialEq)]
pub enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    Root,
    Empty,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthChild(NthExpr),
    NthLastChild(NthExpr),
    NthOfType(NthExpr),
    NthLastOfType(NthExpr),
    Not(Box<SelectorPart>),
    // Interactive pseudo-classes, resolved against the document's focus and
    // form-control state during matching.
    Hover,
    Focus,
    /// Matches an element that contains (or is) the focused element.
    FocusWithin,
    Active,
    Checked,
    Disabled,
    Enabled,
    /// Matches a control showing its placeholder because its value is empty.
    PlaceholderShown,
    // Link history is not tracked, so these never match.
    Visited,
    Link,
}

// ── calc() expression tree ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CalcExpr {
    Literal(f32, Unit),
    Percent(f32),
    Add(Box<CalcExpr>, Box<CalcExpr>),
    Sub(Box<CalcExpr>, Box<CalcExpr>),
    Mul(Box<CalcExpr>, Box<CalcExpr>),
    Div(Box<CalcExpr>, Box<CalcExpr>),
}

// ── @media query ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MediaQuery {
    pub conditions: Vec<MediaCondition>,
}

#[derive(Debug, Clone)]
pub enum MediaCondition {
    MinWidth(f32),
    MaxWidth(f32),
    MinHeight(f32),
    MaxHeight(f32),
}

impl MediaQuery {
    /// Returns `true` when this media query is satisfied by the given viewport dimensions.
    pub fn matches(&self, viewport_width: f32, viewport_height: f32) -> bool {
        self.conditions.iter().all(|c| match c {
            MediaCondition::MinWidth(w) => viewport_width >= *w,
            MediaCondition::MaxWidth(w) => viewport_width <= *w,
            MediaCondition::MinHeight(h) => viewport_height >= *h,
            MediaCondition::MaxHeight(h) => viewport_height <= *h,
        })
    }
}

/// Box shadow specification.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: Color,
}

/// 2D Transform specification.
#[derive(Debug, Clone, PartialEq)]
pub struct Transform {
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale: f32,
}

/// CSS Transition Timing Function
#[derive(Debug, Clone, PartialEq)]
pub enum TimingFunction {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),
}

impl TimingFunction {
    pub fn evaluate(&self, progress: f32) -> f32 {
        let p = progress.clamp(0.0, 1.0);
        match self {
            TimingFunction::Linear => p,
            TimingFunction::Ease => cubic_bezier_solve(0.25, 0.1, 0.25, 1.0, p),
            TimingFunction::EaseIn => cubic_bezier_solve(0.42, 0.0, 1.0, 1.0, p),
            TimingFunction::EaseOut => cubic_bezier_solve(0.0, 0.0, 0.58, 1.0, p),
            TimingFunction::EaseInOut => cubic_bezier_solve(0.42, 0.0, 0.58, 1.0, p),
            TimingFunction::CubicBezier(x1, y1, x2, y2) => cubic_bezier_solve(*x1, *y1, *x2, *y2, p),
        }
    }
}

pub fn cubic_bezier_solve(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let mut low = 0.0f32;
    let mut high = 1.0f32;
    let mut t = x.clamp(0.0, 1.0);

    for _ in 0..16 {
        let sample = 3.0 * (1.0 - t).powi(2) * t * x1 + 3.0 * (1.0 - t) * t.powi(2) * x2 + t.powi(3);
        if (sample - x).abs() < 1e-4 {
            break;
        }
        if sample < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) * 0.5;
    }

    3.0 * (1.0 - t).powi(2) * t * y1 + 3.0 * (1.0 - t) * t.powi(2) * y2 + t.powi(3)
}

/// A parsed single transition specification
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionSpec {
    pub property: String,
    pub duration_ms: f32,
    pub timing_function: TimingFunction,
    pub delay_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationFillMode {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationIterationCount {
    Finite(f32),
    Infinite,
}

impl Default for AnimationIterationCount {
    fn default() -> Self {
        Self::Finite(1.0)
    }
}

/// A parsed single animation specification
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationSpec {
    pub name: String,
    pub duration_ms: f32,
    pub timing_function: TimingFunction,
    pub delay_ms: f32,
    pub iteration_count: AnimationIterationCount,
    pub direction: AnimationDirection,
    pub fill_mode: AnimationFillMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterFunction {
    Blur(f32),
    Grayscale(f32),
    Brightness(f32),
    Contrast(f32),
    Invert(f32),
    Opacity(f32),
    None,
}

impl FilterFunction {
    pub fn to_css_string(&self) -> String {
        match self {
            FilterFunction::Blur(px) => format!("blur({}px)", px),
            FilterFunction::Grayscale(amt) => format!("grayscale({}%)", (amt * 100.0).round()),
            FilterFunction::Brightness(amt) => format!("brightness({})", amt),
            FilterFunction::Contrast(amt) => format!("contrast({})", amt),
            FilterFunction::Invert(amt) => format!("invert({}%)", (amt * 100.0).round()),
            FilterFunction::Opacity(amt) => format!("opacity({})", amt),
            FilterFunction::None => "none".to_string(),
        }
    }
}

pub fn parse_filter(input: &str) -> Option<Vec<FilterFunction>> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return Some(vec![FilterFunction::None]);
    }
    let mut funcs = Vec::new();
    let mut pos = 0;
    let bytes = trimmed.as_bytes();

    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        let open_paren = match trimmed[pos..].find('(') {
            Some(idx) => pos + idx,
            None => break,
        };
        let close_paren = match trimmed[open_paren..].find(')') {
            Some(idx) => open_paren + idx,
            None => break,
        };

        let fname = trimmed[pos..open_paren].trim().to_ascii_lowercase();
        let arg = trimmed[open_paren + 1..close_paren].trim();

        match fname.as_str() {
            "blur" => {
                let px = arg.trim_end_matches("px").trim().parse::<f32>().unwrap_or(0.0);
                funcs.push(FilterFunction::Blur(px));
            }
            "grayscale" => {
                let val = if let Some(stripped) = arg.strip_suffix('%') {
                    stripped.trim().parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    arg.parse::<f32>().unwrap_or(1.0)
                };
                funcs.push(FilterFunction::Grayscale(val.clamp(0.0, 1.0)));
            }
            "brightness" => {
                let val = if let Some(stripped) = arg.strip_suffix('%') {
                    stripped.trim().parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    arg.parse::<f32>().unwrap_or(1.0)
                };
                funcs.push(FilterFunction::Brightness(val.max(0.0)));
            }
            "contrast" => {
                let val = if let Some(stripped) = arg.strip_suffix('%') {
                    stripped.trim().parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    arg.parse::<f32>().unwrap_or(1.0)
                };
                funcs.push(FilterFunction::Contrast(val.max(0.0)));
            }
            "invert" => {
                let val = if let Some(stripped) = arg.strip_suffix('%') {
                    stripped.trim().parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    arg.parse::<f32>().unwrap_or(1.0)
                };
                funcs.push(FilterFunction::Invert(val.clamp(0.0, 1.0)));
            }
            "opacity" => {
                let val = if let Some(stripped) = arg.strip_suffix('%') {
                    stripped.trim().parse::<f32>().unwrap_or(100.0) / 100.0
                } else {
                    arg.parse::<f32>().unwrap_or(1.0)
                };
                funcs.push(FilterFunction::Opacity(val.clamp(0.0, 1.0)));
            }
            _ => {}
        }
        pos = close_paren + 1;
    }

    if funcs.is_empty() {
        None
    } else {
        Some(funcs)
    }
}

// ── Value ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, Unit),
    Color(Color),
    /// Unitless number (e.g. `line-height: 1.5`, `opacity: 0.8`, `flex-grow: 2`).
    Number(f32),
    LinearGradient(LinearGradient),
    BoxShadow(BoxShadow),
    Transform(Transform),
    Transition(Vec<TransitionSpec>),
    Animation(Vec<AnimationSpec>),
    Filter(Vec<FilterFunction>),
    /// `var(--name)` or `var(--name, fallback)`.
    Var {
        name: String,
        fallback: Option<Box<Value>>,
    },
    /// `calc(expression)`.
    Calc(Box<CalcExpr>),
}

impl Value {
    /// Resolve to pixels (no containing-block context; use `layout::to_px` for % and calc).
    pub fn to_px(&self) -> f32 {
        match self {
            Value::Length(n, Unit::Px) => *n,
            Value::Length(n, Unit::Em) => n * 16.0,
            _ => 0.0,
        }
    }

    /// Format this CSS value as a standard CSS string.
    pub fn to_css_string(&self) -> String {
        match self {
            Value::Keyword(k) => k.clone(),
            Value::Length(n, Unit::Px) => format!("{}px", n),
            Value::Length(n, Unit::Em) => format!("{}em", n),
            Value::Length(n, Unit::Percent) => format!("{}%", n),
            Value::Length(n, Unit::Fr) => format!("{}fr", n),
            Value::Color(c) => {
                if c.a == 255 {
                    format!("rgb({}, {}, {})", c.r, c.g, c.b)
                } else {
                    let a_float = c.a as f32 / 255.0;
                    format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, a_float)
                }
            }
            Value::Number(n) => format!("{}", n),
            Value::Calc(_) => "calc(...)".to_string(),
            Value::Var { name, .. } => format!("var({})", name),
            Value::LinearGradient(_) => "linear-gradient(...)".to_string(),
            Value::BoxShadow(_) => "box-shadow(...)".to_string(),
            Value::Transform(_) => "transform(...)".to_string(),
            Value::Transition(_) => "transition(...)".to_string(),
            Value::Animation(_) => "animation(...)".to_string(),
            Value::Filter(funcs) => {
                funcs.iter().map(|f| f.to_css_string()).collect::<Vec<_>>().join(" ")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Unit {
    Px,
    Em,
    Percent,
    Fr,
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn transparent() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }
    }
}

// ── Parser internals ──────────────────────────────────────────────────────────

struct Parser {
    chars: Vec<char>,
    pos: usize,
    keyframes: HashMap<String, KeyframeRule>,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            keyframes: HashMap::new(),
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.chars.len()
    }
    fn peek(&self) -> char {
        self.chars.get(self.pos).copied().unwrap_or('\0')
    }
    fn consume(&mut self) -> char {
        let c = self.peek();
        if !self.eof() {
            self.pos += 1;
        }
        c
    }

    fn consume_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while !self.eof() && f(self.peek()) {
            s.push(self.consume());
        }
        s
    }

    fn skip_ws(&mut self) {
        self.consume_while(char::is_whitespace);
    }

    fn skip_comment(&mut self) -> bool {
        if self.peek() == '/' && self.chars.get(self.pos + 1) == Some(&'*') {
            self.pos += 2;
            loop {
                if self.eof() {
                    break;
                }
                if self.peek() == '*' && self.chars.get(self.pos + 1) == Some(&'/') {
                    self.pos += 2;
                    break;
                }
                self.pos += 1;
            }
            true
        } else {
            false
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            let before = self.pos;
            self.skip_ws();
            self.skip_comment();
            if self.pos == before {
                break;
            }
        }
    }

    /// Skip a `{ … }` block with brace nesting.  Expects `{` to be the next character.
    fn skip_brace_block(&mut self) {
        if self.peek() == '{' {
            self.consume();
        }
        let mut depth = 1i32;
        while !self.eof() && depth > 0 {
            match self.consume() {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
    }

    fn parse_ident(&mut self) -> String {
        self.consume_while(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    // ── Selector parsing ──────────────────────────────────────────────────

    /// Parse one simple selector component (tag / #id / .class / * / :pseudo / ::pseudo-elem / [attr]).
    fn parse_simple_part(&mut self) -> SelectorPart {
        let mut part = SelectorPart::default();
        loop {
            match self.peek() {
                '#' => {
                    self.consume();
                    part.id = Some(self.parse_ident());
                }
                '.' => {
                    self.consume();
                    part.classes.push(self.parse_ident());
                }
                '*' => {
                    self.consume();
                }
                '[' => {
                    self.consume();
                    self.skip_ws_and_comments();
                    let attr_name = self.parse_ident();
                    self.skip_ws_and_comments();
                    let val = if self.peek() == '='
                        || (self.peek() == '^' && self.chars.get(self.pos + 1) == Some(&'='))
                    {
                        if self.peek() == '^' {
                            self.consume();
                        }
                        self.consume();
                        self.skip_ws_and_comments();
                        let raw_val = if self.peek() == '"' || self.peek() == '\'' {
                            let quote = self.consume();
                            let s = self.consume_while(|c| c != quote);
                            if self.peek() == quote {
                                self.consume();
                            }
                            s
                        } else {
                            self.consume_while(|c| c != ']' && !c.is_whitespace())
                        };
                        Some(raw_val)
                    } else {
                        None
                    };
                    self.consume_while(|c| c != ']');
                    if self.peek() == ']' {
                        self.consume();
                    }
                    part.attributes.push((attr_name, val));
                }
                ':' => {
                    self.consume();
                    if self.peek() == ':' {
                        // Pseudo-element (::before, ::after, …)
                        self.consume();
                        part.pseudo_element = Some(self.parse_ident());
                    } else {
                        // Pseudo-class
                        let name = self.parse_ident().to_ascii_lowercase();
                        let pc = self.parse_pseudo_class(&name);
                        part.pseudo_classes.push(pc);
                    }
                }
                c if c.is_alphabetic() || c == '_' => {
                    part.tag_name = Some(self.parse_ident());
                }
                _ => break,
            }
        }
        part
    }

    /// Parse a pseudo-class by name, consuming the argument `(…)` when present.
    fn parse_pseudo_class(&mut self, name: &str) -> PseudoClass {
        match name {
            "first-child" => PseudoClass::FirstChild,
            "last-child" => PseudoClass::LastChild,
            "only-child" => PseudoClass::OnlyChild,
            "root" => PseudoClass::Root,
            "empty" => PseudoClass::Empty,
            "first-of-type" => PseudoClass::FirstOfType,
            "last-of-type" => PseudoClass::LastOfType,
            "only-of-type" => PseudoClass::OnlyOfType,
            "hover" => PseudoClass::Hover,
            "focus" => PseudoClass::Focus,
            "focus-within" => PseudoClass::FocusWithin,
            "placeholder-shown" => PseudoClass::PlaceholderShown,
            "active" => PseudoClass::Active,
            "visited" => PseudoClass::Visited,
            "link" => PseudoClass::Link,
            "checked" => PseudoClass::Checked,
            "disabled" => PseudoClass::Disabled,
            "enabled" => PseudoClass::Enabled,
            "nth-child" => self.parse_nth_pseudo(PseudoClass::NthChild),
            "nth-last-child" => self.parse_nth_pseudo(PseudoClass::NthLastChild),
            "nth-of-type" => self.parse_nth_pseudo(PseudoClass::NthOfType),
            "nth-last-of-type" => self.parse_nth_pseudo(PseudoClass::NthLastOfType),
            "not" => {
                if self.peek() == '(' {
                    self.consume();
                    self.skip_ws_and_comments();
                    let inner = self.parse_simple_part();
                    self.skip_ws_and_comments();
                    if self.peek() == ')' {
                        self.consume();
                    }
                    PseudoClass::Not(Box::new(inner))
                } else {
                    PseudoClass::Not(Box::default())
                }
            }
            // Anything else: treat as non-matching (Hover is the "never matches" stand-in).
            _ => PseudoClass::Hover,
        }
    }

    fn parse_nth_pseudo(&mut self, ctor: impl Fn(NthExpr) -> PseudoClass) -> PseudoClass {
        if self.peek() == '(' {
            self.consume();
            let expr = self.parse_nth_expr();
            if self.peek() == ')' {
                self.consume();
            }
            ctor(expr)
        } else {
            ctor(NthExpr { a: 0, b: 0 })
        }
    }

    /// Parse the `An+B` expression inside `nth-child(…)`.
    fn parse_nth_expr(&mut self) -> NthExpr {
        self.skip_ws_and_comments();
        let raw = self.consume_while(|c| c != ')');
        let s = raw.trim();
        match s {
            "odd" => NthExpr { a: 2, b: 1 },
            "even" => NthExpr { a: 2, b: 0 },
            _ if s.contains('n') => {
                let n_pos = s.find('n').unwrap();
                let a_str = s[..n_pos].trim();
                let b_str = s[n_pos + 1..].replace(' ', ""); // strip spaces around +/-
                let a: i32 = match a_str {
                    "" | "+" => 1,
                    "-" => -1,
                    x => x.parse().unwrap_or(1),
                };
                let b: i32 = if b_str.is_empty() {
                    0
                } else {
                    b_str.parse().unwrap_or(0)
                };
                NthExpr { a, b }
            }
            _ => NthExpr {
                a: 0,
                b: s.parse().unwrap_or(0),
            },
        }
    }

    /// Parse a full compound selector (possibly chained with descendant / child combinators).
    fn parse_compound_selector(&mut self) -> Option<Selector> {
        self.skip_ws_and_comments();
        if self.peek() == '{' || self.eof() {
            return None;
        }

        let first = self.parse_simple_part();
        if first.is_empty() {
            return None;
        }
        let mut parts = vec![first];

        loop {
            let before_ws = self.pos;
            self.skip_ws_and_comments();
            let had_ws = self.pos > before_ws;

            match self.peek() {
                ',' | '{' | '\0' => break,
                '>' => {
                    self.consume();
                    self.skip_ws_and_comments();
                    let mut part = self.parse_simple_part();
                    if part.is_empty() {
                        break;
                    }
                    part.combinator = Combinator::Child;
                    parts.push(part);
                }
                '+' => {
                    self.consume();
                    self.skip_ws_and_comments();
                    let mut part = self.parse_simple_part();
                    if part.is_empty() {
                        break;
                    }
                    part.combinator = Combinator::AdjacentSibling;
                    parts.push(part);
                }
                '~' => {
                    self.consume();
                    self.skip_ws_and_comments();
                    let mut part = self.parse_simple_part();
                    if part.is_empty() {
                        break;
                    }
                    part.combinator = Combinator::GeneralSibling;
                    parts.push(part);
                }
                _ if had_ws => {
                    let mut part = self.parse_simple_part();
                    if part.is_empty() {
                        break;
                    }
                    part.combinator = Combinator::Descendant;
                    parts.push(part);
                }
                _ => break,
            }
        }

        Some(Selector { parts })
    }

    fn parse_selectors(&mut self) -> Vec<Selector> {
        let mut selectors = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == '{' || self.eof() {
                break;
            }
            if let Some(sel) = self.parse_compound_selector() {
                selectors.push(sel);
            }
            self.skip_ws_and_comments();
            if self.peek() == ',' {
                self.consume();
            } else {
                break;
            }
        }
        // Sort highest specificity first for cascade ordering.
        selectors.sort_by_key(|selector| std::cmp::Reverse(selector.specificity()));
        selectors
    }

    // ── Value parsing ─────────────────────────────────────────────────────

    fn parse_hex_color(&mut self) -> Value {
        let hex = self.consume_while(|c| c.is_ascii_hexdigit());
        let parse2 = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0);
        let expand1 = |s: &str| {
            let c = &s[..1];
            parse2(&format!("{c}{c}"))
        };
        let color = match hex.len() {
            3 => Color::rgb(expand1(&hex[0..]), expand1(&hex[1..]), expand1(&hex[2..])),
            6 => Color::rgb(parse2(&hex[0..2]), parse2(&hex[2..4]), parse2(&hex[4..6])),
            8 => Color::rgba(
                parse2(&hex[0..2]),
                parse2(&hex[2..4]),
                parse2(&hex[4..6]),
                parse2(&hex[6..8]),
            ),
            _ => Color::rgb(0, 0, 0),
        };
        Value::Color(color)
    }

    fn parse_length(&mut self) -> Value {
        let mut num_s = String::new();
        if matches!(self.peek(), '+' | '-') {
            num_s.push(self.consume());
        }
        num_s.push_str(&self.consume_while(|c| c.is_ascii_digit() || c == '.'));
        let num: f32 = num_s.parse().unwrap_or(0.0);
        if self.peek() == '%' {
            self.consume();
            return Value::Length(num, Unit::Percent);
        }
        let unit = self.consume_while(char::is_alphabetic);
        match unit.to_ascii_lowercase().as_str() {
            "em" => Value::Length(num, Unit::Em),
            "px" => Value::Length(num, Unit::Px),
            "rem" => Value::Length(num, Unit::Px),
            "fr" => Value::Length(num, Unit::Fr),
            "" => Value::Number(num),
            _ => Value::Length(num, Unit::Px),
        }
    }

    fn parse_value(&mut self) -> Value {
        match self.peek() {
            '#' => {
                self.consume();
                self.parse_hex_color()
            }
            c if c.is_ascii_digit() || c == '.' => self.parse_length(),
            '+' | '-'
                if self
                    .chars
                    .get(self.pos + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.') =>
            {
                self.parse_length()
            }
            _ => {
                let kw = self.parse_ident();
                if self.peek() == '(' {
                    self.consume();
                    return self.parse_function(kw);
                }
                named_color(&kw)
                    .map(Value::Color)
                    .unwrap_or(Value::Keyword(kw))
            }
        }
    }

    fn parse_function(&mut self, name: String) -> Value {
        match name.to_ascii_lowercase().as_str() {
            "linear-gradient" => self.parse_linear_gradient_inner(),
            "var" => self.parse_var_inner(),
            "calc" => self.parse_calc_inner(),
            "rgb" | "rgba" => self.parse_rgb_inner(),
            "hsl" | "hsla" => self.parse_hsl_inner(),
            _ => {
                // Unknown function: preserve full function call string as Value::Keyword.
                let mut body = String::new();
                let mut depth = 1i32;
                while !self.eof() && depth > 0 {
                    let c = self.consume();
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    body.push(c);
                }
                Value::Keyword(format!("{}({})", name, body))
            }
        }
    }

    /// Parse inside `var(--name)` or `var(--name, fallback)`.
    fn parse_var_inner(&mut self) -> Value {
        self.skip_ws_and_comments();
        let name = self.consume_while(|c| !c.is_whitespace() && c != ',' && c != ')');
        self.skip_ws_and_comments();
        let fallback = if self.peek() == ',' {
            self.consume();
            self.skip_ws_and_comments();
            Some(Box::new(self.parse_value()))
        } else {
            None
        };
        // Drain to closing ')'.
        self.consume_while(|c| c != ')');
        if self.peek() == ')' {
            self.consume();
        }
        Value::Var {
            name: name.trim().to_string(),
            fallback,
        }
    }

    /// Parse inside `calc(…)` — supports a single binary operation.
    fn parse_calc_inner(&mut self) -> Value {
        self.skip_ws_and_comments();
        let lhs = self.parse_calc_atom();
        self.skip_ws_and_comments();

        let op = self.peek();
        if matches!(op, '+' | '-' | '*' | '/') {
            self.consume();
            self.skip_ws_and_comments();
            let rhs = self.parse_calc_atom();
            self.skip_ws_and_comments();
            if self.peek() == ')' {
                self.consume();
            }
            let expr = match op {
                '+' => CalcExpr::Add(Box::new(lhs), Box::new(rhs)),
                '-' => CalcExpr::Sub(Box::new(lhs), Box::new(rhs)),
                '*' => CalcExpr::Mul(Box::new(lhs), Box::new(rhs)),
                '/' => CalcExpr::Div(Box::new(lhs), Box::new(rhs)),
                _ => lhs,
            };
            Value::Calc(Box::new(expr))
        } else {
            if self.peek() == ')' {
                self.consume();
            }
            Value::Calc(Box::new(lhs))
        }
    }

    fn parse_calc_atom(&mut self) -> CalcExpr {
        self.skip_ws_and_comments();
        let is_num_start = self.peek().is_ascii_digit()
            || self.peek() == '.'
            || (self.peek() == '-'
                && self
                    .chars
                    .get(self.pos + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.'));
        if is_num_start {
            match self.parse_length() {
                Value::Length(n, Unit::Percent) => CalcExpr::Percent(n),
                Value::Length(n, u) => CalcExpr::Literal(n, u),
                Value::Number(n) => CalcExpr::Literal(n, Unit::Px),
                _ => CalcExpr::Literal(0.0, Unit::Px),
            }
        } else {
            self.consume_while(|c| !matches!(c, '+' | '-' | '*' | '/' | ')'));
            CalcExpr::Literal(0.0, Unit::Px)
        }
    }

    /// Skip a comma or slash separator (both used in modern CSS color functions).
    fn skip_sep(&mut self) {
        self.skip_ws_and_comments();
        if matches!(self.peek(), ',' | '/') {
            self.consume();
        }
        self.skip_ws_and_comments();
    }

    /// Parse a 0-255 integer or 0%-100% component of `rgb()`.
    fn parse_color_component(&mut self) -> u8 {
        let s = self.consume_while(|c| c.is_ascii_digit() || c == '.');
        let n: f32 = s.parse().unwrap_or(0.0);
        self.skip_ws_and_comments();
        if self.peek() == '%' {
            self.consume();
            (n * 255.0 / 100.0).round().clamp(0.0, 255.0) as u8
        } else {
            n.round().clamp(0.0, 255.0) as u8
        }
    }

    /// Parse an alpha value: 0–1 float or 0%–100%.
    fn parse_alpha_component(&mut self) -> u8 {
        let s = self.consume_while(|c| c.is_ascii_digit() || c == '.');
        let n: f32 = s.parse().unwrap_or(1.0);
        self.skip_ws_and_comments();
        if self.peek() == '%' {
            self.consume();
            (n * 255.0 / 100.0).round().clamp(0.0, 255.0) as u8
        } else {
            (n * 255.0).round().clamp(0.0, 255.0) as u8
        }
    }

    /// Parse inside `rgb(R G B)` / `rgb(R, G, B)` / `rgba(R, G, B, A)`.
    fn parse_rgb_inner(&mut self) -> Value {
        self.skip_ws_and_comments();
        let r = self.parse_color_component();
        self.skip_sep();
        let g = self.parse_color_component();
        self.skip_sep();
        let b = self.parse_color_component();
        self.skip_ws_and_comments();
        let a = if matches!(self.peek(), ',' | '/') {
            self.skip_sep();
            self.parse_alpha_component()
        } else {
            255u8
        };
        self.consume_while(|c| c != ')');
        if self.peek() == ')' {
            self.consume();
        }
        Value::Color(Color::rgba(r, g, b, a))
    }

    /// Parse inside `hsl(H S% L%)` / `hsla(H, S%, L%, A)`.
    fn parse_hsl_inner(&mut self) -> Value {
        self.skip_ws_and_comments();
        let h_s = self.consume_while(|c| c.is_ascii_digit() || c == '.' || c == '-');
        let h: f32 = h_s.parse().unwrap_or(0.0);
        self.consume_while(char::is_alphabetic); // consume "deg", "turn", etc.
        self.skip_sep();
        let s_s = self.consume_while(|c| c.is_ascii_digit() || c == '.');
        let s: f32 = s_s.parse().unwrap_or(0.0) / 100.0;
        if self.peek() == '%' {
            self.consume();
        }
        self.skip_sep();
        let l_s = self.consume_while(|c| c.is_ascii_digit() || c == '.');
        let l: f32 = l_s.parse().unwrap_or(0.0) / 100.0;
        if self.peek() == '%' {
            self.consume();
        }
        self.skip_ws_and_comments();
        let a = if matches!(self.peek(), ',' | '/') {
            self.skip_sep();
            self.parse_alpha_component()
        } else {
            255u8
        };
        self.consume_while(|c| c != ')');
        if self.peek() == ')' {
            self.consume();
        }
        Value::Color(hsl_to_rgb(h, s, l, a))
    }

    /// Parse the direction / angle at the start of `linear-gradient(…)`.
    fn parse_gradient_angle(&mut self) -> f32 {
        self.skip_ws_and_comments();
        let saved = self.pos;

        if self.peek().is_alphabetic() {
            let kw = self.parse_ident();
            if kw == "to" {
                self.skip_ws_and_comments();
                let d1 = self.parse_ident();
                self.skip_ws_and_comments();
                let d2_saved = self.pos;
                let d2 = if self.peek().is_alphabetic() {
                    let candidate = self.parse_ident();
                    if matches!(candidate.as_str(), "top" | "bottom" | "left" | "right") {
                        candidate
                    } else {
                        self.pos = d2_saved;
                        String::new()
                    }
                } else {
                    String::new()
                };
                self.skip_ws_and_comments();
                if self.peek() == ',' {
                    self.consume();
                }
                return match d1.as_str() {
                    "top" => match d2.as_str() {
                        "right" => 45.0,
                        "left" => 315.0,
                        _ => 0.0,
                    },
                    "bottom" => match d2.as_str() {
                        "right" => 135.0,
                        "left" => 225.0,
                        _ => 180.0,
                    },
                    "right" => match d2.as_str() {
                        "top" => 45.0,
                        "bottom" => 135.0,
                        _ => 90.0,
                    },
                    "left" => match d2.as_str() {
                        "top" => 315.0,
                        "bottom" => 225.0,
                        _ => 270.0,
                    },
                    _ => {
                        self.pos = saved;
                        180.0
                    }
                };
            }
            self.pos = saved;
            return 180.0;
        }

        if self.peek().is_ascii_digit() || matches!(self.peek(), '-' | '+') {
            let mut num_s = String::new();
            if matches!(self.peek(), '+' | '-') {
                num_s.push(self.consume());
            }
            num_s.push_str(&self.consume_while(|c| c.is_ascii_digit() || c == '.'));
            let num: f32 = num_s.parse().unwrap_or(180.0);
            self.consume_while(char::is_alphabetic);
            self.skip_ws_and_comments();
            if self.peek() == ',' {
                self.consume();
            }
            return num;
        }

        180.0
    }

    /// Parse the body of `linear-gradient(…)` after the opening `(` has been consumed.
    fn parse_linear_gradient_inner(&mut self) -> Value {
        let angle_deg = self.parse_gradient_angle();
        let mut stops: Vec<ColorStop> = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if matches!(self.peek(), ')' | '\0') {
                break;
            }

            let color_opt: Option<Color> = match self.peek() {
                '#' => {
                    self.consume();
                    if let Value::Color(c) = self.parse_hex_color() {
                        Some(c)
                    } else {
                        None
                    }
                }
                c if c.is_alphabetic() => {
                    let sp = self.pos;
                    let name = self.parse_ident();
                    if let Some(c) = named_color(&name) {
                        Some(c)
                    } else {
                        self.pos = sp;
                        None
                    }
                }
                _ => None,
            };

            let Some(color) = color_opt else {
                self.consume_while(|c| c != ',' && c != ')');
                if self.peek() == ',' {
                    self.consume();
                }
                continue;
            };

            self.skip_ws_and_comments();
            let position = if self.peek().is_ascii_digit() || self.peek() == '.' {
                let num_s = self.consume_while(|c| c.is_ascii_digit() || c == '.');
                let num: f32 = num_s.parse().unwrap_or(0.0);
                if self.peek() == '%' {
                    self.consume();
                }
                Some(num / 100.0)
            } else {
                None
            };

            stops.push(ColorStop { color, position });
            self.skip_ws_and_comments();
            if self.peek() == ',' {
                self.consume();
            } else if self.peek() == ')' {
                break;
            }
        }

        if self.peek() == ')' {
            self.consume();
        }
        Value::LinearGradient(LinearGradient { angle_deg, stops })
    }

    // ── Declaration parsing ───────────────────────────────────────────────

    fn parse_shorthand_values(&mut self, first: Value) -> Vec<Value> {
        let mut values = vec![first];
        loop {
            self.skip_ws_and_comments();
            if matches!(self.peek(), ';' | '}' | '\0') {
                break;
            }
            if self.peek() == '#'
                || self.peek().is_ascii_digit()
                || self.peek() == '.'
                || self.peek() == '+'
                || self.peek() == '-'
                || self.peek().is_alphabetic()
            {
                values.push(self.parse_value());
            } else {
                break;
            }
        }
        values
    }

    fn parse_declaration(&mut self) -> Vec<Declaration> {
        self.skip_ws_and_comments();
        if self.peek() == '}' || self.eof() {
            return Vec::new();
        }
        let name = self.parse_ident();
        if name.is_empty() {
            self.consume_while(|c| c != ';' && c != '}');
            if self.peek() == ';' {
                self.consume();
            }
            return Vec::new();
        }
        self.skip_ws_and_comments();
        if self.peek() != ':' {
            return Vec::new();
        }
        self.consume();
        self.skip_ws_and_comments();

        // Custom properties (--name: …) store their raw value as a Keyword string.
        // They can contain anything and always inherit.
        if name.starts_with("--") {
            let raw = self
                .consume_while(|c| c != ';' && c != '}')
                .trim()
                .to_string();
            if self.peek() == ';' {
                self.consume();
            }
            return vec![Declaration::new(name, Value::Keyword(raw))];
        }

        let first_value = self.parse_value();
        let decls = match name.as_str() {
            "margin" | "padding" | "border-width" => {
                let values = self.parse_shorthand_values(first_value);
                expand_box_shorthand(&name, values)
            }
            "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
                let values = self.parse_shorthand_values(first_value);
                expand_border_shorthand(&name, values)
            }
            "flex" => {
                let values = self.parse_shorthand_values(first_value);
                expand_flex_shorthand(values)
            }
            "flex-flow" => {
                let values = self.parse_shorthand_values(first_value);
                expand_flex_flow_shorthand(values)
            }
            "gap" | "grid-gap" => {
                let values = self.parse_shorthand_values(first_value);
                expand_gap_shorthand(values)
            }
            "grid-column" | "grid-row" => {
                let mut raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, Unit::Px) => format!("{}px", n),
                    Value::Number(n) => format!("{}", n),
                    _ => String::new(),
                };
                let rest = self.consume_while(|c| c != ';' && c != '!' && c != '}');
                raw.push(' ');
                raw.push_str(&rest);
                expand_grid_placement_shorthand(&name, raw.trim())
            }
            "box-shadow" => {
                let values = self.parse_shorthand_values(first_value);
                let mut lengths = Vec::new();
                let mut color = Color::rgba(0, 0, 0, 128);
                for v in values {
                    match v {
                        Value::Length(n, _) | Value::Number(n) => lengths.push(n),
                        Value::Color(c) => color = c,
                        _ => {}
                    }
                }
                let offset_x = lengths.first().copied().unwrap_or(0.0);
                let offset_y = lengths.get(1).copied().unwrap_or(0.0);
                let blur_radius = lengths.get(2).copied().unwrap_or(0.0);
                vec![Declaration::new(
                    name,
                    Value::BoxShadow(BoxShadow {
                        offset_x,
                        offset_y,
                        blur_radius,
                        color,
                    }),
                )]
            }
            "grid-template-columns" | "grid-template-rows" => {
                let values = self.parse_shorthand_values(first_value);
                let raw_strs: Vec<String> = values
                    .iter()
                    .map(|v| match v {
                        Value::Length(n, Unit::Px) => format!("{}px", n),
                        Value::Length(n, Unit::Fr) => format!("{}fr", n),
                        Value::Length(n, Unit::Em) => format!("{}em", n),
                        Value::Length(n, Unit::Percent) => format!("{}%", n),
                        Value::Number(n) => format!("{}", n),
                        Value::Keyword(s) => s.clone(),
                        _ => "1fr".to_string(),
                    })
                    .collect();
                vec![Declaration::new(name, Value::Keyword(raw_strs.join(" ")))]
            }
            "transform" => {
                let values = self.parse_shorthand_values(first_value);
                let raw_val: String = values
                    .iter()
                    .map(|v| match v {
                        Value::Keyword(s) => s.clone(),
                        Value::Length(n, _) | Value::Number(n) => format!("{}", n),
                        _ => "".to_string(),
                    })
                    .collect::<Vec<String>>()
                    .join(" ");
                let mut tx = 0.0f32;
                let mut ty = 0.0f32;
                let mut scale = 1.0f32;
                if let Some(start) = raw_val.find("translate(") {
                    let inner = &raw_val[start + 10..];
                    if let Some(end) = inner.find(')') {
                        let parts: Vec<&str> = inner[..end].split(',').collect();
                        tx = parts
                            .first()
                            .and_then(|p| p.trim().trim_end_matches("px").parse().ok())
                            .unwrap_or(0.0);
                        ty = parts
                            .get(1)
                            .and_then(|p| p.trim().trim_end_matches("px").parse().ok())
                            .unwrap_or(0.0);
                    }
                }
                if let Some(start) = raw_val.find("scale(") {
                    let inner = &raw_val[start + 6..];
                    if let Some(end) = inner.find(')') {
                        scale = inner[..end].trim().parse().unwrap_or(1.0);
                    }
                }
                vec![Declaration::new(
                    name,
                    Value::Transform(Transform {
                        translate_x: tx,
                        translate_y: ty,
                        scale,
                    }),
                )]
            }
            "transition" => {
                let mut raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, Unit::Px) => format!("{}px", n),
                    Value::Number(n) => format!("{}", n),
                    _ => String::new(),
                };
                let rest = self.consume_while(|c| c != ';' && c != '!' && c != '}');
                raw.push(' ');
                raw.push_str(&rest);
                let specs = parse_transition_value(&raw);
                vec![Declaration::new(name, Value::Transition(specs))]
            }
            "transition-duration" | "transition-delay" => {
                let raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, _) | Value::Number(n) => format!("{}ms", n),
                    _ => String::new(),
                };
                let time_ms = parse_time_to_ms(&raw).unwrap_or(0.0);
                vec![Declaration::new(name, Value::Number(time_ms))]
            }
            "animation" => {
                let mut raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, Unit::Px) => format!("{}px", n),
                    Value::Number(n) => format!("{}", n),
                    _ => String::new(),
                };
                let rest = self.consume_while(|c| c != ';' && c != '!' && c != '}');
                raw.push(' ');
                raw.push_str(&rest);
                let specs = parse_animation_value(&raw);
                vec![Declaration::new(name, Value::Animation(specs))]
            }
            "animation-duration" | "animation-delay" => {
                let raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, _) | Value::Number(n) => format!("{}ms", n),
                    _ => String::new(),
                };
                let time_ms = parse_time_to_ms(&raw).unwrap_or(0.0);
                vec![Declaration::new(name, Value::Number(time_ms))]
            }
            "filter" => {
                let mut raw = match &first_value {
                    Value::Keyword(s) => s.clone(),
                    Value::Length(n, Unit::Px) => format!("{}px", n),
                    Value::Number(n) => format!("{}", n),
                    _ => String::new(),
                };
                let rest = self.consume_while(|c| c != ';' && c != '!' && c != '}');
                raw.push(' ');
                raw.push_str(&rest);
                if let Some(filters) = parse_filter(&raw) {
                    vec![Declaration::new(name, Value::Filter(filters))]
                } else {
                    vec![Declaration::new(name, Value::Keyword(raw.trim().to_string()))]
                }
            }
            _ => vec![Declaration::new(name, first_value)],
        };

        // Whatever is left before the semicolon — this is where `!important`
        // lands, since no value parser consumes `!`.
        let trailing = self.consume_while(|c| c != ';' && c != '}');
        if self.peek() == ';' {
            self.consume();
        }

        let important = trailing.trim_start().starts_with('!')
            && trailing.to_ascii_lowercase().contains("important");
        if important {
            return decls
                .into_iter()
                .map(|d| Declaration {
                    important: true,
                    ..d
                })
                .collect();
        }
        decls
    }

    fn parse_declarations(&mut self) -> Vec<Declaration> {
        self.skip_ws_and_comments();
        if self.peek() == '{' {
            self.consume();
        }
        let mut decls = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == '}' || self.eof() {
                break;
            }
            decls.extend(self.parse_declaration());
        }
        if self.peek() == '}' {
            self.consume();
        }
        decls
    }

    // ── Rule / stylesheet parsing ─────────────────────────────────────────

    /// Parse one stylesheet item, returning 0 or more rules (multiple for @media).
    fn parse_rule_item(&mut self) -> Vec<Rule> {
        self.skip_ws_and_comments();
        if self.eof() {
            return Vec::new();
        }

        if self.peek() == '@' {
            self.consume(); // '@'
            let name = self.parse_ident().to_ascii_lowercase();
            return match name.as_str() {
                "media" => self.parse_media_block(),
                "keyframes" | "-webkit-keyframes" => {
                    self.parse_keyframe_block();
                    Vec::new()
                }
                _ => {
                    // Skip other @-rules (charset, import, …)
                    self.consume_while(|c| c != '{' && c != ';');
                    if self.peek() == '{' {
                        self.skip_brace_block();
                    } else if self.peek() == ';' {
                        self.consume();
                    }
                    Vec::new()
                }
            };
        }

        let selectors = self.parse_selectors();
        let declarations = self.parse_declarations();
        if selectors.is_empty() {
            Vec::new()
        } else {
            vec![Rule {
                selectors,
                declarations,
                media_query: None,
            }]
        }
    }

    /// Parse the content of `@media … { rules }`.
    fn parse_media_block(&mut self) -> Vec<Rule> {
        // Parse the media condition, returning None if the media type is non-screen.
        let conditions_opt = self.parse_media_conditions();

        self.skip_ws_and_comments();
        if self.peek() != '{' {
            return Vec::new();
        }

        if conditions_opt.is_none() {
            // Non-screen media type (e.g. "print") — skip the whole block.
            self.skip_brace_block();
            return Vec::new();
        }

        self.consume(); // '{'
        let conditions = conditions_opt.unwrap();

        let mut inner_rules = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == '}' || self.eof() {
                break;
            }
            inner_rules.extend(self.parse_rule_item());
        }
        if self.peek() == '}' {
            self.consume();
        }

        if conditions.is_empty() {
            // `@media screen { … }` or `@media { … }` — unconditional.
            inner_rules
        } else {
            let mq = MediaQuery { conditions };
            for rule in &mut inner_rules {
                rule.media_query = Some(mq.clone());
            }
            inner_rules
        }
    }

    /// Parse media conditions up to (but not including) the `{`.
    /// Returns `None` if a non-screen media type is detected.
    fn parse_media_conditions(&mut self) -> Option<Vec<MediaCondition>> {
        let mut conditions = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == '{' || self.eof() {
                break;
            }

            if self.peek().is_alphabetic() {
                let word = self.parse_ident();
                match word.to_ascii_lowercase().as_str() {
                    "and" | "or" | "not" | "only" => continue,
                    "screen" | "all" => continue,
                    _ => return None, // non-screen media type, e.g. "print"
                }
            }

            if self.peek() == '(' {
                self.consume(); // '('
                self.skip_ws_and_comments();
                let prop = self.parse_ident();
                self.skip_ws_and_comments();
                if self.peek() == ':' {
                    self.consume();
                    self.skip_ws_and_comments();
                    let val = match self.parse_length() {
                        Value::Length(n, _) | Value::Number(n) => n,
                        _ => 0.0,
                    };
                    let cond = match prop.to_ascii_lowercase().as_str() {
                        "min-width" => Some(MediaCondition::MinWidth(val)),
                        "max-width" => Some(MediaCondition::MaxWidth(val)),
                        "min-height" => Some(MediaCondition::MinHeight(val)),
                        "max-height" => Some(MediaCondition::MaxHeight(val)),
                        _ => None,
                    };
                    if let Some(c) = cond {
                        conditions.push(c);
                    }
                }
                self.consume_while(|c| c != ')');
                if self.peek() == ')' {
                    self.consume();
                }
            } else {
                break;
            }
        }
        Some(conditions)
    }

    fn parse_keyframe_block(&mut self) {
        self.skip_ws_and_comments();
        let name = self.parse_ident();
        self.skip_ws_and_comments();
        if self.peek() != '{' {
            return;
        }
        self.consume(); // '{'
        let mut steps = Vec::new();

        loop {
            self.skip_ws_and_comments();
            if self.peek() == '}' || self.eof() {
                break;
            }

            let mut offsets = Vec::new();
            loop {
                self.skip_ws_and_comments();
                if self.peek() == '{' || self.eof() {
                    break;
                }
                let token = self.consume_while(|c| c != ',' && c != '{' && !c.is_whitespace());
                let offset = match token.to_ascii_lowercase().as_str() {
                    "from" => Some(0.0),
                    "to" => Some(1.0),
                    s if s.ends_with('%') => s
                        .trim_end_matches('%')
                        .parse::<f32>()
                        .ok()
                        .map(|p| (p / 100.0).clamp(0.0, 1.0)),
                    _ => None,
                };
                if let Some(off) = offset {
                    offsets.push(off);
                }
                self.skip_ws_and_comments();
                if self.peek() == ',' {
                    self.consume();
                } else {
                    break;
                }
            }

            let decls = self.parse_declarations();
            for off in offsets {
                steps.push(KeyframeStep {
                    offset: off,
                    declarations: decls.clone(),
                });
            }
        }

        if self.peek() == '}' {
            self.consume();
        }

        steps.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
        if !name.is_empty() {
            self.keyframes.insert(name.clone(), KeyframeRule { name, steps });
        }
    }

    fn parse_stylesheet(&mut self) -> Stylesheet {
        let mut rules = Vec::new();
        while !self.eof() {
            rules.extend(self.parse_rule_item());
        }
        Stylesheet {
            rules,
            keyframes: self.keyframes.clone(),
        }
    }
}

// ── Shorthand expansion ───────────────────────────────────────────────────────

fn expand_box_shorthand(name: &str, values: Vec<Value>) -> Vec<Declaration> {
    let (top, right, bottom, left) = match name {
        "margin" => ("margin-top", "margin-right", "margin-bottom", "margin-left"),
        "padding" => (
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ),
        "border-width" => (
            "border-top-width",
            "border-right-width",
            "border-bottom-width",
            "border-left-width",
        ),
        _ => return Vec::new(),
    };
    let (tv, rv, bv, lv) = match values.len() {
        0 => return Vec::new(),
        1 => (
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
        ),
        2 => (
            values[0].clone(),
            values[1].clone(),
            values[0].clone(),
            values[1].clone(),
        ),
        3 => (
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[1].clone(),
        ),
        _ => (
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
        ),
    };
    vec![
        Declaration::new(top, tv),
        Declaration::new(right, rv),
        Declaration::new(bottom, bv),
        Declaration::new(left, lv),
    ]
}

/// `flex: <grow> <shrink> <basis>` — plus the `flex: 1`, `flex: auto`,
/// `flex: none` and `flex: 0 200px` forms.
fn expand_flex_shorthand(values: Vec<Value>) -> Vec<Declaration> {
    let decl = |name: &str, value: Value| Declaration::new(name, value);

    // Keyword forms first.
    if let Some(Value::Keyword(k)) = values.first() {
        match k.as_str() {
            "none" => {
                return vec![
                    decl("flex-grow", Value::Number(0.0)),
                    decl("flex-shrink", Value::Number(0.0)),
                    decl("flex-basis", Value::Keyword("auto".into())),
                ];
            }
            "auto" | "initial" => {
                let grow = if k == "auto" { 1.0 } else { 0.0 };
                return vec![
                    decl("flex-grow", Value::Number(grow)),
                    decl("flex-shrink", Value::Number(1.0)),
                    decl("flex-basis", Value::Keyword("auto".into())),
                ];
            }
            _ => {}
        }
    }

    // Numbers are grow/shrink; a length (or `auto`) is the basis.
    let mut numbers: Vec<f32> = Vec::new();
    let mut basis: Option<Value> = None;
    for value in &values {
        match value {
            Value::Number(n) => numbers.push(*n),
            Value::Length(..) | Value::Calc(_) => basis = Some(value.clone()),
            Value::Keyword(k) if k == "auto" => basis = Some(value.clone()),
            _ => {}
        }
    }

    let grow = numbers.first().copied().unwrap_or(1.0);
    let shrink = numbers.get(1).copied().unwrap_or(1.0);
    // `flex: 1` means basis 0, not auto — that is what makes items share space evenly.
    let basis = basis.unwrap_or(Value::Length(0.0, Unit::Px));

    vec![
        decl("flex-grow", Value::Number(grow)),
        decl("flex-shrink", Value::Number(shrink)),
        decl("flex-basis", basis),
    ]
}

/// `flex-flow: <flex-direction> || <flex-wrap>`
fn expand_flex_flow_shorthand(values: Vec<Value>) -> Vec<Declaration> {
    let mut direction: Option<Value> = None;
    let mut wrap: Option<Value> = None;

    for val in values {
        if let Value::Keyword(k) = &val {
            match k.as_str() {
                "row" | "row-reverse" | "column" | "column-reverse" => {
                    direction = Some(val.clone());
                }
                "nowrap" | "wrap" | "wrap-reverse" => {
                    wrap = Some(val.clone());
                }
                _ => {}
            }
        }
    }

    let mut decls = Vec::new();
    if let Some(d) = direction {
        decls.push(Declaration::new("flex-direction", d));
    }
    if let Some(w) = wrap {
        decls.push(Declaration::new("flex-wrap", w));
    }
    decls
}

/// `gap: <row> <column>` (one value sets both).
fn expand_gap_shorthand(values: Vec<Value>) -> Vec<Declaration> {
    let Some(row) = values.first().cloned() else {
        return Vec::new();
    };
    let column = values.get(1).cloned().unwrap_or_else(|| row.clone());
    vec![
        Declaration::new("row-gap", row),
        Declaration::new("column-gap", column),
    ]
}

/// `grid-column: <start> [ / <end> ]` and `grid-row: <start> [ / <end> ]`.
fn expand_grid_placement_shorthand(name: &str, raw: &str) -> Vec<Declaration> {
    let (start_prop, end_prop) = if name == "grid-column" {
        ("grid-column-start", "grid-column-end")
    } else {
        ("grid-row-start", "grid-row-end")
    };
    let parts: Vec<&str> = raw.split('/').map(|s| s.trim()).collect();
    if parts.len() == 2 {
        vec![
            Declaration::new(start_prop, Value::Keyword(parts[0].to_string())),
            Declaration::new(end_prop, Value::Keyword(parts[1].to_string())),
        ]
    } else if parts.len() == 1 {
        let val = parts[0];
        if val.starts_with("span") {
            vec![
                Declaration::new(start_prop, Value::Keyword("auto".to_string())),
                Declaration::new(end_prop, Value::Keyword(val.to_string())),
            ]
        } else {
            vec![
                Declaration::new(start_prop, Value::Keyword(val.to_string())),
                Declaration::new(end_prop, Value::Keyword("auto".to_string())),
            ]
        }
    } else {
        vec![Declaration::new(name, Value::Keyword(raw.to_string()))]
    }
}

/// Expands `repeat(count, track...)` expressions and returns individual track definitions.
pub fn expand_grid_template_tracks(spec: &str) -> Vec<String> {
    let mut tracks = Vec::new();
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        let mut token = String::new();
        while i < chars.len() {
            let ch = chars[i];
            if ch.is_whitespace() {
                break;
            }
            if ch == '(' {
                token.push(ch);
                i += 1;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    let c = chars[i];
                    token.push(c);
                    if c == '(' {
                        depth += 1;
                    } else if c == ')' {
                        depth -= 1;
                    }
                    i += 1;
                }
            } else {
                token.push(ch);
                i += 1;
            }
        }

        let token_trim = token.trim();
        if token_trim.starts_with("repeat(") && token_trim.ends_with(')') {
            let inner = &token_trim[7..token_trim.len() - 1];
            if let Some((count_str, track_spec)) = inner.split_once(',') {
                let count: usize = count_str.trim().parse().unwrap_or(1);
                let sub_tracks = expand_grid_template_tracks(track_spec.trim());
                for _ in 0..count {
                    tracks.extend(sub_tracks.clone());
                }
            } else {
                tracks.push(token_trim.to_string());
            }
        } else if !token_trim.is_empty() {
            tracks.push(token_trim.to_string());
        }
    }

    if tracks.is_empty() {
        tracks.push("1fr".to_string());
    }
    tracks
}

// ── Border shorthand expansion ────────────────────────────────────────────────

const BORDER_STYLE_KEYWORDS: &[&str] = &[
    "none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset",
];

/// Expand `border`, `border-top`, etc. into individual longhands.
/// Each value in the list is classified as width, style, or color based on its type.
fn expand_border_shorthand(name: &str, values: Vec<Value>) -> Vec<Declaration> {
    type SideNames = (&'static str, &'static str, &'static str);
    let sides: &[SideNames] = match name {
        "border" => &[
            ("border-top-width", "border-top-style", "border-top-color"),
            (
                "border-right-width",
                "border-right-style",
                "border-right-color",
            ),
            (
                "border-bottom-width",
                "border-bottom-style",
                "border-bottom-color",
            ),
            (
                "border-left-width",
                "border-left-style",
                "border-left-color",
            ),
        ],
        "border-top" => &[("border-top-width", "border-top-style", "border-top-color")],
        "border-right" => &[(
            "border-right-width",
            "border-right-style",
            "border-right-color",
        )],
        "border-bottom" => &[(
            "border-bottom-width",
            "border-bottom-style",
            "border-bottom-color",
        )],
        "border-left" => &[(
            "border-left-width",
            "border-left-style",
            "border-left-color",
        )],
        _ => return Vec::new(),
    };

    let mut width: Option<Value> = None;
    let mut style: Option<Value> = None;
    let mut color: Option<Value> = None;

    for val in values {
        match &val {
            Value::Length(_, _) | Value::Number(_) => {
                width.get_or_insert(val);
            }
            Value::Color(_) => {
                color.get_or_insert(val);
            }
            Value::Keyword(kw) if BORDER_STYLE_KEYWORDS.contains(&kw.as_str()) => {
                style.get_or_insert(val);
            }
            _ => {}
        }
    }

    let mut decls = Vec::new();
    for &(w_name, s_name, c_name) in sides {
        if let Some(ref v) = width {
            decls.push(Declaration::new(w_name, v.clone()));
        }
        if let Some(ref v) = style {
            decls.push(Declaration::new(s_name, v.clone()));
        }
        if let Some(ref v) = color {
            decls.push(Declaration::new(c_name, v.clone()));
        }
    }
    decls
}

// ── HSL → RGB conversion ──────────────────────────────────────────────────────

fn hsl_to_rgb(h: f32, s: f32, l: f32, a: u8) -> Color {
    if s < 1e-6 {
        let v = (l * 255.0).round() as u8;
        return Color::rgba(v, v, v, a);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h / 360.0;
    Color::rgba(
        (hue_channel(p, q, h + 1.0 / 3.0) * 255.0).round() as u8,
        (hue_channel(p, q, h) * 255.0).round() as u8,
        (hue_channel(p, q, h - 1.0 / 3.0) * 255.0).round() as u8,
        a,
    )
}

fn hue_channel(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

// ── Named color table ─────────────────────────────────────────────────────────

pub fn named_color(name: &str) -> Option<Color> {
    Some(match name {
        "transparent" => Color::transparent(),
        "black" => Color::rgb(0, 0, 0),
        "white" => Color::rgb(255, 255, 255),
        "red" => Color::rgb(255, 0, 0),
        "green" => Color::rgb(0, 128, 0),
        "lime" => Color::rgb(0, 255, 0),
        "blue" => Color::rgb(0, 0, 255),
        "yellow" => Color::rgb(255, 255, 0),
        "orange" => Color::rgb(255, 165, 0),
        "pink" => Color::rgb(255, 192, 203),
        "purple" => Color::rgb(128, 0, 128),
        "cyan" => Color::rgb(0, 255, 255),
        "magenta" => Color::rgb(255, 0, 255),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "silver" => Color::rgb(192, 192, 192),
        "navy" => Color::rgb(0, 0, 128),
        "teal" => Color::rgb(0, 128, 128),
        "maroon" => Color::rgb(128, 0, 0),
        "olive" => Color::rgb(128, 128, 0),
        "coral" => Color::rgb(255, 127, 80),
        "salmon" => Color::rgb(250, 128, 114),
        "khaki" => Color::rgb(240, 230, 140),
        "indigo" => Color::rgb(75, 0, 130),
        "violet" => Color::rgb(238, 130, 238),
        _ => return None,
    })
}

pub fn parse_time_to_ms(s: &str) -> Option<f32> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(rest) = s.strip_suffix("ms") {
        rest.trim().parse::<f32>().ok()
    } else if let Some(rest) = s.strip_suffix('s') {
        rest.trim().parse::<f32>().ok().map(|secs| secs * 1000.0)
    } else {
        s.parse::<f32>().ok()
    }
}

pub fn parse_timing_func(s: &str) -> Option<TimingFunction> {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "linear" => Some(TimingFunction::Linear),
        "ease" => Some(TimingFunction::Ease),
        "ease-in" => Some(TimingFunction::EaseIn),
        "ease-out" => Some(TimingFunction::EaseOut),
        "ease-in-out" => Some(TimingFunction::EaseInOut),
        _ if s.starts_with("cubic-bezier(") && s.ends_with(')') => {
            let inner = &s[13..s.len() - 1];
            let parts: Vec<f32> = inner
                .split(',')
                .filter_map(|p| p.trim().parse::<f32>().ok())
                .collect();
            if parts.len() == 4 {
                Some(TimingFunction::CubicBezier(parts[0], parts[1], parts[2], parts[3]))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn parse_transition_value(raw: &str) -> Vec<TransitionSpec> {
    let mut specs = Vec::new();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in raw.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    for part in parts {
        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut p_depth = 0;
        for c in part.chars() {
            if c == '(' {
                p_depth += 1;
                cur.push(c);
            } else if c == ')' {
                if p_depth > 0 {
                    p_depth -= 1;
                }
                cur.push(c);
            } else if c.is_whitespace() && p_depth == 0 {
                if !cur.is_empty() {
                    tokens.push(cur.clone());
                    cur.clear();
                }
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }

        if tokens.is_empty() {
            continue;
        }
        let mut prop = "all".to_string();
        let mut duration_ms = 0.0f32;
        let mut delay_ms = 0.0f32;
        let mut timing_fn = TimingFunction::Ease;
        let mut found_duration = false;

        for token in &tokens {
            if let Some(time) = parse_time_to_ms(token) {
                if token.ends_with('s') || token.ends_with("ms") || token.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    if !found_duration {
                        duration_ms = time;
                        found_duration = true;
                    } else {
                        delay_ms = time;
                    }
                }
            } else if let Some(tf) = parse_timing_func(token) {
                timing_fn = tf;
            } else if !token.is_empty() {
                prop = token.to_ascii_lowercase();
            }
        }

        specs.push(TransitionSpec {
            property: prop,
            duration_ms,
            timing_function: timing_fn,
            delay_ms,
        });
    }

    if specs.is_empty() {
        specs.push(TransitionSpec {
            property: "all".to_string(),
            duration_ms: 0.0,
            timing_function: TimingFunction::Ease,
            delay_ms: 0.0,
        });
    }

    specs
}

pub fn parse_animation_direction(s: &str) -> Option<AnimationDirection> {
    match s.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(AnimationDirection::Normal),
        "reverse" => Some(AnimationDirection::Reverse),
        "alternate" => Some(AnimationDirection::Alternate),
        "alternate-reverse" => Some(AnimationDirection::AlternateReverse),
        _ => None,
    }
}

pub fn parse_animation_fill_mode(s: &str) -> Option<AnimationFillMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "none" => Some(AnimationFillMode::None),
        "forwards" => Some(AnimationFillMode::Forwards),
        "backwards" => Some(AnimationFillMode::Backwards),
        "both" => Some(AnimationFillMode::Both),
        _ => None,
    }
}

pub fn parse_animation_iteration_count(s: &str) -> Option<AnimationIterationCount> {
    let s = s.trim().to_ascii_lowercase();
    if s == "infinite" {
        Some(AnimationIterationCount::Infinite)
    } else if let Ok(n) = s.parse::<f32>() {
        Some(AnimationIterationCount::Finite(n))
    } else {
        None
    }
}

pub fn parse_animation_value(raw: &str) -> Vec<AnimationSpec> {
    let mut specs = Vec::new();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in raw.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    for part in parts {
        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut p_depth = 0;
        for c in part.chars() {
            if c == '(' {
                p_depth += 1;
                cur.push(c);
            } else if c == ')' {
                if p_depth > 0 {
                    p_depth -= 1;
                }
                cur.push(c);
            } else if c.is_whitespace() && p_depth == 0 {
                if !cur.is_empty() {
                    tokens.push(cur.clone());
                    cur.clear();
                }
            } else {
                cur.push(c);
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }

        if tokens.is_empty() {
            continue;
        }

        let mut name = String::new();
        let mut duration_ms = 0.0f32;
        let mut delay_ms = 0.0f32;
        let mut timing_fn = TimingFunction::Ease;
        let mut iteration_count = AnimationIterationCount::Finite(1.0);
        let mut direction = AnimationDirection::Normal;
        let mut fill_mode = AnimationFillMode::None;
        let mut found_duration = false;

        for token in &tokens {
            if let Some(time) = parse_time_to_ms(token) {
                if token.ends_with('s') || token.ends_with("ms") {
                    if !found_duration {
                        duration_ms = time;
                        found_duration = true;
                    } else {
                        delay_ms = time;
                    }
                    continue;
                }
            }
            if let Some(tf) = parse_timing_func(token) {
                timing_fn = tf;
            } else if let Some(dir) = parse_animation_direction(token) {
                direction = dir;
            } else if let Some(fm) = parse_animation_fill_mode(token) {
                fill_mode = fm;
            } else if let Some(ic) = parse_animation_iteration_count(token) {
                iteration_count = ic;
            } else if !token.is_empty() {
                name = token.clone();
            }
        }

        if !name.is_empty() {
            specs.push(AnimationSpec {
                name,
                duration_ms,
                timing_function: timing_fn,
                delay_ms,
                iteration_count,
                direction,
                fill_mode,
            });
        }
    }

    specs
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn parse_css(input: &str) -> Stylesheet {
    Parser::new(input).parse_stylesheet()
}

/// Parse a single CSS value expression from a raw string (used for custom property resolution).
pub fn parse_single_value(s: &str) -> Value {
    Parser::new(s.trim()).parse_value()
}

/// Parse a CSS color string (#hex, rgb/rgba, hsl/hsla, named color).
pub fn parse_color(input: &str) -> Option<Color> {
    match Parser::new(input.trim()).parse_value() {
        Value::Color(c) => Some(c),
        _ => None,
    }
}

/// Parse a bare declaration block — `"color: red; margin: 0"` — with no
/// surrounding selector or braces. Used for `style="…"` attributes.
pub fn parse_declaration_block(input: &str) -> Vec<Declaration> {
    Parser::new(input).parse_declarations()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_rule() {
        let ss = parse_css("p { color: red; }");
        assert_eq!(ss.rules.len(), 1);
        let rule = &ss.rules[0];
        assert_eq!(rule.selectors[0].parts[0].tag_name.as_deref(), Some("p"));
        assert_eq!(rule.declarations[0].name, "color");
        assert_eq!(
            rule.declarations[0].value,
            Value::Color(Color::rgb(255, 0, 0))
        );
    }

    #[test]
    fn hex_color() {
        let ss = parse_css("div { background-color: #1a2b3c; }");
        let v = &ss.rules[0].declarations[0].value;
        assert_eq!(*v, Value::Color(Color::rgb(0x1a, 0x2b, 0x3c)));
    }

    #[test]
    fn length_px() {
        let ss = parse_css("div { width: 100px; }");
        assert_eq!(
            ss.rules[0].declarations[0].value,
            Value::Length(100.0, Unit::Px)
        );
    }

    #[test]
    fn signed_length_px() {
        let ss = parse_css("div { margin-top: -100px; }");
        assert_eq!(
            ss.rules[0].declarations[0].value,
            Value::Length(-100.0, Unit::Px)
        );
    }

    #[test]
    fn id_and_class_selector() {
        let ss = parse_css("#main.hero { color: blue; }");
        let part = &ss.rules[0].selectors[0].parts[0];
        assert_eq!(part.id.as_deref(), Some("main"));
        assert_eq!(part.classes[0], "hero");
    }

    #[test]
    fn grouped_selectors() {
        let ss = parse_css("h1, h2, h3 { font-weight: bold; }");
        assert_eq!(ss.rules[0].selectors.len(), 3);
    }

    #[test]
    fn comment_skipped() {
        let ss = parse_css("/* heading */ h1 { color: black; /* dark */ }");
        assert_eq!(ss.rules.len(), 1);
    }

    #[test]
    fn at_rule_skipped() {
        // @media print is non-screen → 0 rules; p rule stays.
        let ss = parse_css("@media print { h1 { color: black; } } p { color: red; }");
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(
            ss.rules[0].selectors[0].parts[0].tag_name.as_deref(),
            Some("p")
        );
    }

    #[test]
    fn specificity_order() {
        let ss = parse_css("p, #id, .cls { color: red; }");
        let sels = &ss.rules[0].selectors;
        assert_eq!(sels[0].parts[0].id.as_deref(), Some("id"));
        assert!(!sels[1].parts[0].classes.is_empty());
        assert_eq!(sels[2].parts[0].tag_name.as_deref(), Some("p"));
    }

    #[test]
    fn linear_gradient_to_right_parsed() {
        let ss = parse_css("div { background-image: linear-gradient(to right, red, blue); }");
        assert_eq!(ss.rules[0].declarations[0].name, "background-image");
        if let Value::LinearGradient(g) = &ss.rules[0].declarations[0].value {
            assert!((g.angle_deg - 90.0).abs() < 0.01);
            assert_eq!(g.stops.len(), 2);
            assert_eq!(g.stops[0].color, Color::rgb(255, 0, 0));
            assert_eq!(g.stops[1].color, Color::rgb(0, 0, 255));
        } else {
            panic!("expected LinearGradient");
        }
    }

    #[test]
    fn linear_gradient_angle_deg_parsed() {
        let ss = parse_css("div { background: linear-gradient(45deg, #ffffff, #000000); }");
        if let Value::LinearGradient(g) = &ss.rules[0].declarations[0].value {
            assert!((g.angle_deg - 45.0).abs() < 0.01);
            assert_eq!(g.stops.len(), 2);
        } else {
            panic!("expected LinearGradient");
        }
    }

    #[test]
    fn linear_gradient_three_stops() {
        let ss = parse_css(
            "div { background-image: linear-gradient(to bottom, red, green 50%, blue); }",
        );
        if let Value::LinearGradient(g) = &ss.rules[0].declarations[0].value {
            assert!((g.angle_deg - 180.0).abs() < 0.01);
            assert_eq!(g.stops.len(), 3);
            assert_eq!(g.stops[1].color, Color::rgb(0, 128, 0));
            assert!((g.stops[1].position.unwrap() - 0.5).abs() < 0.01);
        } else {
            panic!("expected LinearGradient");
        }
    }

    #[test]
    fn descendant_selector_parsed() {
        let ss = parse_css("footer p { color: red; }");
        assert_eq!(ss.rules.len(), 1);
        let parts = &ss.rules[0].selectors[0].parts;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].tag_name.as_deref(), Some("footer"));
        assert_eq!(parts[0].combinator, Combinator::Root);
        assert_eq!(parts[1].tag_name.as_deref(), Some("p"));
        assert_eq!(parts[1].combinator, Combinator::Descendant);
    }

    #[test]
    fn child_selector_parsed() {
        let ss = parse_css("nav > a { color: blue; }");
        assert_eq!(ss.rules.len(), 1);
        let parts = &ss.rules[0].selectors[0].parts;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].tag_name.as_deref(), Some("nav"));
        assert_eq!(parts[1].tag_name.as_deref(), Some("a"));
        assert_eq!(parts[1].combinator, Combinator::Child);
    }

    #[test]
    fn descendant_selector_specificity() {
        let ss = parse_css("div p, #id p { color: red; }");
        let sels = &ss.rules[0].selectors;
        assert_eq!(sels[0].parts[0].id.as_deref(), Some("id"));
        assert_eq!(sels[1].parts[0].tag_name.as_deref(), Some("div"));
    }

    // ── Pseudo-class tests ────────────────────────────────────────────────

    #[test]
    fn first_child_parsed() {
        let ss = parse_css("li:first-child { color: red; }");
        let part = &ss.rules[0].selectors[0].parts[0];
        assert_eq!(part.tag_name.as_deref(), Some("li"));
        assert_eq!(part.pseudo_classes, vec![PseudoClass::FirstChild]);
    }

    #[test]
    fn nth_child_odd_parsed() {
        let ss = parse_css("li:nth-child(odd) { color: red; }");
        let part = &ss.rules[0].selectors[0].parts[0];
        assert_eq!(
            part.pseudo_classes[0],
            PseudoClass::NthChild(NthExpr { a: 2, b: 1 })
        );
    }

    #[test]
    fn nth_child_2n_plus_1_parsed() {
        let ss = parse_css("li:nth-child(2n+1) { color: red; }");
        let part = &ss.rules[0].selectors[0].parts[0];
        assert_eq!(
            part.pseudo_classes[0],
            PseudoClass::NthChild(NthExpr { a: 2, b: 1 })
        );
    }

    #[test]
    fn not_pseudo_parsed() {
        let ss = parse_css("li:not(.special) { color: red; }");
        let part = &ss.rules[0].selectors[0].parts[0];
        if let PseudoClass::Not(inner) = &part.pseudo_classes[0] {
            assert_eq!(inner.classes, vec!["special".to_string()]);
        } else {
            panic!("expected Not pseudo-class");
        }
    }

    #[test]
    fn nth_expr_matches() {
        let odd = NthExpr { a: 2, b: 1 };
        assert!(odd.matches(1));
        assert!(!odd.matches(2));
        assert!(odd.matches(3));
        assert!(!odd.matches(4));
        assert!(odd.matches(5));

        let nth3 = NthExpr { a: 0, b: 3 };
        assert!(!nth3.matches(2));
        assert!(nth3.matches(3));
        assert!(!nth3.matches(4));

        let neg = NthExpr { a: -1, b: 3 }; // :nth-child(-n+3) = first 3
        assert!(neg.matches(1));
        assert!(neg.matches(2));
        assert!(neg.matches(3));
        assert!(!neg.matches(4));
    }

    // ── CSS variable tests ────────────────────────────────────────────────

    #[test]
    fn var_function_parsed() {
        let ss = parse_css("div { color: var(--primary); }");
        let val = &ss.rules[0].declarations[0].value;
        assert_eq!(
            *val,
            Value::Var {
                name: "--primary".into(),
                fallback: None
            }
        );
    }

    #[test]
    fn var_function_with_fallback_parsed() {
        let ss = parse_css("div { color: var(--primary, blue); }");
        let val = &ss.rules[0].declarations[0].value;
        assert_eq!(
            *val,
            Value::Var {
                name: "--primary".into(),
                fallback: Some(Box::new(Value::Color(Color::rgb(0, 0, 255)))),
            }
        );
    }

    #[test]
    fn custom_property_stored_as_keyword() {
        let ss = parse_css(":root { --primary: red; }");
        let val = &ss.rules[0].declarations[0].value;
        assert_eq!(*val, Value::Keyword("red".into()));
    }

    // ── @media tests ──────────────────────────────────────────────────────

    #[test]
    fn media_max_width_parsed() {
        let ss = parse_css("@media (max-width: 600px) { div { color: red; } }");
        assert_eq!(ss.rules.len(), 1);
        let mq = ss.rules[0]
            .media_query
            .as_ref()
            .expect("media_query should be Some");
        assert_eq!(mq.conditions.len(), 1);
        assert!(matches!(mq.conditions[0], MediaCondition::MaxWidth(w) if (w - 600.0).abs() < 0.1));
    }

    #[test]
    fn media_screen_and_condition_parsed() {
        let ss = parse_css("@media screen and (min-width: 768px) { p { color: blue; } }");
        assert_eq!(ss.rules.len(), 1);
        let mq = ss.rules[0].media_query.as_ref().unwrap();
        assert!(matches!(mq.conditions[0], MediaCondition::MinWidth(w) if (w - 768.0).abs() < 0.1));
    }

    #[test]
    fn media_print_skipped() {
        let ss = parse_css("@media print { div { color: red; } } p { color: blue; }");
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(
            ss.rules[0].selectors[0].parts[0].tag_name.as_deref(),
            Some("p")
        );
    }

    #[test]
    fn media_query_matches_viewport() {
        let mq = MediaQuery {
            conditions: vec![MediaCondition::MaxWidth(600.0)],
        };
        assert!(mq.matches(400.0, 800.0));
        assert!(mq.matches(600.0, 800.0));
        assert!(!mq.matches(601.0, 800.0));
    }

    // ── Sibling selector tests ────────────────────────────────────────────

    #[test]
    fn adjacent_sibling_parsed() {
        let ss = parse_css("h2 + p { color: red; }");
        let parts = &ss.rules[0].selectors[0].parts;
        assert_eq!(parts[0].tag_name.as_deref(), Some("h2"));
        assert_eq!(parts[1].tag_name.as_deref(), Some("p"));
        assert_eq!(parts[1].combinator, Combinator::AdjacentSibling);
    }

    #[test]
    fn general_sibling_parsed() {
        let ss = parse_css("h2 ~ p { color: blue; }");
        let parts = &ss.rules[0].selectors[0].parts;
        assert_eq!(parts[1].combinator, Combinator::GeneralSibling);
    }

    // ── rgb() / hsl() tests ───────────────────────────────────────────────

    #[test]
    fn rgb_function_parsed() {
        let ss = parse_css("div { color: rgb(255, 0, 128); }");
        assert_eq!(
            ss.rules[0].declarations[0].value,
            Value::Color(Color::rgba(255, 0, 128, 255))
        );
    }

    #[test]
    fn rgba_function_parsed() {
        let ss = parse_css("div { color: rgba(0, 128, 255, 0.5); }");
        if let Value::Color(c) = ss.rules[0].declarations[0].value {
            assert_eq!(c.r, 0);
            assert_eq!(c.g, 128);
            assert_eq!(c.b, 255);
            assert!((c.a as f32 - 127.5).abs() < 2.0); // 0.5 * 255 ≈ 128
        } else {
            panic!("expected Color");
        }
    }

    #[test]
    fn rgb_percent_parsed() {
        let ss = parse_css("div { color: rgb(100%, 0%, 50%); }");
        if let Value::Color(c) = ss.rules[0].declarations[0].value {
            assert_eq!(c.r, 255);
            assert_eq!(c.g, 0);
            assert!((c.b as i32 - 128).abs() <= 1);
        } else {
            panic!("expected Color");
        }
    }

    #[test]
    fn hsl_function_red() {
        // hsl(0, 100%, 50%) = red
        let ss = parse_css("div { color: hsl(0, 100%, 50%); }");
        if let Value::Color(c) = ss.rules[0].declarations[0].value {
            assert_eq!(c.r, 255);
            assert_eq!(c.g, 0);
            assert_eq!(c.b, 0);
        } else {
            panic!("expected Color");
        }
    }

    #[test]
    fn hsl_function_blue() {
        // hsl(240, 100%, 50%) = blue
        let ss = parse_css("div { color: hsl(240, 100%, 50%); }");
        if let Value::Color(c) = ss.rules[0].declarations[0].value {
            assert_eq!(c.r, 0);
            assert_eq!(c.g, 0);
            assert_eq!(c.b, 255);
        } else {
            panic!("expected Color");
        }
    }

    // ── border shorthand tests ────────────────────────────────────────────

    #[test]
    fn border_shorthand_expands() {
        let ss = parse_css("div { border: 2px solid red; }");
        let decls = &ss.rules[0].declarations;
        let has = |name: &str, val: Value| decls.iter().any(|d| d.name == name && d.value == val);
        assert!(has("border-top-width", Value::Length(2.0, Unit::Px)));
        assert!(has("border-right-width", Value::Length(2.0, Unit::Px)));
        assert!(has("border-bottom-width", Value::Length(2.0, Unit::Px)));
        assert!(has("border-left-width", Value::Length(2.0, Unit::Px)));
        assert!(has("border-top-color", Value::Color(Color::rgb(255, 0, 0))));
        assert!(has("border-top-style", Value::Keyword("solid".into())));
    }

    #[test]
    fn border_top_shorthand_expands() {
        let ss = parse_css("div { border-top: 1px dashed blue; }");
        let decls = &ss.rules[0].declarations;
        assert!(decls
            .iter()
            .any(|d| d.name == "border-top-width" && d.value == Value::Length(1.0, Unit::Px)));
        assert!(decls.iter().any(
            |d| d.name == "border-top-color" && d.value == Value::Color(Color::rgb(0, 0, 255))
        ));
        // No border-right-width etc.
        assert!(!decls.iter().any(|d| d.name == "border-right-width"));
    }

    // ── flex / gap shorthand tests ────────────────────────────────────────

    /// Look up one declaration produced by a single-rule stylesheet.
    fn declared(css: &str, name: &str) -> Option<Value> {
        let ss = parse_css(css);
        ss.rules
            .first()?
            .declarations
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.value.clone())
    }

    #[test]
    fn flex_number_shorthand_sets_basis_zero() {
        // `flex: 1` is grow 1, shrink 1, basis 0 — the "share space evenly" form.
        assert_eq!(
            declared("div { flex: 1; }", "flex-grow"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            declared("div { flex: 1; }", "flex-shrink"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            declared("div { flex: 1; }", "flex-basis"),
            Some(Value::Length(0.0, Unit::Px))
        );
    }

    #[test]
    fn flex_three_value_shorthand_expands() {
        let css = "div { flex: 2 0 150px; }";
        assert_eq!(declared(css, "flex-grow"), Some(Value::Number(2.0)));
        assert_eq!(declared(css, "flex-shrink"), Some(Value::Number(0.0)));
        assert_eq!(
            declared(css, "flex-basis"),
            Some(Value::Length(150.0, Unit::Px))
        );
    }

    #[test]
    fn flex_keyword_shorthands_expand() {
        assert_eq!(
            declared("div { flex: none; }", "flex-grow"),
            Some(Value::Number(0.0))
        );
        assert_eq!(
            declared("div { flex: none; }", "flex-basis"),
            Some(Value::Keyword("auto".into()))
        );
        assert_eq!(
            declared("div { flex: auto; }", "flex-grow"),
            Some(Value::Number(1.0))
        );
        assert_eq!(
            declared("div { flex: auto; }", "flex-basis"),
            Some(Value::Keyword("auto".into()))
        );
    }

    #[test]
    fn gap_shorthand_expands_to_row_and_column() {
        assert_eq!(
            declared("div { gap: 12px; }", "row-gap"),
            Some(Value::Length(12.0, Unit::Px))
        );
        assert_eq!(
            declared("div { gap: 12px; }", "column-gap"),
            Some(Value::Length(12.0, Unit::Px))
        );

        let two = "div { gap: 4px 16px; }";
        assert_eq!(declared(two, "row-gap"), Some(Value::Length(4.0, Unit::Px)));
        assert_eq!(
            declared(two, "column-gap"),
            Some(Value::Length(16.0, Unit::Px))
        );
    }

    // ── calc() tests ──────────────────────────────────────────────────────

    #[test]
    fn calc_sub_parsed() {
        let ss = parse_css("div { width: calc(100% - 20px); }");
        let val = &ss.rules[0].declarations[0].value;
        assert_eq!(
            *val,
            Value::Calc(Box::new(CalcExpr::Sub(
                Box::new(CalcExpr::Percent(100.0)),
                Box::new(CalcExpr::Literal(20.0, Unit::Px)),
            )))
        );
    }

    #[test]
    fn calc_add_parsed() {
        let ss = parse_css("div { width: calc(50% + 10px); }");
        let val = &ss.rules[0].declarations[0].value;
        assert!(matches!(val, Value::Calc(expr) if matches!(expr.as_ref(), CalcExpr::Add(_, _))));
    }

    #[test]
    fn transform_translate_and_scale_parsed() {
        let ss = parse_css("div { transform: translate(15px, 25px) scale(1.5); }");
        let decl = &ss.rules[0].declarations[0];
        assert_eq!(decl.name, "transform");
        if let Value::Transform(t) = &decl.value {
            assert_eq!(t.translate_x, 15.0);
            assert_eq!(t.translate_y, 25.0);
            assert_eq!(t.scale, 1.5);
        } else {
            panic!("expected Transform value");
        }
    }
}
