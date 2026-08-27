// ============================================================
//  script/parser.rs  —  JavaScript parser
// ============================================================
//
//  A recursive-descent parser with precedence climbing for
//  expressions.  It is deliberately forgiving: unparsable input is
//  skipped rather than reported, so a broken <script> never takes
//  down the page.
//
//  Supported grammar:
//    statements   let/const/var, function, if/else, while, for,
//                 for…of, for…in, return, break, continue, blocks,
//                 expressions, destructuring declarations
//    expressions  assignment (= += -= *= /=), ternary, ?? || &&,
//                 equality, relational, additive, multiplicative,
//                 unary (! - typeof), prefix/postfix ++/--,
//                 calls, member access, indexing, array & object
//                 literals, function expressions, arrow functions,
//                 template literals, spread, optional chaining

use super::ast::*;
use super::lexer::{Lexer, Tok};

pub struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
    /// Extra declarators from `let a = 1, b = 2;`, drained after each statement.
    pending: Vec<Stmt>,
}

impl Parser {
    pub fn new(input: &str) -> Self {
        Self {
            tokens: Lexer::new(input).tokenize(),
            pos: 0,
            pending: Vec::new(),
        }
    }

    // ── Token helpers ─────────────────────────────────────────────────────

    fn peek(&self) -> &Tok {
        self.tokens.get(self.pos).unwrap_or(&Tok::Eof)
    }

    fn peek_at(&self, offset: usize) -> &Tok {
        self.tokens.get(self.pos + offset).unwrap_or(&Tok::Eof)
    }

    fn bump(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, tok: Tok) -> bool {
        if *self.peek() == tok {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn at_end(&self) -> bool {
        *self.peek() == Tok::Eof
    }

    /// Any identifier-like token usable as a property or parameter name.
    /// The name a token contributes where an identifier is expected.
    ///
    /// Keywords are legal property names in JavaScript — `promise.catch(…)`,
    /// `promise.finally(…)`, `x.for` — so every keyword maps back to its text
    /// here. Without this, adding a keyword to the lexer would silently break
    /// any property with that name.
    fn ident_name(tok: &Tok) -> Option<String> {
        let keyword = match tok {
            Tok::Ident(s) => return Some(s.clone()),
            Tok::Of => "of",
            Tok::For => "for",
            Tok::If => "if",
            Tok::Else => "else",
            Tok::While => "while",
            Tok::Function => "function",
            Tok::Return => "return",
            Tok::Let => "let",
            Tok::Const => "const",
            Tok::Var => "var",
            Tok::Break => "break",
            Tok::Continue => "continue",
            Tok::Throw => "throw",
            Tok::Try => "try",
            Tok::Catch => "catch",
            Tok::Finally => "finally",
            Tok::New => "new",
            Tok::Typeof => "typeof",
            Tok::In => "in",
            Tok::True => "true",
            Tok::False => "false",
            Tok::Null => "null",
            Tok::Undefined => "undefined",
            _ => return None,
        };
        Some(keyword.to_string())
    }

    // ── Program & statements ──────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Vec<Stmt> {
        let mut out = Vec::new();
        while !self.at_end() {
            let before = self.pos;
            match self.parse_stmt() {
                Some(s) => {
                    out.push(s);
                    out.append(&mut self.pending);
                }
                None => {
                    // Guarantee forward progress on malformed input.
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        out
    }

    fn parse_block(&mut self) -> Vec<Stmt> {
        let mut out = Vec::new();
        while !self.at_end() && *self.peek() != Tok::RBrace {
            let before = self.pos;
            match self.parse_stmt() {
                Some(s) => {
                    out.push(s);
                    out.append(&mut self.pending);
                }
                None => {
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        self.eat(Tok::RBrace);
        out
    }

    /// A statement body: either a `{ … }` block or a single statement.
    fn parse_body(&mut self) -> Vec<Stmt> {
        if self.eat(Tok::LBrace) {
            self.parse_block()
        } else {
            let mut out = Vec::new();
            if let Some(s) = self.parse_stmt() {
                out.push(s);
                out.append(&mut self.pending);
            }
            out
        }
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        match self.peek().clone() {
            Tok::Semi => {
                self.bump();
                None
            }
            Tok::LBrace => {
                self.bump();
                Some(Stmt::Block(self.parse_block()))
            }
            Tok::Let | Tok::Const | Tok::Var => self.parse_var_decl(),
            Tok::Function => {
                self.bump();
                let name = Self::ident_name(&self.bump())?;
                let (params, body) = self.parse_function_tail()?;
                Some(Stmt::FnDecl { name, params, body })
            }
            Tok::If => self.parse_if(),
            Tok::While => {
                self.bump();
                self.eat(Tok::LParen);
                let test = self.parse_expr()?;
                self.eat(Tok::RParen);
                Some(Stmt::While {
                    test,
                    body: self.parse_body(),
                })
            }
            Tok::For => self.parse_for(),
            Tok::Return => {
                self.bump();
                let value = if matches!(self.peek(), Tok::Semi | Tok::RBrace | Tok::Eof) {
                    None
                } else {
                    self.parse_expr()
                };
                self.eat(Tok::Semi);
                Some(Stmt::Return(value))
            }
            Tok::Throw => {
                self.bump();
                let value = self.parse_expr()?;
                self.eat(Tok::Semi);
                Some(Stmt::Throw(value))
            }
            Tok::Try => self.parse_try(),
            Tok::Break => {
                self.bump();
                self.eat(Tok::Semi);
                Some(Stmt::Break)
            }
            Tok::Continue => {
                self.bump();
                self.eat(Tok::Semi);
                Some(Stmt::Continue)
            }
            _ => {
                let expr = self.parse_expr()?;
                self.eat(Tok::Semi);
                Some(Stmt::Expr(expr))
            }
        }
    }

    /// `try { … } catch (e) { … } finally { … }`, with either clause optional.
    fn parse_try(&mut self) -> Option<Stmt> {
        self.bump(); // try
        self.eat(Tok::LBrace);
        let block = self.parse_block();

        let catch = if self.eat(Tok::Catch) {
            // The binding is optional: `catch { … }` is legal.
            let binding = if self.eat(Tok::LParen) {
                let name = Self::ident_name(&self.bump());
                self.eat(Tok::RParen);
                name
            } else {
                None
            };
            self.eat(Tok::LBrace);
            Some((binding, self.parse_block()))
        } else {
            None
        };

        let finally = if self.eat(Tok::Finally) {
            self.eat(Tok::LBrace);
            Some(self.parse_block())
        } else {
            None
        };

        Some(Stmt::Try {
            block,
            catch,
            finally,
        })
    }

    fn parse_var_decl(&mut self) -> Option<Stmt> {
        self.bump(); // let / const / var
        // Destructuring: `let { a, b } = …` or `let [x, y] = …`
        if *self.peek() == Tok::LBrace {
            let pattern = self.parse_destruct_object_pattern()?;
            self.eat(Tok::Assign);
            let init = self.parse_expr()?;
            self.eat(Tok::Semi);
            return Some(Stmt::DestructDecl { pattern, init });
        }
        if *self.peek() == Tok::LBracket {
            let pattern = self.parse_destruct_array_pattern()?;
            self.eat(Tok::Assign);
            let init = self.parse_expr()?;
            self.eat(Tok::Semi);
            return Some(Stmt::DestructDecl { pattern, init });
        }
        let first = self.parse_one_declarator()?;
        while self.eat(Tok::Comma) {
            match self.parse_one_declarator() {
                Some(extra) => self.pending.push(extra),
                None => break,
            }
        }
        self.eat(Tok::Semi);
        Some(first)
    }

    fn parse_destruct_object_pattern(&mut self) -> Option<DestructPat> {
        self.bump(); // {
        let mut fields = Vec::new();
        while !self.at_end() && *self.peek() != Tok::RBrace {
            let key = Self::ident_name(&self.bump())?;
            let binding = if self.eat(Tok::Colon) {
                Self::ident_name(&self.bump())?
            } else {
                key.clone()
            };
            fields.push((key, binding));
            self.eat(Tok::Comma);
        }
        self.eat(Tok::RBrace);
        Some(DestructPat::Object(fields))
    }

    fn parse_destruct_array_pattern(&mut self) -> Option<DestructPat> {
        self.bump(); // [
        let mut items = Vec::new();
        let mut rest = None;
        while !self.at_end() && *self.peek() != Tok::RBracket {
            if *self.peek() == Tok::DotDotDot {
                self.bump();
                rest = Self::ident_name(&self.bump());
                self.eat(Tok::Comma);
                break;
            }
            if *self.peek() == Tok::Comma {
                items.push(None);
            } else {
                items.push(Self::ident_name(&self.bump()));
            }
            self.eat(Tok::Comma);
        }
        self.eat(Tok::RBracket);
        Some(DestructPat::Array { items, rest })
    }

    fn parse_one_declarator(&mut self) -> Option<Stmt> {
        let name = Self::ident_name(&self.bump())?;
        let init = if self.eat(Tok::Assign) {
            self.parse_expr()
        } else {
            None
        };
        Some(Stmt::VarDecl { name, init })
    }

    fn parse_if(&mut self) -> Option<Stmt> {
        self.bump(); // if
        self.eat(Tok::LParen);
        let test = self.parse_expr()?;
        self.eat(Tok::RParen);
        let cons = self.parse_body();
        let alt = if self.eat(Tok::Else) {
            if *self.peek() == Tok::If {
                self.parse_if().map(|s| vec![s])
            } else {
                Some(self.parse_body())
            }
        } else {
            None
        };
        Some(Stmt::If { test, cons, alt })
    }

    fn parse_for(&mut self) -> Option<Stmt> {
        self.bump(); // for
        self.eat(Tok::LParen);

        // `for (let x of items)` or `for (let x in obj)` — look ahead.
        let is_for_of = {
            let decl_offset = usize::from(matches!(self.peek(), Tok::Let | Tok::Const | Tok::Var));
            matches!(self.peek_at(decl_offset), Tok::Ident(_))
                && *self.peek_at(decl_offset + 1) == Tok::Of
        };
        let is_for_in = {
            let decl_offset = usize::from(matches!(self.peek(), Tok::Let | Tok::Const | Tok::Var));
            matches!(self.peek_at(decl_offset), Tok::Ident(_))
                && *self.peek_at(decl_offset + 1) == Tok::In
        };
        if is_for_of {
            if matches!(self.peek(), Tok::Let | Tok::Const | Tok::Var) {
                self.bump();
            }
            let name = Self::ident_name(&self.bump())?;
            self.bump(); // of
            let iterable = self.parse_expr()?;
            self.eat(Tok::RParen);
            return Some(Stmt::ForOf {
                name,
                iterable,
                body: self.parse_body(),
            });
        }
        if is_for_in {
            if matches!(self.peek(), Tok::Let | Tok::Const | Tok::Var) {
                self.bump();
            }
            let name = Self::ident_name(&self.bump())?;
            self.bump(); // in
            let target = self.parse_expr()?;
            self.eat(Tok::RParen);
            return Some(Stmt::ForIn {
                name,
                target,
                body: self.parse_body(),
            });
        }

        let init = if self.eat(Tok::Semi) {
            None
        } else if matches!(self.peek(), Tok::Let | Tok::Const | Tok::Var) {
            self.parse_var_decl().map(Box::new)
        } else {
            let e = self.parse_expr()?;
            self.eat(Tok::Semi);
            Some(Box::new(Stmt::Expr(e)))
        };

        let test = if *self.peek() == Tok::Semi {
            None
        } else {
            self.parse_expr()
        };
        self.eat(Tok::Semi);
        let update = if *self.peek() == Tok::RParen {
            None
        } else {
            self.parse_expr()
        };
        self.eat(Tok::RParen);

        Some(Stmt::For {
            init,
            test,
            update,
            body: self.parse_body(),
        })
    }

    fn parse_function_tail(&mut self) -> Option<(Vec<String>, Vec<Stmt>)> {
        self.eat(Tok::LParen);
        let mut params = Vec::new();
        while !self.at_end() && *self.peek() != Tok::RParen {
            // Rest parameter: `...args`
            if *self.peek() == Tok::DotDotDot {
                self.bump();
                if let Some(p) = Self::ident_name(&self.bump()) {
                    params.push(format!("...{p}"));
                }
                self.eat(Tok::Comma);
                continue;
            }
            if let Some(p) = Self::ident_name(&self.bump()) {
                params.push(p);
            }
            self.eat(Tok::Comma);
        }
        self.eat(Tok::RParen);
        self.eat(Tok::LBrace);
        Some((params, self.parse_block()))
    }

    // ── Expressions (precedence climbing) ─────────────────────────────────

    pub fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Option<Expr> {
        let lhs = self.parse_conditional()?;
        let op = match self.peek() {
            Tok::Assign => AssignOp::Assign,
            Tok::PlusAssign => AssignOp::Add,
            Tok::MinusAssign => AssignOp::Sub,
            Tok::StarAssign => AssignOp::Mul,
            Tok::SlashAssign => AssignOp::Div,
            _ => return Some(lhs),
        };
        self.bump();
        let value = self.parse_assignment()?; // right-associative
        Some(Expr::Assign {
            target: Box::new(lhs),
            op,
            value: Box::new(value),
        })
    }

    fn parse_conditional(&mut self) -> Option<Expr> {
        let test = self.parse_nullish_coalescing()?;
        if !self.eat(Tok::Question) {
            return Some(test);
        }
        let cons = self.parse_assignment()?;
        self.eat(Tok::Colon);
        let alt = self.parse_assignment()?;
        Some(Expr::Cond {
            test: Box::new(test),
            cons: Box::new(cons),
            alt: Box::new(alt),
        })
    }

    /// `??` sits between `||` and `?:` in precedence.
    fn parse_nullish_coalescing(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_logical_or()?;
        while self.eat(Tok::QuestionQuestion) {
            let rhs = self.parse_logical_or()?;
            lhs = Expr::Logical {
                op: LogicalOp::NullishCoalescing,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Some(lhs)
    }

    fn parse_logical_or(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_logical_and()?;
        while self.eat(Tok::OrOr) {
            let rhs = self.parse_logical_and()?;
            lhs = Expr::Logical {
                op: LogicalOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Some(lhs)
    }

    fn parse_logical_and(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_equality()?;
        while self.eat(Tok::AndAnd) {
            let rhs = self.parse_equality()?;
            lhs = Expr::Logical {
                op: LogicalOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Some(lhs)
    }

    fn parse_equality(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_relational()?;
        loop {
            let op = match self.peek() {
                Tok::Eq => BinOp::Eq,
                Tok::NotEq => BinOp::NotEq,
                Tok::StrictEq => BinOp::StrictEq,
                Tok::StrictNotEq => BinOp::StrictNotEq,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.parse_relational()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_relational(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Tok::Lt => BinOp::Lt,
                Tok::Gt => BinOp::Gt,
                Tok::Le => BinOp::Le,
                Tok::Ge => BinOp::Ge,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.parse_additive()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_additive(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.parse_multiplicative()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn parse_multiplicative(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Rem,
                _ => return Some(lhs),
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    /// `new Callee(args)` — the argument list is optional, as in JavaScript.
    fn parse_new(&mut self) -> Option<Expr> {
        self.bump(); // new
                     // The callee is a member expression without a call: `new a.b.C(…)`.
        let mut callee = self.parse_primary()?;
        while *self.peek() == Tok::Dot {
            self.bump();
            let prop = Self::ident_name(&self.bump())?;
            callee = Expr::Member {
                obj: Box::new(callee),
                prop,
            };
        }
        let args = if self.eat(Tok::LParen) {
            self.parse_args()
        } else {
            Vec::new()
        };
        Some(Expr::New {
            callee: Box::new(callee),
            args,
        })
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        let op = match self.peek() {
            Tok::Not => Some(UnaryOp::Not),
            Tok::Minus => Some(UnaryOp::Neg),
            Tok::Typeof => Some(UnaryOp::Typeof),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let expr = self.parse_unary()?;
            return Some(Expr::Unary {
                op,
                expr: Box::new(expr),
            });
        }
        if *self.peek() == Tok::New {
            let expression = self.parse_new()?;
            // `new X().y` and `new X().y()` keep working through the postfix
            // chain.
            return self.parse_member_chain(expression);
        }
        // Prefix ++ / --
        if matches!(self.peek(), Tok::Inc | Tok::Dec) {
            let op = if *self.peek() == Tok::Inc {
                UpdateOp::Inc
            } else {
                UpdateOp::Dec
            };
            self.bump();
            let target = self.parse_unary()?;
            return Some(Expr::Update {
                target: Box::new(target),
                op,
                prefix: true,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut expr = self.parse_call_member()?;
        if matches!(self.peek(), Tok::Inc | Tok::Dec) {
            let op = if *self.peek() == Tok::Inc {
                UpdateOp::Inc
            } else {
                UpdateOp::Dec
            };
            self.bump();
            expr = Expr::Update {
                target: Box::new(expr),
                op,
                prefix: false,
            };
        }
        Some(expr)
    }

    fn parse_call_member(&mut self) -> Option<Expr> {
        let expr = self.parse_primary()?;
        self.parse_member_chain(expr)
    }

    /// Continue a `.prop`, `[index]`, `(args)`, `?.prop`, `?.[index]` and
    /// `?.(args)` chain from `expr`.
    fn parse_member_chain(&mut self, expr: Expr) -> Option<Expr> {
        let mut expr = expr;
        loop {
            match self.peek() {
                Tok::Dot => {
                    self.bump();
                    let prop = Self::ident_name(&self.bump())?;
                    expr = Expr::Member {
                        obj: Box::new(expr),
                        prop,
                    };
                }
                Tok::QuestionDot => {
                    self.bump();
                    if *self.peek() == Tok::LBracket {
                        self.bump();
                        let index = self.parse_expr()?;
                        self.eat(Tok::RBracket);
                        expr = Expr::OptionalIndex {
                            obj: Box::new(expr),
                            index: Box::new(index),
                        };
                    } else if *self.peek() == Tok::LParen {
                        self.bump();
                        let args = self.parse_args();
                        expr = Expr::OptionalCall {
                            callee: Box::new(expr),
                            args,
                        };
                    } else {
                        let prop = Self::ident_name(&self.bump())?;
                        expr = Expr::OptionalMember {
                            obj: Box::new(expr),
                            prop,
                        };
                    }
                }
                Tok::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    self.eat(Tok::RBracket);
                    expr = Expr::Index {
                        obj: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Tok::LParen => {
                    self.bump();
                    let args = self.parse_args();
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                _ => return Some(expr),
            }
        }
    }

    fn parse_args(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        while !self.at_end() && *self.peek() != Tok::RParen {
            let before = self.pos;
            // Spread in calls: `fn(...arr)`
            if *self.peek() == Tok::DotDotDot {
                self.bump();
                match self.parse_assignment() {
                    Some(inner) => args.push(Expr::Spread(Box::new(inner))),
                    None => {
                        if self.pos == before + 1 {
                            self.bump();
                        }
                    }
                }
            } else {
                match self.parse_assignment() {
                    Some(a) => args.push(a),
                    None => {
                        if self.pos == before {
                            self.bump();
                        }
                    }
                }
            }
            self.eat(Tok::Comma);
        }
        self.eat(Tok::RParen);
        args
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        match self.peek().clone() {
            Tok::Num(n) => {
                self.bump();
                Some(Expr::Num(n))
            }
            Tok::Str(s) => {
                self.bump();
                Some(Expr::Str(s))
            }
            Tok::True => {
                self.bump();
                Some(Expr::Bool(true))
            }
            Tok::False => {
                self.bump();
                Some(Expr::Bool(false))
            }
            Tok::Null => {
                self.bump();
                Some(Expr::Null)
            }
            Tok::Undefined => {
                self.bump();
                Some(Expr::Undefined)
            }
            Tok::Ident(name) => {
                // Single-parameter arrow function: `x => …`
                if *self.peek_at(1) == Tok::Arrow {
                    self.bump();
                    self.bump();
                    let body = self.parse_arrow_body();
                    return Some(Expr::Function {
                        params: vec![name],
                        body,
                    });
                }
                self.bump();
                Some(Expr::Ident(name))
            }
            Tok::Function => {
                self.bump();
                // Named function expressions: the name is not bound, just skipped.
                if matches!(self.peek(), Tok::Ident(_)) {
                    self.bump();
                }
                let (params, body) = self.parse_function_tail()?;
                Some(Expr::Function { params, body })
            }
            Tok::LParen => {
                if self.is_arrow_ahead() {
                    self.bump(); // (
                    let mut params = Vec::new();
                    while !self.at_end() && *self.peek() != Tok::RParen {
                        if *self.peek() == Tok::DotDotDot {
                            self.bump();
                            if let Some(p) = Self::ident_name(&self.bump()) {
                                params.push(format!("...{p}"));
                            }
                        } else if let Some(p) = Self::ident_name(&self.bump()) {
                            params.push(p);
                        }
                        self.eat(Tok::Comma);
                    }
                    self.eat(Tok::RParen);
                    self.eat(Tok::Arrow);
                    let body = self.parse_arrow_body();
                    return Some(Expr::Function { params, body });
                }
                self.bump();
                let inner = self.parse_expr()?;
                self.eat(Tok::RParen);
                Some(inner)
            }
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                while !self.at_end() && *self.peek() != Tok::RBracket {
                    let before = self.pos;
                    if *self.peek() == Tok::DotDotDot {
                        self.bump();
                        match self.parse_assignment() {
                            Some(inner) => items.push(Expr::Spread(Box::new(inner))),
                            None => {
                                if self.pos == before + 1 {
                                    self.bump();
                                }
                            }
                        }
                    } else {
                        match self.parse_assignment() {
                            Some(e) => items.push(e),
                            None => {
                                if self.pos == before {
                                    self.bump();
                                }
                            }
                        }
                    }
                    self.eat(Tok::Comma);
                }
                self.eat(Tok::RBracket);
                Some(Expr::Array(items))
            }
            Tok::LBrace => {
                self.bump();
                let mut props = Vec::new();
                while !self.at_end() && *self.peek() != Tok::RBrace {
                    // Spread in objects: `{ ...other }`
                    if *self.peek() == Tok::DotDotDot {
                        self.bump();
                        if let Some(inner) = self.parse_assignment() {
                            props.push(("__spread__".to_string(), Expr::Spread(Box::new(inner))));
                        }
                        self.eat(Tok::Comma);
                        continue;
                    }
                    let key = match self.bump() {
                        Tok::Str(s) => s,
                        other => Self::ident_name(&other)?,
                    };
                    // Shorthand properties: `{ a, b }` === `{ a: a, b: b }`
                    if matches!(self.peek(), Tok::Comma | Tok::RBrace) {
                        props.push((key.clone(), Expr::Ident(key)));
                    } else {
                        self.eat(Tok::Colon);
                        if let Some(v) = self.parse_assignment() {
                            props.push((key, v));
                        }
                    }
                    self.eat(Tok::Comma);
                }
                self.eat(Tok::RBrace);
                Some(Expr::Object(props))
            }
            // Template literals
            Tok::TemplatePart(first_part) => {
                self.bump();
                self.parse_template_literal(first_part)
            }
            Tok::TemplateEnd(s) => {
                self.bump();
                // No interpolation — just a plain string.
                Some(Expr::Str(s))
            }
            _ => None,
        }
    }

    /// The body of an arrow function: a block, or a single expression that is
    /// desugared into `return <expr>;`.
    fn parse_arrow_body(&mut self) -> Vec<Stmt> {
        if self.eat(Tok::LBrace) {
            self.parse_block()
        } else {
            match self.parse_assignment() {
                Some(e) => vec![Stmt::Return(Some(e))],
                None => Vec::new(),
            }
        }
    }

    /// At a `(`, decide whether this opens an arrow-function parameter list by
    /// scanning ahead to the matching `)` and checking for `=>`.
    fn is_arrow_ahead(&self) -> bool {
        let mut depth = 0usize;
        let mut i = self.pos;
        loop {
            match self.tokens.get(i) {
                Some(Tok::LParen) => depth += 1,
                Some(Tok::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(i + 1), Some(Tok::Arrow));
                    }
                }
                Some(Tok::Eof) | None => return false,
                _ => {}
            }
            i += 1;
        }
    }

    /// Parse a template literal after the first `TemplatePart` has been consumed.
    fn parse_template_literal(&mut self, first_part: String) -> Option<Expr> {
        let mut parts = vec![first_part];
        let mut exprs = Vec::new();
        loop {
            // Parse the interpolated expression.
            if let Some(e) = self.parse_expr() {
                exprs.push(e);
            } else {
                exprs.push(Expr::Undefined);
            }
            // The lexer has already emitted either TemplatePart or TemplateEnd next.
            match self.peek().clone() {
                Tok::TemplatePart(s) => {
                    self.bump();
                    parts.push(s);
                }
                Tok::TemplateEnd(s) => {
                    self.bump();
                    parts.push(s);
                    break;
                }
                _ => {
                    // Malformed: bail.
                    parts.push(String::new());
                    break;
                }
            }
        }
        Some(Expr::TemplateLiteral { parts, exprs })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<Stmt> {
        Parser::new(src).parse_program()
    }

    #[test]
    fn parses_var_decl_and_expression_statements() {
        let stmts = parse("let a = 1; a + 2;");
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::VarDecl { .. }));
        assert!(matches!(stmts[1], Stmt::Expr(Expr::Binary { .. })));
    }

    #[test]
    fn parses_multiple_declarators() {
        let stmts = parse("let a = 1, b = 2;");
        assert_eq!(stmts.len(), 2);
        assert!(matches!(&stmts[1], Stmt::VarDecl { name, .. } if name == "b"));
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        let stmts = parse("1 + 2 * 3;");
        match &stmts[0] {
            Stmt::Expr(Expr::Binary {
                op: BinOp::Add,
                rhs,
                ..
            }) => {
                assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("unexpected AST: {:?}", other),
        }
    }

    #[test]
    fn parses_if_else_chain() {
        let stmts = parse("if (a) { b(); } else if (c) { d(); } else { e(); }");
        assert!(matches!(stmts[0], Stmt::If { alt: Some(_), .. }));
    }

    #[test]
    fn parses_for_loop_parts() {
        let stmts = parse("for (let i = 0; i < 3; i++) { x(); }");
        match &stmts[0] {
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                assert!(init.is_some() && test.is_some() && update.is_some());
                assert_eq!(body.len(), 1);
            }
            other => panic!("unexpected AST: {:?}", other),
        }
    }

    #[test]
    fn parses_for_of_loop() {
        let stmts = parse("for (const item of items) { log(item); }");
        assert!(matches!(&stmts[0], Stmt::ForOf { name, .. } if name == "item"));
    }

    #[test]
    fn parses_arrow_functions() {
        let stmts = parse("let f = (a, b) => a + b; let g = x => x;");
        assert!(matches!(
            &stmts[0],
            Stmt::VarDecl {
                init: Some(Expr::Function { .. }),
                ..
            }
        ));
        assert!(matches!(
            &stmts[1],
            Stmt::VarDecl {
                init: Some(Expr::Function { .. }),
                ..
            }
        ));
    }

    #[test]
    fn parses_member_chain_with_call_and_index() {
        let stmts = parse("a.b(1)[2].c;");
        assert!(matches!(stmts[0], Stmt::Expr(Expr::Member { .. })));
    }

    #[test]
    fn parses_array_and_object_literals() {
        let stmts = parse(r#"let a = [1, 2]; let o = { x: 1, "y": 2 };"#);
        assert!(
            matches!(&stmts[0], Stmt::VarDecl { init: Some(Expr::Array(items)), .. } if items.len() == 2)
        );
        assert!(
            matches!(&stmts[1], Stmt::VarDecl { init: Some(Expr::Object(props)), .. } if props.len() == 2)
        );
    }

    #[test]
    fn parenthesised_expression_is_not_mistaken_for_arrow() {
        let stmts = parse("let a = (1 + 2) * 3;");
        assert!(matches!(
            &stmts[0],
            Stmt::VarDecl {
                init: Some(Expr::Binary { op: BinOp::Mul, .. }),
                ..
            }
        ));
    }

    #[test]
    fn malformed_input_does_not_hang() {
        let stmts = parse("let ; ) } === ;");
        assert!(stmts.len() < 5);
    }

    // ── `new`, `throw` and `try` ──────────────────────────────────────────

    #[test]
    fn parses_a_new_expression_with_arguments() {
        // `new` is general: nothing here is specific to Promise.
        let stmts = parse("new Promise(function (resolve) { resolve(1); });");
        assert_eq!(stmts.len(), 1, "{stmts:?}");
        match &stmts[0] {
            Stmt::Expr(Expr::New { callee, args }) => {
                assert!(matches!(&**callee, Expr::Ident(name) if name == "Promise"));
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected a New expression, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_new_expression_without_arguments() {
        let stmts = parse("new Thing;");
        match &stmts[0] {
            Stmt::Expr(Expr::New { callee, args }) => {
                assert!(matches!(&**callee, Expr::Ident(name) if name == "Thing"));
                assert!(args.is_empty());
            }
            other => panic!("expected a New expression, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_method_call_on_a_new_expression() {
        let stmts = parse("new Promise(f).then(g);");
        match &stmts[0] {
            Stmt::Expr(Expr::Call { callee, .. }) => {
                assert!(matches!(&**callee, Expr::Member { prop, .. } if prop == "then"));
            }
            other => panic!("expected a call, got {other:?}"),
        }
    }

    #[test]
    fn keywords_are_usable_as_property_names() {
        // `catch`, `finally`, `new` and friends are tokens, but after a dot
        // they are ordinary property names.
        for source in [
            "p.catch(f);",
            "p.finally(f);",
            "p.then(f);",
            "o.new;",
            "o.for;",
            "o.class;",
        ] {
            let stmts = parse(source);
            assert_eq!(stmts.len(), 1, "{source} produced {stmts:?}");
            let is_member = match &stmts[0] {
                Stmt::Expr(Expr::Member { .. }) => true,
                Stmt::Expr(Expr::Call { callee, .. }) => {
                    matches!(&**callee, Expr::Member { .. })
                }
                _ => false,
            };
            assert!(is_member, "{source} did not parse as a property access");
        }
    }

    #[test]
    fn parses_throw_try_catch_finally() {
        let stmts = parse("try { throw 1; } catch (e) { f(e); } finally { g(); }");
        match &stmts[0] {
            Stmt::Try {
                block,
                catch,
                finally,
            } => {
                assert!(matches!(block.as_slice(), [Stmt::Throw(_)]));
                let (binding, body) = catch.as_ref().expect("catch clause");
                assert_eq!(binding.as_deref(), Some("e"));
                assert_eq!(body.len(), 1);
                assert_eq!(finally.as_ref().map(Vec::len), Some(1));
            }
            other => panic!("expected a Try statement, got {other:?}"),
        }
    }

    #[test]
    fn catch_binding_and_finally_are_both_optional() {
        match &parse("try { f(); } catch { g(); }")[0] {
            Stmt::Try { catch, finally, .. } => {
                let (binding, _) = catch.as_ref().expect("catch clause");
                assert!(binding.is_none(), "`catch {{ … }}` has no binding");
                assert!(finally.is_none());
            }
            other => panic!("expected a Try statement, got {other:?}"),
        }
        match &parse("try { f(); } finally { g(); }")[0] {
            Stmt::Try { catch, finally, .. } => {
                assert!(catch.is_none());
                assert!(finally.is_some());
            }
            other => panic!("expected a Try statement, got {other:?}"),
        }
    }
}
