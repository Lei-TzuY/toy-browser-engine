// ============================================================
//  script/mod.rs  —  Embedded JavaScript Engine & DOM API
// ============================================================
//
//  A lightweight JS AST tokenizer, parser, and tree-walk interpreter.
//  Enables parsing <script> tags, querying DOM elements by ID,
//  mutating DOM text/styles, and registering 'click' event handlers.

use std::collections::HashMap;
use crate::dom::{Node, NodeType};

// ── Tokens ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Let,
    Const,
    Function,
    Document,
    Console,
    Log,
    GetElementById,
    AddEventListener,
    InnerText,
    Style,
    SetAttribute,
    Ident(String),
    StringLit(String),
    NumberLit(f32),
    Dot,
    Comma,
    Semi,
    Colon,
    Equal,
    Plus,
    Minus,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Eof,
}

pub struct Tokenizer {
    chars: Vec<char>,
    pos: usize,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        Self { chars: input.chars().collect(), pos: 0 }
    }

    fn peek(&self) -> char { self.chars.get(self.pos).copied().unwrap_or('\0') }

    fn consume(&mut self) -> char {
        let c = self.peek();
        if self.pos < self.chars.len() { self.pos += 1; }
        c
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while self.peek() != '\0' && (self.peek().is_whitespace() || self.peek() == '\n' || self.peek() == '\r') {
                self.consume();
            }
            if self.peek() == '/' && self.chars.get(self.pos + 1) == Some(&'/') {
                while self.peek() != '\0' && self.peek() != '\n' { self.consume(); }
            } else {
                break;
            }
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_ws_and_comments();
        if self.pos >= self.chars.len() { return Token::Eof; }
        let c = self.peek();

        if c.is_alphabetic() || c == '_' || c == '$' {
            let mut s = String::new();
            while self.peek().is_alphanumeric() || self.peek() == '_' || self.peek() == '$' {
                s.push(self.consume());
            }
            return match s.as_str() {
                "let" => Token::Let,
                "const" => Token::Const,
                "function" => Token::Function,
                "document" => Token::Document,
                "console" => Token::Console,
                "log" => Token::Log,
                "getElementById" => Token::GetElementById,
                "addEventListener" => Token::AddEventListener,
                "innerText" => Token::InnerText,
                "style" => Token::Style,
                "setAttribute" => Token::SetAttribute,
                _ => Token::Ident(s),
            };
        }

        if c.is_ascii_digit() {
            let mut num_s = String::new();
            while self.peek().is_ascii_digit() || self.peek() == '.' {
                num_s.push(self.consume());
            }
            let n: f32 = num_s.parse().unwrap_or(0.0);
            return Token::NumberLit(n);
        }

        if c == '"' || c == '\'' {
            let quote = self.consume();
            let mut s = String::new();
            while self.peek() != '\0' && self.peek() != quote {
                s.push(self.consume());
            }
            if self.peek() == quote { self.consume(); }
            return Token::StringLit(s);
        }

        self.consume();
        match c {
            '.' => Token::Dot,
            ',' => Token::Comma,
            ';' => Token::Semi,
            ':' => Token::Colon,
            '=' => Token::Equal,
            '+' => Token::Plus,
            '-' => Token::Minus,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            _ => Token::Eof,
        }
    }
}

// ── AST Nodes ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum JsExpr {
    StringLit(String),
    NumberLit(f32),
    Ident(String),
    GetElementById(String),
    PropertyAccess { obj: Box<JsExpr>, prop: String },
    Call { callee: Box<JsExpr>, args: Vec<JsExpr> },
    Function { params: Vec<String>, body: Vec<JsStmt> },
    Binary { op: char, lhs: Box<JsExpr>, rhs: Box<JsExpr> },
}

#[derive(Debug, Clone)]
pub enum JsStmt {
    VarDecl { name: String, val: JsExpr },
    AssignProperty { obj: JsExpr, prop: String, val: JsExpr },
    Expr(JsExpr),
}

// ── Parser ────────────────────────────────────────────────────────────────────

pub struct JsParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl JsParser {
    pub fn new(input: &str) -> Self {
        let mut tokenizer = Tokenizer::new(input);
        let mut tokens = Vec::new();
        loop {
            let tok = tokenizer.next_token();
            if tok == Token::Eof {
                tokens.push(tok);
                break;
            }
            tokens.push(tok);
        }
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn consume(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() { self.pos += 1; }
        tok
    }

    pub fn parse_program(&mut self) -> Vec<JsStmt> {
        let mut stmts = Vec::new();
        while *self.peek() != Token::Eof {
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            } else {
                self.consume();
            }
        }
        stmts
    }

    fn parse_stmt(&mut self) -> Option<JsStmt> {
        match self.peek() {
            Token::Let | Token::Const => {
                self.consume();
                let name = if let Token::Ident(s) = self.consume() { s } else { return None; };
                if *self.peek() == Token::Equal { self.consume(); }
                let val = self.parse_expr()?;
                if *self.peek() == Token::Semi { self.consume(); }
                Some(JsStmt::VarDecl { name, val })
            }
            Token::Function => {
                self.consume();
                let name = if let Token::Ident(s) = self.consume() { s } else { return None; };
                let (params, body) = self.parse_function_tail()?;
                Some(JsStmt::VarDecl { name, val: JsExpr::Function { params, body } })
            }
            _ => {
                let expr = self.parse_expr()?;
                if *self.peek() == Token::Equal {
                    self.consume();
                    let val = self.parse_expr()?;
                    if *self.peek() == Token::Semi { self.consume(); }
                    if let JsExpr::PropertyAccess { obj, prop } = expr {
                        Some(JsStmt::AssignProperty { obj: *obj, prop, val })
                    } else if let JsExpr::Ident(name) = expr {
                        Some(JsStmt::VarDecl { name, val })
                    } else {
                        Some(JsStmt::Expr(val))
                    }
                } else {
                    if *self.peek() == Token::Semi { self.consume(); }
                    Some(JsStmt::Expr(expr))
                }
            }
        }
    }

    fn parse_function_tail(&mut self) -> Option<(Vec<String>, Vec<JsStmt>)> {
        if *self.peek() == Token::LParen { self.consume(); }
        let mut params = Vec::new();
        while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
            if let Token::Ident(p) = self.consume() {
                params.push(p);
            }
            if *self.peek() == Token::Comma { self.consume(); }
        }
        if *self.peek() == Token::RParen { self.consume(); }
        if *self.peek() == Token::LBrace { self.consume(); }
        let mut body = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            if let Some(s) = self.parse_stmt() {
                body.push(s);
            } else {
                self.consume();
            }
        }
        if *self.peek() == Token::RBrace { self.consume(); }
        Some((params, body))
    }

    fn parse_expr(&mut self) -> Option<JsExpr> {
        let mut lhs = self.parse_primary()?;

        loop {
            match self.peek() {
                Token::Dot => {
                    self.consume();
                    let prop = match self.consume() {
                        Token::Ident(s) => s,
                        Token::InnerText => "innerText".to_string(),
                        Token::Style => "style".to_string(),
                        Token::GetElementById => "getElementById".to_string(),
                        Token::AddEventListener => "addEventListener".to_string(),
                        Token::SetAttribute => "setAttribute".to_string(),
                        Token::Log => "log".to_string(),
                        _ => return Some(lhs),
                    };
                    if *self.peek() == Token::LParen {
                        self.consume();
                        let mut args = Vec::new();
                        while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
                            if let Some(arg) = self.parse_expr() {
                                args.push(arg);
                            }
                            if *self.peek() == Token::Comma { self.consume(); }
                        }
                        if *self.peek() == Token::RParen { self.consume(); }
                        lhs = JsExpr::Call {
                            callee: Box::new(JsExpr::PropertyAccess { obj: Box::new(lhs), prop }),
                            args,
                        };
                    } else {
                        lhs = JsExpr::PropertyAccess { obj: Box::new(lhs), prop };
                    }
                }
                Token::LParen => {
                    self.consume();
                    let mut args = Vec::new();
                    while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
                        if let Some(arg) = self.parse_expr() {
                            args.push(arg);
                        }
                        if *self.peek() == Token::Comma { self.consume(); }
                    }
                    if *self.peek() == Token::RParen { self.consume(); }
                    lhs = JsExpr::Call { callee: Box::new(lhs), args };
                }
                Token::Plus | Token::Minus => {
                    let op = if *self.peek() == Token::Plus { '+' } else { '-' };
                    self.consume();
                    let rhs = self.parse_expr()?;
                    lhs = JsExpr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
                }
                _ => break,
            }
        }

        Some(lhs)
    }

    fn parse_primary(&mut self) -> Option<JsExpr> {
        match self.peek().clone() {
            Token::StringLit(s) => { self.consume(); Some(JsExpr::StringLit(s)) }
            Token::NumberLit(n) => { self.consume(); Some(JsExpr::NumberLit(n)) }
            Token::Ident(s) => { self.consume(); Some(JsExpr::Ident(s)) }
            Token::Document => { self.consume(); Some(JsExpr::Ident("document".to_string())) }
            Token::Console => { self.consume(); Some(JsExpr::Ident("console".to_string())) }
            Token::Function => {
                self.consume();
                let (params, body) = self.parse_function_tail()?;
                Some(JsExpr::Function { params, body })
            }
            _ => None,
        }
    }
}

// ── Interpreter & Runtime ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum JsValue {
    Undefined,
    Number(f32),
    String(String),
    ElementId(String),
    Function { params: Vec<String>, body: Vec<JsStmt> },
}

pub struct JsInterpreter<'a> {
    pub env: HashMap<String, JsValue>,
    pub dom: &'a mut Node,
    pub event_listeners: Vec<(String, String, JsValue)>, // (element_id, event_type, handler)
}

impl<'a> JsInterpreter<'a> {
    pub fn new(dom: &'a mut Node) -> Self {
        Self {
            env: HashMap::new(),
            dom,
            event_listeners: Vec::new(),
        }
    }

    pub fn eval_program(&mut self, stmts: &[JsStmt]) {
        for stmt in stmts {
            self.eval_stmt(stmt);
        }
    }

    fn eval_stmt(&mut self, stmt: &JsStmt) {
        match stmt {
            JsStmt::VarDecl { name, val } => {
                let v = self.eval_expr(val);
                self.env.insert(name.clone(), v);
            }
            JsStmt::AssignProperty { obj, prop, val } => {
                let obj_v = self.eval_expr(obj);
                let val_v = self.eval_expr(val);
                if let JsValue::ElementId(id) = obj_v {
                    if prop == "innerText" {
                        let text = match &val_v {
                            JsValue::String(s) => s.clone(),
                            JsValue::Number(n) => n.to_string(),
                            _ => "".to_string(),
                        };
                        update_element_inner_text(self.dom, &id, &text);
                    } else if prop.starts_with("style") || prop == "style" {
                        if let JsValue::String(s) = val_v {
                            update_element_style(self.dom, &id, &s);
                        }
                    }
                }
            }
            JsStmt::Expr(expr) => {
                self.eval_expr(expr);
            }
        }
    }

    pub fn eval_expr(&mut self, expr: &JsExpr) -> JsValue {
        match expr {
            JsExpr::StringLit(s) => JsValue::String(s.clone()),
            JsExpr::NumberLit(n) => JsValue::Number(*n),
            JsExpr::Ident(name) => self.env.get(name).cloned().unwrap_or(JsValue::Undefined),
            JsExpr::PropertyAccess { obj, prop } => {
                let obj_val = self.eval_expr(obj);
                if let JsValue::ElementId(id) = obj_val {
                    if prop == "style" {
                        JsValue::ElementId(id)
                    } else {
                        JsValue::Undefined
                    }
                } else {
                    JsValue::Undefined
                }
            }
            JsExpr::Call { callee, args } => {
                if let JsExpr::PropertyAccess { obj, prop } = &**callee {
                    let obj_val = self.eval_expr(obj);
                    if let JsExpr::Ident(name) = &**obj {
                        if name == "document" && prop == "getElementById" {
                            if let Some(JsValue::String(id)) = args.first().map(|a| self.eval_expr(a)) {
                                return JsValue::ElementId(id);
                            }
                        }
                        if name == "console" && prop == "log" {
                            if let Some(val) = args.first().map(|a| self.eval_expr(a)) {
                                println!("[JS Console] {:?}", val);
                            }
                            return JsValue::Undefined;
                        }
                    }
                    if let JsValue::ElementId(id) = obj_val {
                        if prop == "addEventListener" {
                            let event_name = args.first().map(|a| self.eval_expr(a));
                            let handler = args.get(1).map(|a| self.eval_expr(a));
                            if let (Some(JsValue::String(evt)), Some(fn_val)) = (event_name, handler) {
                                self.event_listeners.push((id, evt, fn_val));
                            }
                        } else if prop == "setAttribute" {
                            let attr = args.first().map(|a| self.eval_expr(a));
                            let val = args.get(1).map(|a| self.eval_expr(a));
                            if let (Some(JsValue::String(attr_s)), Some(JsValue::String(val_s))) = (attr, val) {
                                update_element_attribute(self.dom, &id, &attr_s, &val_s);
                            }
                        }
                    }
                } else if let JsExpr::Ident(fn_name) = &**callee {
                    if let Some(JsValue::Function { params, body }) = self.env.get(fn_name).cloned() {
                        let arg_vals: Vec<JsValue> = args.iter().map(|a| self.eval_expr(a)).collect();
                        let old_env = self.env.clone();
                        for (p, v) in params.iter().zip(arg_vals) {
                            self.env.insert(p.clone(), v);
                        }
                        self.eval_program(&body);
                        self.env = old_env;
                    }
                }
                JsValue::Undefined
            }
            JsExpr::Function { params, body } => JsValue::Function { params: params.clone(), body: body.clone() },
            JsExpr::Binary { op, lhs, rhs } => {
                let l = self.eval_expr(lhs);
                let r = self.eval_expr(rhs);
                match (l, r) {
                    (JsValue::Number(a), JsValue::Number(b)) => match op {
                        '+' => JsValue::Number(a + b),
                        '-' => JsValue::Number(a - b),
                        _ => JsValue::Number(0.0),
                    },
                    (JsValue::String(a), JsValue::String(b)) => JsValue::String(format!("{}{}", a, b)),
                    (JsValue::String(a), JsValue::Number(b)) => JsValue::String(format!("{}{}", a, b)),
                    (JsValue::Number(a), JsValue::String(b)) => JsValue::String(format!("{}{}", a, b)),
                    _ => JsValue::Undefined,
                }
            }
            _ => JsValue::Undefined,
        }
    }
}

// ── DOM Mutation Helpers ──────────────────────────────────────────────────────

pub fn update_element_inner_text(node: &mut Node, target_id: &str, text: &str) -> bool {
    if let NodeType::Element(ref e) = node.node_type {
        if e.get_attr("id") == Some(target_id) {
            node.children.clear();
            node.children.push(Node::text(text.to_string()));
            return true;
        }
    }
    for child in &mut node.children {
        if update_element_inner_text(child, target_id, text) {
            return true;
        }
    }
    false
}

pub fn update_element_style(node: &mut Node, target_id: &str, style_str: &str) -> bool {
    if let NodeType::Element(ref mut e) = node.node_type {
        if e.get_attr("id") == Some(target_id) {
            e.set_attr("style", style_str);
            return true;
        }
    }
    for child in &mut node.children {
        if update_element_style(child, target_id, style_str) {
            return true;
        }
    }
    false
}

pub fn update_element_attribute(node: &mut Node, target_id: &str, attr: &str, val: &str) -> bool {
    if let NodeType::Element(ref mut e) = node.node_type {
        if e.get_attr("id") == Some(target_id) {
            e.set_attr(attr, val);
            return true;
        }
    }
    for child in &mut node.children {
        if update_element_attribute(child, target_id, attr, val) {
            return true;
        }
    }
    false
}

/// Execute all inline `<script>` blocks in the DOM tree.
pub fn execute_dom_scripts(dom: &mut Node) -> Vec<(String, String, JsValue)> {
    let scripts = collect_script_contents(dom);
    let mut interp = JsInterpreter::new(dom);
    for script_src in scripts {
        let mut parser = JsParser::new(&script_src);
        let stmts = parser.parse_program();
        interp.eval_program(&stmts);
    }
    interp.event_listeners
}

fn collect_script_contents(node: &Node) -> Vec<String> {
    let mut out = Vec::new();
    if let NodeType::Element(ref e) = node.node_type {
        if e.tag_name == "script" {
            for child in &node.children {
                if let NodeType::Text(ref t) = child.node_type {
                    out.push(t.clone());
                }
            }
        }
    }
    for child in &node.children {
        out.extend(collect_script_contents(child));
    }
    out
}

// ── Unit Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    #[test]
    fn test_js_tokenizer_and_parser() {
        let src = r#"let el = document.getElementById("title"); el.innerText = "Updated";"#;
        let mut parser = JsParser::new(src);
        let stmts = parser.parse_program();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_dom_mutation_via_js() {
        let mut dom = parse_html(r#"<div><h1 id="title">Old</h1></div>"#);
        let src = r#"let el = document.getElementById("title"); el.innerText = "New Title";"#;
        let mut parser = JsParser::new(src);
        let stmts = parser.parse_program();
        let mut interp = JsInterpreter::new(&mut dom);
        interp.eval_program(&stmts);

        fn find_text(n: &Node) -> String {
            if let NodeType::Text(t) = &n.node_type { return t.clone(); }
            n.children.iter().map(find_text).collect()
        }
        assert_eq!(find_text(&dom), "New Title");
    }
}
