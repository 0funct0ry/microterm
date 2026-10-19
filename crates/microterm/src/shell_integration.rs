//! `microterm shell-integration bash|zsh|fish` (SPEC §12.4): the snippet a shell sources so
//! that it emits OSC 133 prompt marks (`A` prompt start, `B` input start, `C` output start,
//! `D;status` finished) and OSC 7 working directories. microterm never injects it for you.

use crate::cli::Shell;

const BASH: &str = include_str!("shell_integration/integration.bash");
const ZSH: &str = include_str!("shell_integration/integration.zsh");
const FISH: &str = include_str!("shell_integration/integration.fish");

pub fn snippet(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => BASH,
        Shell::Zsh => ZSH,
        Shell::Fish => FISH,
    }
}

pub fn print(shell: Shell) -> i32 {
    print!("{}", snippet(shell));
    0
}
