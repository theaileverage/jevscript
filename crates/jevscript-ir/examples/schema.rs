//! Prints the IR's JSON Schema to stdout.
//!
//! The spec calls this schema `spec/ir.schema.json` (section 11.1), but `spec/`
//! is owned by another agent, so this example prints instead of writing:
//!
//! ```text
//! cargo run -p jevscript-ir --example schema
//! ```

use jevscript_ir::Ir;

fn main() {
    let schema = schemars::schema_for!(Ir);
    match serde_json::to_string_pretty(&schema) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("could not render the schema: {error}");
            std::process::exit(1);
        }
    }
}
