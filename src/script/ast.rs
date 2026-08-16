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
    /// `&&` / `||` — short-circuiting, so kept separate from `Binary`.
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
