//! The Jevscript intermediate representation.
//!
//! The IR is the interface between the compiler and the runtime (spec section
//! 11.1). It is a tree of plain data with `serde` derives; the JSON form is
//! their serialization, and the JSON Schema generated from them is the IR's
//! published schema.
//!
//! Two invariants the spec fixes:
//!
//! - every label, level and threshold appears as data, so a host can list a
//!   program's judgments, their answer spaces and its declared capabilities
//!   without executing anything;
//! - every node carries a source span, so pauses and errors point back at lines.
//!
//! Spans come from [`jevscript_syntax::Span`]: there is one definition of a
//! source position in the workspace and the IR reuses it.
//!
//! ```
//! use jevscript_ir::{IR_VERSION, Ir};
//!
//! let ir = Ir::empty("inbox_triage");
//! assert_eq!(ir.ir_version, IR_VERSION);
//! ```

#![forbid(unsafe_code)]

mod nodes;
mod shape_hash;

pub use jevscript_syntax::Span;
pub use nodes::*;
pub use shape_hash::{machine_shape_hash, shape_hash};

/// The IR version this crate reads and writes.
pub const IR_VERSION: &str = "0.1";
