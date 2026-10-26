//! Syntax tree of a `.si` source file.

use crate::diag::Span;

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct File {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Import { path: String, span: Span },
    Const(ConstDecl),
    Enum(EnumDecl),
    Module(ModuleDecl),
    Alias(AliasDecl),
}

/// `const NAME (: type)? = expr;`
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
}

/// `enum Name (: type)? { A, B = 4, ... }`
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: Ident,
    pub repr: Option<TypeExpr>,
    pub variants: Vec<(Ident, Option<Expr>)>,
}

/// `module Name<P = default, ...>(args) -> (outputs) { body }`, or without a body
/// (`;`), a bundle type.
#[derive(Debug, Clone)]
pub struct ModuleDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub args: Vec<PortDecl>,
    pub rets: Vec<PortDecl>,
    pub body: Option<Block>,
    /// `extern module ... = "path";`: implemented by the Rust type at `path`.
    pub rust_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub default: Option<Expr>,
}

/// `name: type (reset expr)?`
#[derive(Debug, Clone)]
pub struct PortDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    /// Reset value for the parts of the port driven by registers.
    pub reset: Option<Expr>,
    /// `@label` of an extern module's port: ports with different labels may be in
    /// different clock domains. Unlabelled ports share one.
    pub domain: Option<Ident>,
    pub span: Span,
}

/// `module Name = Target::<params>;`
#[derive(Debug, Clone)]
pub struct AliasDecl {
    pub name: Ident,
    pub target: Ident,
    pub params: Vec<GenericArg>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct GenericArg {
    pub name: Option<Ident>,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `u8`, or `u<expr>`.
    UInt { width: Box<Expr>, span: Span },
    /// An enum or bundle name, with bundle parameters.
    Named(Ident, Option<Vec<GenericArg>>),
    /// `[T; N]`
    Array {
        elem: Box<TypeExpr>,
        len: Box<Expr>,
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::UInt { span, .. } | TypeExpr::Array { span, .. } => *span,
            TypeExpr::Named(id, _) => id.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`: combinational connection.
    Comb,
    /// `<=`: register next-state.
    Reg,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let x = e;` (an immutable value), or `let x: T (reset e)?;` (a net driven later
    /// with `=` or `<=`).
    Let {
        name: Ident,
        ty: Option<TypeExpr>,
        value: Option<Expr>,
        reset: Option<Expr>,
        span: Span,
    },
    /// `target = value;` or `target <= value;`, where `target` is a place expression.
    Assign {
        target: Expr,
        op: AssignOp,
        op_span: Span,
        value: Expr,
    },
    If {
        cond: Expr,
        then: Block,
        els: Option<Else>,
        span: Span,
    },
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm<Block>>,
        span: Span,
    },
    For {
        var: Ident,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Block,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum Else {
    Block(Block),
    If(Box<Stmt>),
}

#[derive(Debug, Clone)]
pub struct MatchArm<B> {
    pub pats: Vec<Pattern>,
    pub body: B,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard(Span),
    Value(Expr),
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(span) => *span,
            Pattern::Value(e) => e.span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LogicAnd,
    LogicOr,
}

impl BinOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::And => "&",
            BinOp::Or => "|",
            BinOp::Xor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::LogicAnd => "&&",
            BinOp::LogicOr => "||",
        }
    }

    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int {
        value: u128,
        width: Option<u32>,
    },
    Bool(bool),
    Name(Ident),
    /// `Enum::Variant`
    Path(Ident, Ident),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Cast(Box<Expr>, TypeExpr),
    Index(Box<Expr>, Box<Expr>),
    Slice {
        base: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
    },
    Field(Box<Expr>, Ident),
    Method {
        recv: Box<Expr>,
        name: Ident,
        args: Vec<Expr>,
    },
    /// Builtin function call or module instantiation.
    Call {
        callee: Ident,
        generics: Option<Vec<GenericArg>>,
        args: Vec<CallArg>,
    },
    If {
        cond: Box<Expr>,
        then: ExprBlock,
        els: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm<Expr>>,
    },
    Block(ExprBlock),
}

#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<Ident>,
    pub value: Expr,
}

/// `{ let a = ..; let b = ..; value }`
#[derive(Debug, Clone)]
pub struct ExprBlock {
    pub lets: Vec<(Ident, Option<TypeExpr>, Expr)>,
    pub value: Box<Expr>,
    pub span: Span,
}
