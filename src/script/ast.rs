// ============================================================
//  script/ast.rs  —  JavaScript syntax tree
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
    /// `??` — returns the RHS only if the LHS is `null` or `undefined`.
    NullishCoalescing,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    Typeof,
}

/// `=`, `+=`, `-=`, `*=`, `/=`
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UpdateOp {
    Inc,
    Dec,
}

/// A destructuring pattern for `let { a, b } = …` and `let [x, y] = …`.
#[derive(Debug, Clone)]
pub enum DestructPat {
    /// `{ a, b: alias, c }` — each entry is `(key, binding_name)`.
    Object(Vec<(String, String)>),
    /// `[a, b, ...rest]` — names in order, with an optional rest element.
    Array {
        items: Vec<Option<String>>,
        rest: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f32),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Ident(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    /// `` `hello ${name}, you are ${age}` ``
    TemplateLiteral {
        /// Static string parts (one more than `exprs`).
        parts: Vec<String>,
        /// Interpolated expressions.
        exprs: Vec<Expr>,
    },
    /// `...expr` — spread in an array literal, object literal, or call.
    Spread(Box<Expr>),
    /// `obj.prop`
    Member {
        obj: Box<Expr>,
        prop: String,
    },
    /// `obj[index]`
    Index {
        obj: Box<Expr>,
        index: Box<Expr>,
    },
    /// `obj?.prop`
    OptionalMember {
        obj: Box<Expr>,
        prop: String,
    },
    /// `obj?.[index]`
    OptionalIndex {
        obj: Box<Expr>,
        index: Box<Expr>,
    },
    /// `obj?.(args)`
    OptionalCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `&&` / `||` / `??` — short-circuiting, so kept separate from `Binary`.
    Logical {
        op: LogicalOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        op: AssignOp,
        value: Box<Expr>,
    },
    Update {
        target: Box<Expr>,
        op: UpdateOp,
        prefix: bool,
    },
    /// `test ? cons : alt`
    Cond {
        test: Box<Expr>,
        cons: Box<Expr>,
        alt: Box<Expr>,
    },
    Function {
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    /// `new Callee(args)`
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let` / `const` / `var`
    VarDecl {
        name: String,
        init: Option<Expr>,
    },
    /// `let { a, b } = expr;` or `let [x, y] = expr;`
    DestructDecl {
        pattern: DestructPat,
        init: Expr,
    },
    FnDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Expr(Expr),
    If {
        test: Expr,
        cons: Vec<Stmt>,
        alt: Option<Vec<Stmt>>,
    },
    While {
        test: Expr,
        body: Vec<Stmt>,
    },
    For {
        init: Option<Box<Stmt>>,
        test: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
    },
    /// `for (const x of items) { … }`
    ForOf {
        name: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },
    /// `for (const key in obj) { … }`
    ForIn {
        name: String,
        target: Expr,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Block(Vec<Stmt>),
    /// `throw expression;`
    Throw(Expr),
    /// `try { … } catch (binding) { … } finally { … }`
    Try {
        block: Vec<Stmt>,
        /// The catch clause; the binding is absent in `catch { … }`.
        catch: Option<(Option<String>, Vec<Stmt>)>,
        finally: Option<Vec<Stmt>>,
    },
}
