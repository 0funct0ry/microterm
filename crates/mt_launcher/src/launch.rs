use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use mt_cli::Cli;
use mt_proto::SpawnParams;

use crate::handoff;

/// Environment passed to a freshly started app (LaunchServices does not forward the shell's).
fn forwarded_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            matches!(k.as_ref(), "PATH" | "SHELL" | "LANG" | "SSH_AUTH_SOCK")
                || k.starts_with("XDG_")
                || k.starts_with("MICROTERM_")
        })
        .collect()
}

/// The `.app` this launcher lives in: `X.app/Contents/Resources/bin/microterm`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.ancestors().nth(4)?;
    bundle
        .extension()
        .is_some_and(|e| e == "app")
        .then(|| bundle.to_path_buf())
}

/// The app binary: inside the bundle, else next to the launcher (dev builds, tests).
fn app_binary(exe: &Path) -> PathBuf {
    if let Some(b) = bundle_of(exe) {
        let p = b.join("Contents/MacOS/microterm-app");
        if p.exists() {
            return p;
        }
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    let sibling = dir.join("microterm-app");
    if sibling.exists() {
        return sibling;
    }
    dir.join("microterm")
}

pub fn main() {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("microterm-launcher"));
    let cli = Cli::parse_or_exit();
    if cli.version {
        print!("{}", Cli::version_text(cli.verbose));
        return;
    }
    // Everything but a plain GUI launch is the app binary's job: same tty, stdio, signals and
    // exit status.
    if cli.command.is_some() || cli.headless {
        exec_app(&exe);
    }
    gui(cli, &exe);
}

fn exec_app(exe: &Path) -> ! {
    let err = Command::new(app_binary(exe))
        .arg0("microterm")
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("microterm: cannot run the app binary: {err}");
    std::process::exit(1);
}

fn gui(cli: Cli, exe: &Path) {
    let cwd = std::env::current_dir().ok();
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    if !cli.new_instance {
        let params = SpawnParams {
            workspace: cli.workspace.clone(),
            cwd: cli
                .cwd
                .as_deref()
                .map(abs)
                .or_else(|| cwd.clone())
                .map(|p| p.to_string_lossy().into_owned()),
            command: (!cli.exec.is_empty()).then(|| cli.exec.clone()),
            ..Default::default()
        };
        if handoff::try_running_instance(&params) {
            return;
        }
    }
    let Some(bundle) = bundle_of(exe) else {
        // Not inside a bundle (dev build): start the app binary directly.
        exec_app(exe);
    };
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n").arg("-a").arg(&bundle);
    if cli.wait {
        open.arg("-W");
    }
    for (k, v) in forwarded_env() {
        let mut kv = k;
        kv.push("=");
        kv.push(v);
        open.arg("--env").arg(kv);
    }
    open.arg("--args");
    // LaunchServices starts the app in `/`: pass the caller's directory, made absolute.
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut seen_cwd = false;
    let mut drop_wait = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_string_lossy().into_owned();
        if a == "-e" {
            break;
        }
        if a == "--wait" {
            drop_wait.push(i);
        } else if a == "--cwd" && i + 1 < args.len() {
            args[i + 1] = abs(Path::new(&args[i + 1])).into_os_string();
            seen_cwd = true;
            i += 1;
        } else if let Some(v) = a.strip_prefix("--cwd=") {
            args[i] = format!("--cwd={}", abs(Path::new(v)).display()).into();
            seen_cwd = true;
        }
        i += 1;
    }
    if !seen_cwd {
        if let Some(c) = &cwd {
            open.arg("--cwd").arg(c);
        }
    }
    for i in drop_wait.into_iter().rev() {
        args.remove(i);
    }
    open.args(&args);
    match open.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("microterm: cannot start {}: {e}", bundle.display());
            std::process::exit(1);
        }
    }
}
