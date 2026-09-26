//! The committed IR schema is exactly what the `schema` example prints (spec
//! section 11.1), so it cannot drift from the types.

use jevscript_ir::Ir;

#[test]
fn the_committed_schema_is_the_generated_one() {
    let schema = schemars::schema_for!(Ir);
    let generated = serde_json::to_string_pretty(&schema).expect("renders") + "\n";
    let committed = include_str!("../ir.schema.json");
    assert_eq!(
        committed, generated,
        "crates/jevscript-ir/ir.schema.json is stale; regenerate it with \
         `cargo run -p jevscript-ir --example schema > crates/jevscript-ir/ir.schema.json`"
    );
}
