//! `xiaoguai completions` — write a shell completion script to stdout.
//!
//! This is a hidden subcommand (not shown in `--help`) because it is
//! primarily for packaging scripts and advanced users.  Shells source the
//! output directly:
//!
//! ```sh
//! xiaoguai completions bash >> ~/.bash_completion
//! xiaoguai completions zsh  > ~/.zfunc/_xiaoguai
//! ```

use anyhow::Result;
use clap_complete::{generate, shells};
use std::io;

/// Which shell to generate completions for.
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Pwsh,
    Elvish,
}

/// Write the completion script for `shell` to `out`.
///
/// Using a writer parameter (rather than always writing to stdout) keeps the
/// function testable without spawning a subprocess.
///
/// # Errors
/// Returns an error if writing to `out` fails.
#[allow(clippy::needless_pass_by_value, reason = "Shell is a small enum; value semantics match clap usage")]
pub fn run<W: io::Write>(shell: Shell, cmd: &mut clap::Command, out: &mut W) -> Result<()> {
    match shell {
        Shell::Bash => generate(shells::Bash, cmd, "xiaoguai", out),
        Shell::Zsh => generate(shells::Zsh, cmd, "xiaoguai", out),
        Shell::Fish => generate(shells::Fish, cmd, "xiaoguai", out),
        Shell::Pwsh => generate(shells::PowerShell, cmd, "xiaoguai", out),
        Shell::Elvish => generate(shells::Elvish, cmd, "xiaoguai", out),
    }
    Ok(())
}
