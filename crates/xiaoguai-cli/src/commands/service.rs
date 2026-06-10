//! `xiaoguai service install|uninstall|status` — one-command daemon setup
//! (T8.3, `docs/plans/2026-06-10-install-polish.md` §1).
//!
//! - **Linux**: writes the repo's systemd unit (embedded via `include_str!`)
//!   to `/etc/systemd/system/`, provisions the `xiaoguai` system user/group
//!   and `/var/lib/xiaoguai` + `/var/log/xiaoguai` (mirroring the rpm
//!   `%pre`/`%post` scriptlets), then `daemon-reload` + `enable` + `start`.
//!   Requires root; idempotent. Uninstall stops/disables and removes the
//!   unit but **leaves data, user and dirs in place**.
//! - **macOS**: renders a launchd user-agent plist (template embedded from
//!   `deploy/launchd/dev.xiaoguai.plist`) pointing at the current
//!   executable, installs it to `~/Library/LaunchAgents`, and
//!   `launchctl load -w`s it. Per-user — no root.
//! - **Windows**: friendly "not supported" message (use Docker or WSL).
//!
//! Template rendering and path resolution are pure fns (unit-tested); the
//! shell-out layer is deliberately thin. `--print-only` renders everything
//! and prints target paths without touching the system.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

/// systemd unit shipped in packages — single source of truth, embedded.
pub const SYSTEMD_UNIT: &str = include_str!("../../../../deploy/systemd/xiaoguai-core.service");
/// launchd plist template with `{{BINARY}}` / `{{LOG_DIR}}` placeholders.
pub const LAUNCHD_TEMPLATE: &str = include_str!("../../../../deploy/launchd/dev.xiaoguai.plist");

/// launchd job label (also the plist file stem).
pub const LAUNCHD_LABEL: &str = "dev.xiaoguai";
/// Where the unit is installed on Linux.
pub const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/xiaoguai-core.service";
/// systemd unit name (for systemctl verbs).
pub const SYSTEMD_UNIT_NAME: &str = "xiaoguai-core.service";

/// Subcommand actions, decoupled from clap so the module stays library-pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `print_only` renders artifacts + target paths with zero side effects.
    Install {
        print_only: bool,
    },
    Uninstall,
    Status,
}

// ---------------------------------------------------------------------------
// Pure: template rendering + path resolution
// ---------------------------------------------------------------------------

/// Substitute `{{BINARY}}` / `{{LOG_DIR}}` into the launchd template.
///
/// # Errors
/// Fails if any placeholder survives substitution (template drift guard).
pub fn render_launchd_plist(template: &str, binary: &Path, log_dir: &Path) -> Result<String> {
    let rendered = template
        .replace("{{BINARY}}", &binary.display().to_string())
        .replace("{{LOG_DIR}}", &log_dir.display().to_string());
    if rendered.contains("{{") {
        bail!("launchd template still contains unsubstituted placeholders");
    }
    Ok(rendered)
}

/// `~/Library/LaunchAgents/dev.xiaoguai.plist` for a given home dir.
#[must_use]
pub fn launchd_plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

/// `~/Library/Logs/xiaoguai` for a given home dir.
#[must_use]
pub fn launchd_log_dir(home: &Path) -> PathBuf {
    home.join("Library/Logs/xiaoguai")
}

/// The friendly unsupported-platform message.
#[must_use]
pub fn unsupported_message(os: &str) -> String {
    format!(
        "`xiaoguai service` is not supported on {os}.\n\
         Run xiaoguai under Docker (deploy/docker-compose.yml) or inside WSL instead."
    )
}

// ---------------------------------------------------------------------------
// Thin shell-out layer
// ---------------------------------------------------------------------------

/// Entry point for `xiaoguai service <action>`. Dispatches on the running OS.
///
/// # Errors
/// Returns errors from filesystem writes or failed `systemctl`/`launchctl`
/// invocations; on unsupported platforms returns the friendly message.
pub fn run(action: Action) -> Result<()> {
    match std::env::consts::OS {
        "linux" => run_linux(action),
        "macos" => run_macos(action),
        other => Err(anyhow!(unsupported_message(other))),
    }
}

fn home_dir() -> Result<PathBuf> {
    // $HOME is guaranteed in any login context where launchctl makes sense.
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("$HOME is not set — cannot locate ~/Library"))
}

fn current_binary() -> Result<PathBuf> {
    std::env::current_exe().context("resolve current executable path")
}

/// Run a command, capturing output; error (with stderr) on non-zero exit.
fn run_cmd(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("spawn `{program} {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`{program} {}` failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`run_cmd`] but a non-zero exit is reported, not an error — for
/// best-effort steps (e.g. unloading an agent that isn't loaded).
fn run_cmd_lenient(program: &str, args: &[&str]) -> String {
    match run_cmd(program, args) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("  (non-fatal) {e:#}");
            String::new()
        }
    }
}

// ---------------------------------------------------------------------------
// macOS — launchd user agent (no root)
// ---------------------------------------------------------------------------

fn run_macos(action: Action) -> Result<()> {
    let home = home_dir()?;
    let plist_path = launchd_plist_path(&home);
    match action {
        Action::Install { print_only } => {
            let log_dir = launchd_log_dir(&home);
            let rendered = render_launchd_plist(LAUNCHD_TEMPLATE, &current_binary()?, &log_dir)?;
            if print_only {
                println!("would write: {}", plist_path.display());
                println!("would create log dir: {}", log_dir.display());
                println!("would run: launchctl load -w {}\n", plist_path.display());
                println!("{rendered}");
                return Ok(());
            }
            std::fs::create_dir_all(&log_dir)
                .with_context(|| format!("create log dir {}", log_dir.display()))?;
            let parent = plist_path
                .parent()
                .ok_or_else(|| anyhow!("plist path has no parent"))?;
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
            std::fs::write(&plist_path, &rendered)
                .with_context(|| format!("write {}", plist_path.display()))?;
            // Idempotent re-install: unload a previous copy first (lenient —
            // first installs have nothing loaded).
            run_cmd_lenient("launchctl", &["unload", &plist_path.display().to_string()]);
            run_cmd(
                "launchctl",
                &["load", "-w", &plist_path.display().to_string()],
            )?;
            println!("✓ installed launchd agent {LAUNCHD_LABEL}");
            println!("  plist: {}", plist_path.display());
            println!("  logs:  {}", log_dir.display());
            println!("  check: xiaoguai service status   (or: xiaoguai doctor)");
            Ok(())
        }
        Action::Uninstall => {
            if plist_path.exists() {
                run_cmd_lenient(
                    "launchctl",
                    &["unload", "-w", &plist_path.display().to_string()],
                );
                std::fs::remove_file(&plist_path)
                    .with_context(|| format!("remove {}", plist_path.display()))?;
                println!("✓ uninstalled launchd agent {LAUNCHD_LABEL}");
            } else {
                println!("nothing to do — {} is not installed", plist_path.display());
            }
            println!("  data kept: ~/.xiaoguai (delete manually if you want a clean slate)");
            Ok(())
        }
        Action::Status => {
            match run_cmd("launchctl", &["list", LAUNCHD_LABEL]) {
                Ok(out) => print!("{out}"),
                Err(_) => {
                    println!(
                        "{LAUNCHD_LABEL}: not loaded — install with: xiaoguai service install"
                    );
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Linux — systemd system unit (root)
// ---------------------------------------------------------------------------

fn require_root() -> Result<()> {
    // Thin + portable: `id -u` instead of a libc dependency (the cli crate
    // is #![forbid(unsafe_code)]).
    let uid = run_cmd("id", &["-u"])?;
    if uid.trim() != "0" {
        bail!(
            "`xiaoguai service` on Linux installs a system-wide systemd unit and needs root.\n\
             Re-run with: sudo xiaoguai service install"
        );
    }
    Ok(())
}

fn run_linux(action: Action) -> Result<()> {
    match action {
        Action::Install { print_only } => {
            if print_only {
                println!("would write: {SYSTEMD_UNIT_PATH}");
                println!(
                    "would provision: user/group `xiaoguai`, /var/lib/xiaoguai, /var/log/xiaoguai"
                );
                println!("would run: systemctl daemon-reload && systemctl enable --now {SYSTEMD_UNIT_NAME}\n");
                println!("{SYSTEMD_UNIT}");
                return Ok(());
            }
            require_root()?;
            ensure_linux_user_and_dirs()?;
            std::fs::write(SYSTEMD_UNIT_PATH, SYSTEMD_UNIT)
                .with_context(|| format!("write {SYSTEMD_UNIT_PATH}"))?;
            run_cmd("systemctl", &["daemon-reload"])?;
            run_cmd("systemctl", &["enable", SYSTEMD_UNIT_NAME])?;
            run_cmd("systemctl", &["start", SYSTEMD_UNIT_NAME])?;
            println!("✓ installed + started {SYSTEMD_UNIT_NAME}");
            println!("  unit:  {SYSTEMD_UNIT_PATH}");
            println!("  check: xiaoguai service status   (or: xiaoguai doctor)");
            Ok(())
        }
        Action::Uninstall => {
            require_root()?;
            run_cmd_lenient("systemctl", &["stop", SYSTEMD_UNIT_NAME]);
            run_cmd_lenient("systemctl", &["disable", SYSTEMD_UNIT_NAME]);
            if Path::new(SYSTEMD_UNIT_PATH).exists() {
                std::fs::remove_file(SYSTEMD_UNIT_PATH)
                    .with_context(|| format!("remove {SYSTEMD_UNIT_PATH}"))?;
            }
            run_cmd("systemctl", &["daemon-reload"])?;
            println!("✓ uninstalled {SYSTEMD_UNIT_NAME}");
            println!("  data kept: /var/lib/xiaoguai and the `xiaoguai` user (remove manually if desired)");
            Ok(())
        }
        Action::Status => {
            // `systemctl status` exits non-zero for inactive units — that's
            // information, not an error.
            let out = Command::new("systemctl")
                .args(["status", SYSTEMD_UNIT_NAME, "--no-pager"])
                .output()
                .context("spawn systemctl status")?;
            print!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            Ok(())
        }
    }
}

/// Provision the `xiaoguai` system user/group + state/log dirs — a faithful
/// mirror of `packaging/rpm/scriptlets/pre_install.sh` + `post_install.sh`.
/// Idempotent: every step checks before it creates.
fn ensure_linux_user_and_dirs() -> Result<()> {
    if run_cmd("getent", &["group", "xiaoguai"]).is_err() {
        run_cmd("groupadd", &["--system", "xiaoguai"])?;
    }
    if run_cmd("id", &["-u", "xiaoguai"]).is_err() {
        run_cmd(
            "useradd",
            &[
                "--system",
                "--gid",
                "xiaoguai",
                "--home-dir",
                "/var/lib/xiaoguai",
                "--no-create-home",
                "--shell",
                "/sbin/nologin",
                "xiaoguai",
            ],
        )?;
    }
    for dir in ["/var/lib/xiaoguai", "/var/log/xiaoguai"] {
        run_cmd(
            "install",
            &["-d", "-o", "xiaoguai", "-g", "xiaoguai", "-m", "0750", dir],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_template_renders_binary_and_log_paths() {
        let rendered = render_launchd_plist(
            LAUNCHD_TEMPLATE,
            Path::new("/usr/local/bin/xiaoguai"),
            Path::new("/Users/me/Library/Logs/xiaoguai"),
        )
        .unwrap();
        assert!(rendered.contains("<string>/usr/local/bin/xiaoguai</string>"));
        assert!(rendered.contains("<string>serve</string>"));
        assert!(rendered.contains("/Users/me/Library/Logs/xiaoguai/xiaoguai.out.log"));
        assert!(rendered.contains("/Users/me/Library/Logs/xiaoguai/xiaoguai.err.log"));
        assert!(rendered.contains("<key>RunAtLoad</key>"));
        assert!(rendered.contains("<key>KeepAlive</key>"));
        assert!(rendered.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
        assert!(!rendered.contains("{{"), "no placeholder may survive");
    }

    #[test]
    fn launchd_render_rejects_template_drift() {
        let err = render_launchd_plist(
            "<string>{{BINARY}}</string><string>{{UNKNOWN}}</string>",
            Path::new("/bin/x"),
            Path::new("/logs"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("placeholders"));
    }

    #[test]
    fn launchd_paths_resolve_under_home() {
        let home = Path::new("/Users/me");
        assert_eq!(
            launchd_plist_path(home),
            PathBuf::from("/Users/me/Library/LaunchAgents/dev.xiaoguai.plist")
        );
        assert_eq!(
            launchd_log_dir(home),
            PathBuf::from("/Users/me/Library/Logs/xiaoguai")
        );
    }

    #[test]
    fn embedded_systemd_unit_is_the_packaged_one() {
        // Drift guard: the embedded unit must stay the real deploy artifact.
        assert!(SYSTEMD_UNIT.contains("ExecStart=/usr/local/bin/xiaoguai serve"));
        assert!(SYSTEMD_UNIT.contains("User=xiaoguai"));
        assert!(SYSTEMD_UNIT.contains("[Install]"));
    }

    #[test]
    fn unsupported_message_points_at_docker_and_wsl() {
        let m = unsupported_message("windows");
        assert!(m.contains("windows"));
        assert!(m.contains("Docker"));
        assert!(m.contains("WSL"));
    }
}
