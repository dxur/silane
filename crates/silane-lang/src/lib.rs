//! Compiler for the Silane hardware description language (`.si`).
//!
//! A `.si` file is parsed, fully elaborated (parameters resolved, loops unrolled,
//! procedural assignments lowered to multiplexers) and checked, then emitted as Rust
//! code that builds the same circuit through the `silane` library. See `docs/` for
//! the language reference.
//!
//! In a build script:
//!
//! ```ignore
//! fn main() {
//!     silane_lang::build::compile("hw/fifo.si");
//! }
//! ```
//!
//! and in the crate:
//!
//! ```ignore
//! mod fifo {
//!     silane::include_si!("fifo");
//! }
//! ```

pub mod ast;
pub mod build;
pub mod check;
pub mod codegen;
pub mod diag;
pub mod domains;
pub mod elab;
pub mod ir;
pub mod lexer;
pub mod netlist;
pub mod parser;

use diag::{Diagnostic, Diagnostics, SourceMap, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The result of compiling a `.si` file and everything it imports.
pub struct Compilation {
    pub sources: SourceMap,
    pub diagnostics: Diagnostics,
    /// The elaborated design, present when there were no errors.
    pub design: Option<ir::Design>,
    origin: String,
}

impl Compilation {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }

    /// All diagnostics, rendered rustc-style.
    pub fn render_diagnostics(&self) -> String {
        self.diagnostics.render(&self.sources)
    }

    /// Generated Rust code, if compilation succeeded.
    pub fn rust(&self) -> Option<String> {
        self.design
            .as_ref()
            .map(|d| codegen::generate(d, &self.sources, &self.origin))
    }

    /// Builds module `name` (a module, alias or mangled instance name) as a circuit,
    /// without going through Rust. Returns `None` for unknown modules and for designs
    /// that use modules implemented in Rust (`extern module`).
    pub fn circuit(&self, name: &str) -> Option<silane::Circuit> {
        let design = self.design.as_ref()?;
        let idx = design.find(name)?;
        if netlist::uses_rust_modules(design, idx) {
            return None;
        }
        Some(netlist::build_circuit(design, idx))
    }

    /// Paths of every source file that was read.
    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.sources.files.iter().map(|f| f.path.clone()).collect()
    }
}

/// Compiles the file at `path` and the files it imports.
pub fn compile_file(path: impl AsRef<Path>) -> Compilation {
    let path = path.as_ref();
    compile_with(path, &mut |p: &Path| std::fs::read_to_string(p).map_err(|e| e.to_string()))
}

/// Compiles in-memory sources; `files` maps file names (as used by `import`) to text,
/// and the first entry is the root.
pub fn compile_sources(files: &[(&str, &str)]) -> Compilation {
    let map: HashMap<PathBuf, String> = files
        .iter()
        .map(|(n, s)| (PathBuf::from(n), s.to_string()))
        .collect();
    let root = PathBuf::from(files.first().map_or("main.si", |f| f.0));
    compile_with(&root, &mut |p: &Path| {
        map.get(p)
            .cloned()
            .ok_or_else(|| "no such file".to_string())
    })
}

/// Compiles a single in-memory source file.
pub fn compile_str(name: &str, src: &str) -> Compilation {
    compile_sources(&[(name, src)])
}

fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
        }
        out
    })
}

/// Sources of the built-in `std:` imports.
const STD_FILES: &[(&str, &str)] = &[
    ("std:cdc", include_str!("std/cdc.si")),
    ("std:mem", include_str!("std/mem.si")),
];

fn compile_with(root: &Path, read: &mut dyn FnMut(&Path) -> Result<String, String>) -> Compilation {
    let mut sources = SourceMap::default();
    let mut diagnostics = Diagnostics::default();
    let mut files = Vec::new();
    let mut seen: HashMap<PathBuf, ()> = HashMap::new();
    let mut queue: Vec<(PathBuf, Option<Span>)> = vec![(root.to_path_buf(), None)];

    while let Some((path, import_span)) = queue.pop() {
        let key = normalize(&path);
        if seen.insert(key, ()).is_some() {
            continue;
        }
        let builtin = STD_FILES
            .iter()
            .find(|(name, _)| Path::new(name) == path)
            .map(|(_, src)| src.to_string());
        let src = match builtin.map_or_else(|| read(&path), Ok) {
            Ok(src) => src,
            Err(e) => {
                let diag = Diagnostic::error(format!("cannot read `{}`: {e}", path.display()));
                diagnostics.push(match import_span {
                    Some(span) => diag.at(span, "imported here"),
                    None => diag,
                });
                continue;
            }
        };
        let id = sources.add(path.clone(), src);
        let tokens = lexer::lex(id, &sources.file(id).src, &mut diagnostics);
        let file = parser::parse(tokens, &mut diagnostics);
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for item in &file.items {
            if let ast::Item::Import { path: rel, span } = item {
                let target = if rel.starts_with("std:") {
                    PathBuf::from(rel)
                } else {
                    dir.join(rel)
                };
                queue.push((target, Some(*span)));
            }
        }
        files.push(file);
    }

    let origin = root
        .file_name()
        .map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
    let design = if diagnostics.has_errors() {
        None
    } else {
        let design = elab::elaborate(&files, &mut diagnostics);
        (!diagnostics.has_errors()).then_some(design)
    };
    Compilation {
        sources,
        diagnostics,
        design,
        origin,
    }
}
