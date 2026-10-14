//! Repository automation. `gen-docs` arrives in M6.
fn main() {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "gen-docs" => eprintln!("xtask gen-docs: not implemented yet (M6)"),
        _ => eprintln!("usage: cargo run -p xtask -- gen-docs"),
    }
}
