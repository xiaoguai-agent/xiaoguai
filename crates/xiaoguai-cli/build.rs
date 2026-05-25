//! Build script for xiaoguai-cli.
//!
//! When the environment variable `XIAOGUAI_GEN_COMPLETIONS=1` is set, writes
//! shell completions into `target/<profile>/completions/` and man pages into
//! `target/<profile>/man/` for packaging (deb, rpm, tarball).
//!
//! Usage:
//!
//! ```sh
//! XIAOGUAI_GEN_COMPLETIONS=1 cargo build --release -p xiaoguai-cli
//! ```

use clap::Command;
use clap_complete::Shell;
use std::env;
use std::io;
use std::path::PathBuf;

/// Build a minimal command tree that mirrors the real `Cli` in `main.rs`.
/// We keep only the names and structure — no heavy deps from the workspace.
fn build_cli() -> Command {
    Command::new("xiaoguai")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Xiaoguai CLI")
        .subcommand(
            Command::new("chat")
                .about("Send a one-shot prompt to the agent and print the response"),
        )
        .subcommand(
            Command::new("provider")
                .about("Administer the LLM provider registry")
                .subcommand(Command::new("register").about("Register a new provider"))
                .subcommand(Command::new("list").about("List providers"))
                .subcommand(Command::new("remove").about("Remove a provider by id")),
        )
        .subcommand(
            Command::new("mcp")
                .about("Administer the MCP server registry")
                .subcommand(Command::new("register").about("Register a new MCP server"))
                .subcommand(Command::new("list").about("List MCP servers"))
                .subcommand(Command::new("remove").about("Remove an MCP server by id")),
        )
        .subcommand(
            Command::new("remote")
                .about("Talk to a running xiaoguai-api over HTTP/SSE")
                .subcommand(Command::new("healthz").about("Smoke test the remote server"))
                .subcommand(Command::new("chat").about("Send a prompt to the remote server"))
                .subcommand(Command::new("messages").about("Fetch message history"))
                .subcommand(Command::new("cancel").about("Cancel an in-flight agent run")),
        )
        .subcommand(
            Command::new("eval")
                .about("Run an eval suite")
                .subcommand(Command::new("run").about("Walk a directory of .eval.yaml cases")),
        )
        .subcommand(
            Command::new("completions")
                .about("Write a shell completion script to stdout")
                .hide(true),
        )
        .subcommand(
            Command::new("manpages")
                .about("Generate man pages into a directory")
                .hide(true),
        )
        .subcommand(Command::new("backup").about("Create a backup archive"))
        .subcommand(Command::new("restore").about("Restore a backup archive"))
        .subcommand(Command::new("self-update").about("Check for and apply binary updates"))
}

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-env-changed=XIAOGUAI_GEN_COMPLETIONS");

    if env::var_os("XIAOGUAI_GEN_COMPLETIONS").is_none() {
        return Ok(());
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // OUT_DIR is target/debug/build/<pkg>-<hash>/out — go up 3 levels to reach
    // the profile directory (target/debug or target/release).
    let profile_dir = out_dir.ancestors().nth(3).unwrap_or(&out_dir).to_path_buf();

    // ── shell completions ──────────────────────────────────────────────────
    let completions_dir = profile_dir.join("completions");
    std::fs::create_dir_all(&completions_dir)?;
    let mut cmd = build_cli();
    for shell in [
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::PowerShell,
        Shell::Elvish,
    ] {
        clap_complete::generate_to(shell, &mut cmd, "xiaoguai", &completions_dir)?;
    }

    // ── man pages ──────────────────────────────────────────────────────────
    let man_dir = profile_dir.join("man");
    std::fs::create_dir_all(&man_dir)?;
    let top_man = clap_mangen::Man::new(cmd.clone());
    let mut buf = Vec::new();
    top_man.render(&mut buf)?;
    std::fs::write(man_dir.join("xiaoguai.1"), &buf)?;

    for sub in cmd.get_subcommands_mut() {
        let sub_name = format!("xiaoguai-{}", sub.get_name());
        sub.set_bin_name(&sub_name);
        let sub_man = clap_mangen::Man::new(sub.clone());
        let mut buf = Vec::new();
        sub_man.render(&mut buf)?;
        std::fs::write(man_dir.join(format!("{sub_name}.1")), &buf)?;
    }

    Ok(())
}
