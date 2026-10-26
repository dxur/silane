//! Tokenizer for `.si` source files.

use crate::diag::{Diagnostic, Diagnostics, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    /// Integer literal with an optional width suffix (`42`, `0xFF_u8`).
    Int { value: u128, width: Option<u32> },
    Str(String),
    Kw(Kw),

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    DotDotEq,
    FatArrow,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    AmpAmp,
    Pipe,
    PipePipe,
    Caret,
    Bang,
    Shl,
    Shr,
    Arrow,
    /// `@`, before a port's domain label.
    At,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kw {
    Module,
    In,
    Let,
    Reset,
    Extern,
    Const,
    Enum,
    If,
    Else,
    Match,
    For,
    Import,
    As,
    True,
    False,
}

impl Kw {
    fn from_str(s: &str) -> Option<Kw> {
        Some(match s {
            "module" => Kw::Module,
            "in" => Kw::In,
            "let" => Kw::Let,
            "reset" => Kw::Reset,
            "extern" => Kw::Extern,
            "const" => Kw::Const,
            "enum" => Kw::Enum,
            "if" => Kw::If,
            "else" => Kw::Else,
            "match" => Kw::Match,
            "for" => Kw::For,
            "import" => Kw::Import,
            "as" => Kw::As,
            "true" => Kw::True,
            "false" => Kw::False,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Tok::Ident(name) => return write!(f, "identifier `{name}`"),
            Tok::Int { .. } => "integer literal",
            Tok::Str(_) => "string literal",
            Tok::Kw(kw) => return write!(f, "keyword `{}`", format!("{kw:?}").to_lowercase()),
            Tok::LParen => "`(`",
            Tok::RParen => "`)`",
            Tok::LBrace => "`{`",
            Tok::RBrace => "`}`",
            Tok::LBracket => "`[`",
            Tok::RBracket => "`]`",
            Tok::Comma => "`,`",
            Tok::Semi => "`;`",
            Tok::Colon => "`:`",
            Tok::ColonColon => "`::`",
            Tok::Dot => "`.`",
            Tok::DotDot => "`..`",
            Tok::DotDotEq => "`..=`",
            Tok::FatArrow => "`=>`",
            Tok::Eq => "`=`",
            Tok::EqEq => "`==`",
            Tok::Ne => "`!=`",
            Tok::Lt => "`<`",
            Tok::Le => "`<=`",
            Tok::Gt => "`>`",
            Tok::Ge => "`>=`",
            Tok::Plus => "`+`",
            Tok::Minus => "`-`",
            Tok::Star => "`*`",
            Tok::Slash => "`/`",
            Tok::Percent => "`%`",
            Tok::Amp => "`&`",
            Tok::AmpAmp => "`&&`",
            Tok::Pipe => "`|`",
            Tok::PipePipe => "`||`",
            Tok::Caret => "`^`",
            Tok::At => "`@`",
            Tok::Bang => "`!`",
            Tok::Shl => "`<<`",
            Tok::Shr => "`>>`",
            Tok::Arrow => "`->`",
            Tok::Eof => "end of file",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(file: u32, src: &str, diags: &mut Diagnostics) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let span = |lo: usize, hi: usize| Span::new(file, lo, hi);

    while i < bytes.len() {
        let c = bytes[i];
        let start = i;

        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < bytes.len() {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                diags.push(
                    Diagnostic::error("unterminated block comment")
                        .at(span(start, start + 2), "comment starts here"),
                );
            }
            continue;
        }

        if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            let tok = match Kw::from_str(word) {
                Some(kw) => Tok::Kw(kw),
                None => Tok::Ident(word.to_string()),
            };
            tokens.push(Token {
                tok,
                span: span(start, i),
            });
            continue;
        }

        if c.is_ascii_digit() {
            let (radix, digits_start) = match (c, bytes.get(i + 1)) {
                (b'0', Some(b'x')) => (16, i + 2),
                (b'0', Some(b'b')) => (2, i + 2),
                (b'0', Some(b'o')) => (8, i + 2),
                _ => (10, i),
            };
            i = digits_start;
            let mut value: Option<u128> = Some(0);
            let mut any_digit = false;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch == '_' {
                    i += 1;
                    continue;
                }
                // A `u` starts the width suffix (it is never a digit, even in hex).
                if ch == 'u' {
                    break;
                }
                match ch.to_digit(radix) {
                    Some(d) => {
                        value = value
                            .and_then(|v| v.checked_mul(radix as u128))
                            .and_then(|v| v.checked_add(d as u128));
                        any_digit = true;
                        i += 1;
                    }
                    None if ch.is_ascii_alphanumeric() => {
                        let bad_start = i;
                        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                            i += 1;
                        }
                        diags.push(
                            Diagnostic::error(format!("invalid digit for a base {radix} literal"))
                                .at(span(bad_start, bad_start + 1), "invalid digit"),
                        );
                        break;
                    }
                    None => break,
                }
            }
            let mut width = None;
            if i < bytes.len() && bytes[i] == b'u' {
                let suffix_start = i;
                i += 1;
                let digits_from = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                match src[digits_from..i].parse::<u32>() {
                    Ok(w) => width = Some(w),
                    Err(_) => diags.push(
                        Diagnostic::error("invalid integer suffix")
                            .at(span(suffix_start, i), "expected a width like `u8`"),
                    ),
                }
            }
            if !any_digit {
                diags.push(
                    Diagnostic::error("integer literal has no digits").at(span(start, i), ""),
                );
            }
            let value = value.unwrap_or_else(|| {
                diags.push(
                    Diagnostic::error("integer literal is too large").at(span(start, i), ""),
                );
                0
            });
            tokens.push(Token {
                tok: Tok::Int { value, width },
                span: span(start, i),
            });
            continue;
        }

        if c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] != b'"' {
                diags.push(
                    Diagnostic::error("unterminated string literal")
                        .at(span(start, i), "string starts here"),
                );
            } else {
                i += 1;
            }
            let text = src[start + 1..i.saturating_sub(1).max(start + 1)].to_string();
            tokens.push(Token {
                tok: Tok::Str(text),
                span: span(start, i),
            });
            continue;
        }

        let three = src.get(i..i + 3);
        let two = src.get(i..i + 2);
        let (tok, len) = if three == Some("..=") {
            (Tok::DotDotEq, 3)
        } else {
            match two {
                Some("::") => (Tok::ColonColon, 2),
                Some("->") => (Tok::Arrow, 2),
                Some("..") => (Tok::DotDot, 2),
                Some("=>") => (Tok::FatArrow, 2),
                Some("==") => (Tok::EqEq, 2),
                Some("!=") => (Tok::Ne, 2),
                Some("<=") => (Tok::Le, 2),
                Some(">=") => (Tok::Ge, 2),
                Some("&&") => (Tok::AmpAmp, 2),
                Some("||") => (Tok::PipePipe, 2),
                Some("<<") => (Tok::Shl, 2),
                Some(">>") => (Tok::Shr, 2),
                _ => {
                    let tok = match c {
                        b'(' => Tok::LParen,
                        b')' => Tok::RParen,
                        b'{' => Tok::LBrace,
                        b'}' => Tok::RBrace,
                        b'[' => Tok::LBracket,
                        b']' => Tok::RBracket,
                        b',' => Tok::Comma,
                        b';' => Tok::Semi,
                        b':' => Tok::Colon,
                        b'.' => Tok::Dot,
                        b'=' => Tok::Eq,
                        b'<' => Tok::Lt,
                        b'>' => Tok::Gt,
                        b'+' => Tok::Plus,
                        b'-' => Tok::Minus,
                        b'*' => Tok::Star,
                        b'/' => Tok::Slash,
                        b'%' => Tok::Percent,
                        b'&' => Tok::Amp,
                        b'|' => Tok::Pipe,
                        b'^' => Tok::Caret,
                        b'!' => Tok::Bang,
                        b'@' => Tok::At,
                        _ => {
                            let ch_len = src[i..].chars().next().map_or(1, |ch| ch.len_utf8());
                            diags.push(
                                Diagnostic::error(format!(
                                    "unexpected character `{}`",
                                    &src[i..i + ch_len]
                                ))
                                .at(span(i, i + ch_len), ""),
                            );
                            i += ch_len;
                            continue;
                        }
                    };
                    (tok, 1)
                }
            }
        };
        i += len;
        tokens.push(Token {
            tok,
            span: span(start, i),
        });
    }

    tokens.push(Token {
        tok: Tok::Eof,
        span: span(src.len(), src.len()),
    });
    tokens
}
