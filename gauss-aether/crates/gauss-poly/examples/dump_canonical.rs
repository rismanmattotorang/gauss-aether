//! Emits the canonical probe set as pretty-printed JSON. Run via
//! `cargo run -p gauss-poly --example dump_canonical >
//! src/snapshots/canonical.json` to regenerate the baseline.

#![allow(clippy::print_stdout)]

fn main() {
    let set = gauss_poly::canonical();
    let s = serde_json::to_string_pretty(&set).expect("serialise");
    println!("{s}");
}
