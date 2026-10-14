//! Stamps the build date for the throughput example's result file.

fn main() {
    let date = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MT_BENCH_DATE={date}");
    println!("cargo:rerun-if-changed=build.rs");
}
