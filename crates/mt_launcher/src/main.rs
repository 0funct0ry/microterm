//! The `microterm` command users get on `$PATH` on macOS (SPEC §15.5).
//!
//! macOS takes the Dock icon and menu-bar identity from the `.app` that LaunchServices started,
//! so a GUI launch from a shell must go through LaunchServices. Everything else is the app
//! binary's own behaviour, so every subcommand is `exec()`ed into `microterm-app` and the CLI
//! text comes from the same `mt_cli` definition.

#[cfg(target_os = "macos")]
mod handoff;
#[cfg(target_os = "macos")]
mod launch;

#[cfg(target_os = "macos")]
fn main() {
    launch::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("microterm-launcher is only used on macOS; run the `microterm` binary directly");
    std::process::exit(1);
}
