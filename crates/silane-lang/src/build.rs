//! Helpers for Cargo build scripts.

use crate::diag::Level;
use std::path::{Path, PathBuf};

/// Compiles a `.si` file to `$OUT_DIR/silane/<stem>.rs`, for use with
/// `silane::include_si!("<stem>")`.
///
/// Prints `cargo::rerun-if-changed` for every source file read, forwards warnings to
/// Cargo, and panics (failing the build) with rendered diagnostics on errors.
pub fn compile(path: impl AsRef<Path>) {
    let path = path.as_ref();
    let out_dir = PathBuf::from(
        std::env::var_os("OUT_DIR").expect("silane_lang::build must run in a build script"),
    );
    let compilation = crate::compile_file(path);
    println!("cargo::rerun-if-changed={}", path.display());
    // Built-in `std:` files are not on disk; Cargo would treat them as always changed.
    for source in compilation.source_paths() {
        if !source.to_string_lossy().starts_with("std:") {
            println!("cargo::rerun-if-changed={}", source.display());
        }
    }
    for diag in compilation.diagnostics.iter() {
        if diag.level == Level::Warning {
            for line in diag.render(&compilation.sources).lines() {
                println!("cargo::warning={line}");
            }
        }
    }
    let Some(rust) = compilation.rust() else {
        eprintln!("{}", compilation.render_diagnostics());
        panic!(
            "silane: could not compile `{}` ({} error(s))",
            path.display(),
            compilation.diagnostics.error_count()
        );
    };
    let stem = path
        .file_stem()
        .expect("source file name")
        .to_string_lossy()
        .into_owned();
    let dir = out_dir.join("silane");
    std::fs::create_dir_all(&dir).expect("create $OUT_DIR/silane");
    // Unchanged output keeps its timestamp, so rustc does not rebuild the crate.
    let out = dir.join(format!("{stem}.rs"));
    if std::fs::read_to_string(&out).ok().as_deref() != Some(rust.as_str()) {
        std::fs::write(out, rust).expect("write generated Rust");
    }
}

/// Compiles every `.si` file directly inside `dir` (see [`compile`]).
pub fn compile_dir(dir: impl AsRef<Path>) {
    let dir = dir.as_ref();
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("silane: cannot read `{}`: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "si"))
        .collect();
    files.sort();
    for file in files {
        compile(file);
    }
}
