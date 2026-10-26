//! Recursive-descent parser producing [`ast::File`](crate::ast::File).
//!
//! Operator precedence follows Rust, from loosest to tightest:
//! `||`, `&&`, comparisons (non-associative), `|`, `^`, `&`, `<<` `>>`, `+` `-`,
//! `*` `/` `%`, `as`, unary `!` `-`, postfix (`[]`, `.`, calls).

use crate::ast::*;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::lexer::{Kw, Tok, Token};

/// Marker for a reported parse error; the diagnostic is already in `diags`.
struct Fail;

type PResult<T> = Result<T, Fail>;

pub fn parse(tokens: Vec<Token>, diags: &mut Diagnostics) -> File {
    let mut p = Parser {
        tokens,
        pos: 0,
        diags,
    };
    p.file()
}

struct Parser<'d> {
    tokens: Vec<Token>,
    pos: usize,
    diags: &'d mut Diagnostics,
}

impl Parser<'_> {
    // ── token helpers ────────────────────────────────────────────────────

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let idx = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[idx].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn at(&self, tok: &Tok) -> bool {
        self.peek() == tok
    }

    fn at_kw(&self, kw: Kw) -> bool {
        matches!(self.peek(), Tok::Kw(k) if *k == kw)
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.at(tok) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: Kw) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn error_here(&mut self, expected: &str) -> Fail {
        let found = self.peek().clone();
        let span = self.span();
        self.diags.push(
            Diagnostic::error(format!("expected {expected}, found {found}"))
                .at(span, format!("expected {expected}")),
        );
        Fail
    }

    fn expect(&mut self, tok: &Tok) -> PResult<Span> {
        if self.at(tok) {
            Ok(self.bump().span)
        } else {
            Err(self.error_here(&tok.to_string()))
        }
    }

    fn ident(&mut self) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            _ => Err(self.error_here("an identifier")),
        }
    }

    /// Skips tokens until just after a `;` or just before a `}` closing the current block.
    fn recover_stmt(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::Semi if depth == 0 => {
                    self.bump();
                    return;
                }
                Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RBrace if depth == 0 => return,
                Tok::RBrace | Tok::RParen | Tok::RBracket => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && matches!(self.peek(), Tok::RBrace) {
                        self.bump();
                        return;
                    }
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// Skips tokens until the start of the next top-level item.
    fn recover_item(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::LBrace => depth += 1,
                Tok::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                Tok::Kw(Kw::Module | Kw::Const | Kw::Enum | Kw::Import | Kw::Extern) if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    // ── items ────────────────────────────────────────────────────────────

    fn file(&mut self) -> File {
        let mut items = Vec::new();
        while !self.at(&Tok::Eof) {
            let start = self.pos;
            match self.item() {
                Ok(item) => items.push(item),
                Err(Fail) => {
                    self.recover_item();
                    if self.pos == start {
                        self.bump();
                    }
                }
            }
        }
        File { items }
    }

    fn item(&mut self) -> PResult<Item> {
        let start = self.span();
        // `extern` before `module`, if this item is the module part of an extern.
        let start_kw = match self.tokens.get(self.pos.wrapping_sub(1)) {
            Some(Token { tok: Tok::Kw(Kw::Extern), .. }) if self.pos > 0 => Some(Kw::Extern),
            _ => None,
        };
        match self.peek() {
            Tok::Kw(Kw::Import) => {
                self.bump();
                let path = match self.peek().clone() {
                    Tok::Str(s) => {
                        self.bump();
                        s
                    }
                    _ => return Err(self.error_here("a file path string")),
                };
                self.expect(&Tok::Semi)?;
                Ok(Item::Import {
                    path,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Kw(Kw::Const) => {
                self.bump();
                let name = self.ident()?;
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect(&Tok::Eq)?;
                let value = self.expr()?;
                self.expect(&Tok::Semi)?;
                Ok(Item::Const(ConstDecl { name, ty, value }))
            }
            Tok::Kw(Kw::Enum) => {
                self.bump();
                let name = self.ident()?;
                let repr = if self.eat(&Tok::Colon) {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect(&Tok::LBrace)?;
                let mut variants = Vec::new();
                while !self.at(&Tok::RBrace) {
                    let vname = self.ident()?;
                    let value = if self.eat(&Tok::Eq) {
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    variants.push((vname, value));
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBrace)?;
                Ok(Item::Enum(EnumDecl {
                    name,
                    repr,
                    variants,
                }))
            }
            Tok::Kw(Kw::Extern) => {
                self.bump();
                if !self.at_kw(Kw::Module) {
                    return Err(self.error_here("`module`"));
                }
                let Item::Module(mut decl) = self.item()? else {
                    let span = self.prev_span();
                    self.diags.push(Diagnostic::error("expected a module signature").at(span, ""));
                    return Err(Fail);
                };
                if decl.rust_path.is_none() {
                    let span = decl.name.span;
                    self.diags.push(
                        Diagnostic::error("an extern module needs the path of its Rust type")
                            .at(span, "")
                            .note("write `extern module Name(...) -> (...) = \"path::to::Type\";`"),
                    );
                    return Err(Fail);
                }
                decl.body = None;
                Ok(Item::Module(decl))
            }
            Tok::Kw(Kw::Module) => {
                self.bump();
                let name = self.ident()?;
                if self.eat(&Tok::Eq) {
                    let target = self.ident()?;
                    let params = if self.eat(&Tok::ColonColon) {
                        self.generic_args()?
                    } else {
                        Vec::new()
                    };
                    self.expect(&Tok::Semi)?;
                    return Ok(Item::Alias(AliasDecl {
                        name,
                        target,
                        params,
                        span: start.to(self.prev_span()),
                    }));
                }
                let mut params = Vec::new();
                if self.eat(&Tok::Lt) {
                    while !self.at(&Tok::Gt) {
                        let pname = self.ident()?;
                        let default = if self.eat(&Tok::Eq) {
                            // Parse above comparison level so `>` closes the list.
                            Some(self.binary(Prec::BitOr)?)
                        } else {
                            None
                        };
                        params.push(Param {
                            name: pname,
                            default,
                        });
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::Gt)?;
                }
                // `()` and `-> ()` may be left out when empty.
                let args = if self.at(&Tok::LParen) {
                    self.port_list()?
                } else {
                    Vec::new()
                };
                let rets = if self.eat(&Tok::Arrow) {
                    self.port_list()?
                } else {
                    Vec::new()
                };
                let mut rust_path = None;
                let body = if self.eat(&Tok::Semi) {
                    None
                } else if self.eat(&Tok::Eq) {
                    match self.peek().clone() {
                        Tok::Str(path) => {
                            self.bump();
                            rust_path = Some(path);
                        }
                        _ => return Err(self.error_here("the path of a Rust type, as a string")),
                    }
                    self.expect(&Tok::Semi)?;
                    None
                } else {
                    Some(self.block()?)
                };
                if rust_path.is_some() && !matches!(start_kw, Some(Kw::Extern)) {
                    let span = name.span;
                    self.diags.push(
                        Diagnostic::error("a module implemented in Rust must be declared `extern module`")
                            .at(span, ""),
                    );
                    return Err(Fail);
                }
                Ok(Item::Module(ModuleDecl {
                    name,
                    params,
                    args,
                    rets,
                    body,
                    rust_path,
                }))
            }
            _ => Err(self.error_here("an item (`module`, `enum`, `const` or `import`)")),
        }
    }

    /// `( name: type (@domain)? (reset expr)?, ... )`
    fn port_list(&mut self) -> PResult<Vec<PortDecl>> {
        self.expect(&Tok::LParen)?;
        let mut ports = Vec::new();
        while !self.at(&Tok::RParen) {
            if let Tok::Ident(word) = self.peek()
                && matches!(word.as_str(), "out" | "reg" | "wire")
                && matches!(self.peek_at(1), Tok::Ident(_))
            {
                let span = self.span();
                self.diags.push(
                    Diagnostic::error(format!("unexpected `{word}` in a port list"))
                        .at(span, "")
                        .note("inputs go in `(...)` and outputs in `-> (...)`; registers are inferred from `<=`"),
                );
                return Err(Fail);
            }
            if self.at_kw(Kw::In) {
                let span = self.span();
                self.diags.push(
                    Diagnostic::error("unexpected `in` in a port list")
                        .at(span, "")
                        .note("inputs go in `(...)` and outputs in `-> (...)`"),
                );
                return Err(Fail);
            }
            let start = self.span();
            let name = self.ident()?;
            self.expect(&Tok::Colon)?;
            let ty = self.ty()?;
            let domain = if self.eat(&Tok::At) { Some(self.ident()?) } else { None };
            let reset = if self.eat_kw(Kw::Reset) {
                Some(self.expr()?)
            } else {
                None
            };
            ports.push(PortDecl {
                name,
                ty,
                reset,
                domain,
                span: start.to(self.prev_span()),
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(ports)
    }

    /// `::<` already consumed up to `::`; parses `<args>`.
    fn generic_args(&mut self) -> PResult<Vec<GenericArg>> {
        self.expect(&Tok::Lt)?;
        let mut args = Vec::new();
        while !self.at(&Tok::Gt) {
            let name = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Eq {
                let id = self.ident()?;
                self.bump();
                Some(id)
            } else {
                None
            };
            let value = self.binary(Prec::BitOr)?;
            args.push(GenericArg { name, value });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Gt)?;
        Ok(args)
    }

    // ── types ────────────────────────────────────────────────────────────

    fn ty(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        if self.eat(&Tok::LBracket) {
            let elem = self.ty()?;
            self.expect(&Tok::Semi)?;
            let len = self.expr()?;
            self.expect(&Tok::RBracket)?;
            return Ok(TypeExpr::Array {
                elem: Box::new(elem),
                len: Box::new(len),
                span: start.to(self.prev_span()),
            });
        }
        let id = self.ident()?;
        if id.name == "u" && self.at(&Tok::Lt) {
            self.bump();
            let width = self.binary(Prec::BitOr)?;
            self.expect(&Tok::Gt)?;
            return Ok(TypeExpr::UInt {
                width: Box::new(width),
                span: start.to(self.prev_span()),
            });
        }
        if let Some(digits) = id.name.strip_prefix('u')
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            let value = digits.parse::<u128>().unwrap_or(u128::MAX);
            return Ok(TypeExpr::UInt {
                width: Box::new(Expr {
                    kind: ExprKind::Int { value, width: None },
                    span: id.span,
                }),
                span: id.span,
            });
        }
        if id.name == "bool" {
            self.diags.push(
                Diagnostic::error("unknown type `bool`")
                    .at(id.span, "not a type")
                    .note("single bits are `u1`; `true` and `false` are `u1` constants"),
            );
            return Err(Fail);
        }
        let args = if self.at(&Tok::Lt) {
            Some(self.generic_args()?)
        } else {
            None
        };
        Ok(TypeExpr::Named(id, args))
    }

    // ── statements ───────────────────────────────────────────────────────

    fn block(&mut self) -> PResult<Block> {
        let start = self.expect(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            let before = self.pos;
            match self.stmt() {
                Ok(s) => stmts.push(s),
                Err(Fail) => {
                    self.recover_stmt();
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(Block {
            stmts,
            span: start.to(self.prev_span()),
        })
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        match self.peek() {
            Tok::Kw(Kw::Let) => {
                self.bump();
                let name = self.ident()?;
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.ty()?)
                } else {
                    None
                };
                let reset = if self.eat_kw(Kw::Reset) {
                    Some(self.expr()?)
                } else {
                    None
                };
                let value = if reset.is_none() && self.eat(&Tok::Eq) {
                    Some(self.expr()?)
                } else {
                    None
                };
                if value.is_none() && ty.is_none() {
                    let span = self.span();
                    self.diags.push(
                        Diagnostic::error("a net declared without a value needs a type")
                            .at(span, "expected `: <type>` or `= <value>`"),
                    );
                    return Err(Fail);
                }
                self.expect(&Tok::Semi)?;
                Ok(Stmt::Let {
                    name,
                    ty,
                    value,
                    reset,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Ident(word) if matches!(word.as_str(), "reg" | "wire") && matches!(self.peek_at(1), Tok::Ident(_)) => {
                let span = self.span();
                self.diags.push(
                    Diagnostic::error(format!("unexpected `{word}`"))
                        .at(span, "")
                        .note("declare nets with `let name: type;`; driving them with `<=` makes a register, `=` a wire"),
                );
                Err(Fail)
            }
            Tok::Kw(Kw::If) => self.if_stmt(),
            Tok::Kw(Kw::Match) => {
                self.bump();
                let scrutinee = self.expr_no_block()?;
                self.expect(&Tok::LBrace)?;
                let mut arms = Vec::new();
                while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
                    let arm_start = self.span();
                    let pats = self.patterns()?;
                    self.expect(&Tok::FatArrow)?;
                    let body = if self.at(&Tok::LBrace) {
                        let b = self.block()?;
                        self.eat(&Tok::Comma);
                        b
                    } else if self.at_kw(Kw::If) || self.at_kw(Kw::Match) {
                        let s = if self.at_kw(Kw::If) { self.if_stmt()? } else { self.stmt()? };
                        self.eat(&Tok::Comma);
                        Block {
                            span: arm_start.to(self.prev_span()),
                            stmts: vec![s],
                        }
                    } else {
                        let s = self.assign_stmt(false)?;
                        if !self.eat(&Tok::Comma) && !self.at(&Tok::RBrace) {
                            self.eat(&Tok::Semi);
                        }
                        Block {
                            span: arm_start.to(self.prev_span()),
                            stmts: vec![s],
                        }
                    };
                    arms.push(MatchArm {
                        pats,
                        body,
                        span: arm_start.to(self.prev_span()),
                    });
                }
                self.expect(&Tok::RBrace)?;
                Ok(Stmt::Match {
                    scrutinee,
                    arms,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Kw(Kw::For) => {
                self.bump();
                let var = self.ident()?;
                if !self.eat_kw(Kw::In) {
                    return Err(self.error_here("`in`"));
                }
                let range_start = self.binary(Prec::BitOr)?;
                let inclusive = if self.eat(&Tok::DotDot) {
                    false
                } else if self.eat(&Tok::DotDotEq) {
                    true
                } else {
                    return Err(self.error_here("`..` or `..=`"));
                };
                let end = self.expr_no_block()?;
                let body = self.block()?;
                Ok(Stmt::For {
                    var,
                    start: range_start,
                    end,
                    inclusive,
                    body,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Ident(_) => self.assign_stmt(true),
            _ => Err(self.error_here("a statement")),
        }
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        self.bump();
        let cond = self.expr_no_block()?;
        let then = self.block()?;
        let els = if self.eat_kw(Kw::Else) {
            if self.at_kw(Kw::If) {
                Some(Else::If(Box::new(self.if_stmt()?)))
            } else {
                Some(Else::Block(self.block()?))
            }
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then,
            els,
            span: start.to(self.prev_span()),
        })
    }

    fn assign_stmt(&mut self, need_semi: bool) -> PResult<Stmt> {
        let target = self.postfix()?;
        let op_span = self.span();
        let op = match self.peek() {
            Tok::Eq => AssignOp::Comb,
            Tok::Le => AssignOp::Reg,
            _ => return Err(self.error_here("`=` or `<=`")),
        };
        self.bump();
        let value = self.expr()?;
        if need_semi {
            self.expect(&Tok::Semi)?;
        }
        Ok(Stmt::Assign {
            target,
            op,
            op_span,
            value,
        })
    }

    fn patterns(&mut self) -> PResult<Vec<Pattern>> {
        let mut pats = Vec::new();
        loop {
            if let Tok::Ident(name) = self.peek()
                && name == "_"
            {
                pats.push(Pattern::Wildcard(self.bump().span));
            } else {
                pats.push(Pattern::Value(self.binary(Prec::BitXor)?));
            }
            if !self.eat(&Tok::Pipe) {
                return Ok(pats);
            }
        }
    }

    // ── expressions ──────────────────────────────────────────────────────

    pub fn expr(&mut self) -> PResult<Expr> {
        self.binary(Prec::LogicOr)
    }

    /// An expression in a position followed by a block (`if c {`, `match x {`):
    /// identical to `expr`, since `.si` has no struct literals to confuse it with.
    fn expr_no_block(&mut self) -> PResult<Expr> {
        self.expr()
    }

    fn binary(&mut self, min: Prec) -> PResult<Expr> {
        let mut lhs = self.cast()?;
        loop {
            let Some((op, prec)) = binop_of(self.peek()) else {
                break;
            };
            if prec < min {
                break;
            }
            self.bump();
            let rhs = self.binary(prec.next())?;
            if prec == Prec::Compare
                && let Some((_, Prec::Compare)) = binop_of(self.peek())
            {
                let span = self.span();
                self.diags.push(
                    Diagnostic::error("comparison operators cannot be chained")
                        .at(span, "")
                        .note("split it with `&&`, e.g. `a < b && b < c`"),
                );
                return Err(Fail);
            }
            let span = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn cast(&mut self) -> PResult<Expr> {
        let mut e = self.unary()?;
        while self.eat_kw(Kw::As) {
            let ty = self.ty()?;
            let span = e.span.to(ty.span());
            e = Expr {
                kind: ExprKind::Cast(Box::new(e), ty),
                span,
            };
        }
        Ok(e)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let op = match self.peek() {
            Tok::Bang => UnOp::Not,
            Tok::Minus => UnOp::Neg,
            _ => return self.postfix(),
        };
        self.bump();
        let inner = self.unary()?;
        Ok(Expr {
            span: start.to(inner.span),
            kind: ExprKind::Unary(op, Box::new(inner)),
        })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat(&Tok::LBracket) {
                let lo = self.expr()?;
                if self.at(&Tok::DotDot) || self.at(&Tok::DotDotEq) {
                    let inclusive = self.bump().tok == Tok::DotDotEq;
                    let hi = self.expr()?;
                    self.expect(&Tok::RBracket)?;
                    let span = e.span.to(self.prev_span());
                    e = Expr {
                        kind: ExprKind::Slice {
                            base: Box::new(e),
                            lo: Box::new(lo),
                            hi: Box::new(hi),
                            inclusive,
                        },
                        span,
                    };
                } else {
                    self.expect(&Tok::RBracket)?;
                    let span = e.span.to(self.prev_span());
                    e = Expr {
                        kind: ExprKind::Index(Box::new(e), Box::new(lo)),
                        span,
                    };
                }
            } else if self.eat(&Tok::Dot) {
                let name = self.ident()?;
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    while !self.at(&Tok::RParen) {
                        args.push(self.expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    let span = e.span.to(self.prev_span());
                    e = Expr {
                        kind: ExprKind::Method {
                            recv: Box::new(e),
                            name,
                            args,
                        },
                        span,
                    };
                } else {
                    let span = e.span.to(name.span);
                    e = Expr {
                        kind: ExprKind::Field(Box::new(e), name),
                        span,
                    };
                }
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Int { value, width } => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Int { value, width },
                    span: start,
                })
            }
            Tok::Kw(Kw::True) | Tok::Kw(Kw::False) => {
                let v = self.at_kw(Kw::True);
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Bool(v),
                    span: start,
                })
            }
            Tok::LParen => {
                self.bump();
                let mut e = self.expr()?;
                self.expect(&Tok::RParen)?;
                e.span = start.to(self.prev_span());
                Ok(e)
            }
            Tok::LBrace => {
                let block = self.expr_block()?;
                Ok(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                })
            }
            Tok::Kw(Kw::If) => self.if_expr(),
            Tok::Kw(Kw::Match) => {
                self.bump();
                let scrutinee = self.expr_no_block()?;
                self.expect(&Tok::LBrace)?;
                let mut arms = Vec::new();
                while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
                    let arm_start = self.span();
                    let pats = self.patterns()?;
                    self.expect(&Tok::FatArrow)?;
                    let body = self.expr()?;
                    let is_block = matches!(body.kind, ExprKind::Block(_));
                    arms.push(MatchArm {
                        pats,
                        body,
                        span: arm_start.to(self.prev_span()),
                    });
                    if !self.eat(&Tok::Comma) && !is_block && !self.at(&Tok::RBrace) {
                        return Err(self.error_here("`,` between match arms"));
                    }
                }
                self.expect(&Tok::RBrace)?;
                Ok(Expr {
                    kind: ExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
                    },
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Ident(_) => {
                let id = self.ident()?;
                if self.at(&Tok::ColonColon) {
                    self.bump();
                    if self.at(&Tok::Lt) {
                        let generics = self.generic_args()?;
                        return self.call(id, Some(generics));
                    }
                    let variant = self.ident()?;
                    return Ok(Expr {
                        span: id.span.to(variant.span),
                        kind: ExprKind::Path(id, variant),
                    });
                }
                if self.at(&Tok::LParen) {
                    return self.call(id, None);
                }
                Ok(Expr {
                    span: id.span,
                    kind: ExprKind::Name(id),
                })
            }
            _ => Err(self.error_here("an expression")),
        }
    }

    fn call(&mut self, callee: Ident, generics: Option<Vec<GenericArg>>) -> PResult<Expr> {
        self.expect(&Tok::LParen)?;
        let mut args = Vec::new();
        while !self.at(&Tok::RParen) {
            let name = if matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Colon {
                let id = self.ident()?;
                self.bump();
                Some(id)
            } else {
                None
            };
            let value = self.expr()?;
            args.push(CallArg { name, value });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(Expr {
            span: callee.span.to(self.prev_span()),
            kind: ExprKind::Call {
                callee,
                generics,
                args,
            },
        })
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        self.bump();
        let cond = self.expr_no_block()?;
        let then = self.expr_block()?;
        if !self.eat_kw(Kw::Else) {
            let span = self.span();
            self.diags.push(
                Diagnostic::error("`if` expression is missing an `else` branch")
                    .at(span, "expected `else`")
                    .note("as an expression, `if` must produce a value on both branches"),
            );
            return Err(Fail);
        }
        let els = if self.at_kw(Kw::If) {
            self.if_expr()?
        } else {
            let b = self.expr_block()?;
            Expr {
                span: b.span,
                kind: ExprKind::Block(b),
            }
        };
        Ok(Expr {
            span: start.to(self.prev_span()),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                els: Box::new(els),
            },
        })
    }

    fn expr_block(&mut self) -> PResult<ExprBlock> {
        let start = self.expect(&Tok::LBrace)?;
        let mut lets = Vec::new();
        while self.eat_kw(Kw::Let) {
            let name = self.ident()?;
            let ty = if self.eat(&Tok::Colon) {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect(&Tok::Eq)?;
            let value = self.expr()?;
            self.expect(&Tok::Semi)?;
            lets.push((name, ty, value));
        }
        let value = self.expr()?;
        if self.at(&Tok::Semi) {
            let span = self.span();
            self.diags.push(
                Diagnostic::error("unexpected `;` after the value of a block")
                    .at(span, "remove this `;`")
                    .note("an expression block ends with the value it produces"),
            );
            return Err(Fail);
        }
        self.expect(&Tok::RBrace)?;
        Ok(ExprBlock {
            lets,
            value: Box::new(value),
            span: start.to(self.prev_span()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    LogicOr,
    LogicAnd,
    Compare,
    BitOr,
    BitXor,
    BitAnd,
    Shift,
    Add,
    Mul,
    Max,
}

impl Prec {
    fn next(self) -> Prec {
        match self {
            Prec::LogicOr => Prec::LogicAnd,
            Prec::LogicAnd => Prec::Compare,
            Prec::Compare => Prec::BitOr,
            Prec::BitOr => Prec::BitXor,
            Prec::BitXor => Prec::BitAnd,
            Prec::BitAnd => Prec::Shift,
            Prec::Shift => Prec::Add,
            Prec::Add => Prec::Mul,
            Prec::Mul | Prec::Max => Prec::Max,
        }
    }
}

fn binop_of(tok: &Tok) -> Option<(BinOp, Prec)> {
    Some(match tok {
        Tok::PipePipe => (BinOp::LogicOr, Prec::LogicOr),
        Tok::AmpAmp => (BinOp::LogicAnd, Prec::LogicAnd),
        Tok::EqEq => (BinOp::Eq, Prec::Compare),
        Tok::Ne => (BinOp::Ne, Prec::Compare),
        Tok::Lt => (BinOp::Lt, Prec::Compare),
        Tok::Le => (BinOp::Le, Prec::Compare),
        Tok::Gt => (BinOp::Gt, Prec::Compare),
        Tok::Ge => (BinOp::Ge, Prec::Compare),
        Tok::Pipe => (BinOp::Or, Prec::BitOr),
        Tok::Caret => (BinOp::Xor, Prec::BitXor),
        Tok::Amp => (BinOp::And, Prec::BitAnd),
        Tok::Shl => (BinOp::Shl, Prec::Shift),
        Tok::Shr => (BinOp::Shr, Prec::Shift),
        Tok::Plus => (BinOp::Add, Prec::Add),
        Tok::Minus => (BinOp::Sub, Prec::Add),
        Tok::Star => (BinOp::Mul, Prec::Mul),
        Tok::Slash => (BinOp::Div, Prec::Mul),
        Tok::Percent => (BinOp::Rem, Prec::Mul),
        _ => return None,
    })
}
