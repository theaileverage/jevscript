//! The Jevscript language server: `jevscript lsp`.
//!
//! It speaks the Language Server Protocol over stdio and reuses the real front
//! end rather than a second parser. The lexer and parser of
//! `jevscript-syntax` read every buffer, the compiler of
//! `jevscript-compiler` links, checks and lowers it exactly as
//! `jevscript check` does, and its module resolver follows `use` into other
//! files (spec sections 3.9, 12 and 13).
//!
//! What it adds on top is what an editor needs and a compiler does not:
//!
//! - [`recover`]: a parse that survives a half-typed buffer, by re-parsing
//!   with the broken top-level declaration blanked out;
//! - [`index`]: what every name in a file refers to;
//! - [`world`]: open buffers, files on disk, and the modules between them;
//! - one module per feature: [`diagnostics`], [`semantic`] tokens, hover,
//!   definition and references in [`navigate`], [`outline`] and folding,
//!   [`completion`];
//! - [`server`]: the protocol loop.
//!
//! ```no_run
//! jevscript_lsp::run_stdio().expect("the session ends cleanly");
//! ```

#![forbid(unsafe_code)]

pub mod completion;
pub mod diagnostics;
pub mod docs;
pub mod index;
pub mod line_index;
pub mod navigate;
pub mod outline;
pub mod recover;
pub mod semantic;
pub mod server;
pub mod world;

pub use server::{BoxError, run, run_stdio};
