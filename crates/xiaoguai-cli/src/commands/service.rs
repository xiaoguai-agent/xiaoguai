//! `xiaoguai service install|uninstall|status` — one-command daemon setup
//! (T8.3, `docs/plans/2026-06-10-install-polish.md` §1).
//!
//! - **Linux**: renders the repo's systemd unit (embedded via `include_str!`)
//!   with `ExecStart` pointing at the **current executable** (#287 — aligned
//!   with the macOS path; pip/cargo installs don't live in
//!   `/usr/local/bin`), preflights the binary location + `/etc/xiaoguai/
//!   config.yaml` before touching the system, writes the unit to
//!   `/etc/systemd/system/`, provisions the `xiaoguai` system user/group
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
/// Config file the systemd unit's `ExecStart` points at — must exist before
/// install or the service loops in failure (#287).
pub const SYSTEMD_CONFIG_PATH: &str = "/etc/xiaoguai/config.yaml";

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

/// Escape the five predefined XML entities for plist text nodes (#287).
///
/// Install paths can legally contain `&`, `<`, `'`, … — substituting them
/// raw would produce an invalid plist that launchd rejects at load time.
#[must_use]
pub fn xml_escape(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Substitute `{{BINARY}}` / `{{LOG_DIR}}` into the launchd template.
/// Values are XML-escaped (#287) — paths may contain `&` / `<` / quotes.
///
/// # Errors
/// Fails if any placeholder survives substitution (template drift guard).
pub fn render_launchd_plist(template: &str, binary: &Path, log_dir: &Path) -> Result<String> {
    let rendered = template
        .replace("{{BINARY}}", &xml_escape(&binary.display().to_string()))
        .replace("{{LOG_DIR}}", &xml_escape(&log_dir.display().to_string()));
    if rendered.contains("{{") {
        bail!("launchd template still contains unsubstituted placeholders");
    }
    Ok(rendered)
}

/// Render the embedded systemd unit with `ExecStart` pointing at `binary`
/// instead of the packaged `/usr/local/bin/xiaoguai` (#287 — aligned with
/// the macOS `current_exe()` path so pip/cargo installs get a working unit).
/// The arguments after the binary token (`serve --config …`) are preserved.
///
/// # Errors
/// Fails when the binary path contains whitespace (systemd `ExecStart` is
/// split on spaces; a quoted path is more fragile than asking the operator
/// to copy the binary), or when the template does not contain exactly one
/// `ExecStart=` line (drift guard).
pub fn render_systemd_unit(template: &str, binary: &Path) -> Result<String> {
    let bin = binary.display().to_string();
    if bin.is_empty() || bin.chars().any(char::is_whitespace) {
        bail!(
            "binary path `{bin}` contains whitespace — systemd ExecStart cannot point at it \
             safely; copy the binary first: sudo install -m 0755 <binary> /usr/local/bin/xiaoguai"
        );
    }
    let exec_start_lines = template
        .lines()
        .filter(|l| l.trim_start().starts_with("ExecStart="))
        .count();
    if exec_start_lines != 1 {
        bail!(
            "systemd unit template drift: expected exactly one ExecStart= line, \
             found {exec_start_lines}"
        );
    }
    let rendered: Vec<String> = template
        .lines()
        .map(|line| match line.trim_start().strip_prefix("ExecStart=") {
            Some(rest) => {
                // Keep everything after the old binary token (`serve --config …`).
                let args = rest.split_once(' ').map_or("", |(_, a)| a);
                if args.is_empty() {
                    format!("ExecStart={bin}")
                } else {
                    format!("ExecStart={bin} {args}")
                }
            }
            None => line.to_string(),
        })
        .collect();
    Ok(rendered.join("\n") + "\n")
}

/// Preflight the Linux install inputs **before** writing any unit (#287).
///
/// The previous behaviour wrote a unit hardcoding `/usr/local/bin/xiaoguai`
/// with zero checks — pip (`~/.local/bin`, venv) and cargo (`~/.cargo/bin`)
/// installs then produced an enabled-but-failing service restarting every
/// 5s (203/EXEC, or a missing-config crash loop). Pure: the caller resolves
/// `binary` via `current_exe()` and `config_exists` via the filesystem.
///
/// # Errors
/// - `binary` is not absolute (systemd requires an absolute `ExecStart`).
/// - `binary` lives under `/home`, `/root`, `/tmp` or `/var/tmp` — the
///   hardened unit sets `ProtectHome=true` + `PrivateTmp=true`, so the
///   service user cannot execute it (203/EXEC).
/// - `config_exists` is false — `ExecStart` passes
///   `--config /etc/xiaoguai/config.yaml`, which only deb/rpm postinst seeds.
pub fn preflight_linux_install(binary: &Path, config_exists: bool) -> Result<()> {
    // ProtectHome=true / PrivateTmp=true in the hardened unit make these
    // prefixes invisible to the service user → systemd fails with 203/EXEC.
    const SHIELDED_PREFIXES: [&str; 4] = ["/home/", "/root/", "/tmp/", "/var/tmp/"];

    if !binary.is_absolute() {
        bail!(
            "cannot resolve an absolute path for the running binary ({}) — \
             re-run from an absolute path or copy it: \
             sudo install -m 0755 <binary> /usr/local/bin/xiaoguai",
            binary.display()
        );
    }
    let bin_str = binary.display().to_string();
    if let Some(prefix) = SHIELDED_PREFIXES.iter().find(|p| bin_str.starts_with(*p)) {
        bail!(
            "the running binary lives under {prefix} ({bin_str}), which the hardened systemd \
             unit cannot execute (ProtectHome/PrivateTmp). Copy it to a system path first:\n  \
             sudo install -m 0755 {bin_str} /usr/local/bin/xiaoguai\n\
             then re-run: sudo /usr/local/bin/xiaoguai service install"
        );
    }
    if !config_exists {
        bail!(
            "{SYSTEMD_CONFIG_PATH} does not exist — the service starts with \
             `--config {SYSTEMD_CONFIG_PATH}` and would crash-loop without it. Create it first:\n  \
             sudo mkdir -p /etc/xiaoguai\n  \
             sudo cp deploy/config.example.yaml {SYSTEMD_CONFIG_PATH}   # then edit it\n\
             (the .deb/.rpm packages seed this file automatically)"
        );
    }
    Ok(())
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
            // #287: render ExecStart from the *running* binary (pip/cargo
            // installs don't live in /usr/local/bin) and preflight before
            // touching the system — never write a unit doomed to 203/EXEC.
            let binary = current_binary()?;
            let config_exists = Path::new(SYSTEMD_CONFIG_PATH).exists();
            let rendered = render_systemd_unit(SYSTEMD_UNIT, &binary)?;
            if print_only {
                println!("would write: {SYSTEMD_UNIT_PATH}");
                println!(
                    "would provision: user/group `xiaoguai`, /var/lib/xiaoguai, /var/log/xiaoguai"
                );
                println!("would run: systemctl daemon-reload && systemctl enable --now {SYSTEMD_UNIT_NAME}");
                // Surface the preflight verdict without failing — print-only
                // must stay side-effect-free and always print. (#287)
                match preflight_linux_install(&binary, config_exists) {
                    Ok(()) => println!("preflight: ok\n"),
                    Err(e) => println!("preflight would FAIL: {e:#}\n"),
                }
                println!("{rendered}");
                return Ok(());
            }
            require_root()?;
            preflight_linux_install(&binary, config_exists)?;
            ensure_linux_user_and_dirs()?;
            std::fs::write(SYSTEMD_UNIT_PATH, &rendered)
                .with_context(|| format!("write {SYSTEMD_UNIT_PATH}"))?;
            run_cmd("systemctl", &["daemon-reload"])?;
            run_cmd("systemctl", &["enable", SYSTEMD_UNIT_NAME])?;
            run_cmd("systemctl", &["start", SYSTEMD_UNIT_NAME])?;
            println!("✓ installed + started {SYSTEMD_UNIT_NAME}");
            println!("  unit:  {SYSTEMD_UNIT_PATH}");
            println!("  exec:  {}", binary.display()); // #287: rendered, not hardcoded
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

    // ---- #287: XML escaping for plist rendering ----

    #[test]
    fn xml_escape_covers_the_five_entities() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        // Already-plain text passes through untouched.
        assert_eq!(
            xml_escape("/usr/local/bin/xiaoguai"),
            "/usr/local/bin/xiaoguai"
        );
        // No double-escaping surprises on the output of a previous pass.
        assert_eq!(xml_escape("&amp;"), "&amp;amp;");
    }

    #[test]
    fn launchd_render_escapes_xml_in_paths() {
        let rendered = render_launchd_plist(
            LAUNCHD_TEMPLATE,
            Path::new("/Users/a&b/bin/xiaoguai"),
            Path::new("/Users/a&b/Library/Logs/<xiaoguai>"),
        )
        .unwrap();
        assert!(rendered.contains("<string>/Users/a&amp;b/bin/xiaoguai</string>"));
        assert!(rendered.contains("/Users/a&amp;b/Library/Logs/&lt;xiaoguai&gt;/xiaoguai.out.log"));
        assert!(!rendered.contains("a&b"), "raw ampersand must not survive");
    }

    // ---- #287: systemd unit rendering from current_exe ----

    #[test]
    fn systemd_render_points_exec_start_at_the_given_binary() {
        let rendered =
            render_systemd_unit(SYSTEMD_UNIT, Path::new("/opt/xiaoguai/bin/xiaoguai")).unwrap();
        assert!(rendered.contains(
            "ExecStart=/opt/xiaoguai/bin/xiaoguai serve --config /etc/xiaoguai/config.yaml"
        ));
        assert!(
            !rendered.contains("ExecStart=/usr/local/bin/xiaoguai"),
            "hardcoded path must be replaced"
        );
        // Everything else survives untouched (hardening, install section).
        assert!(rendered.contains("User=xiaoguai"));
        assert!(rendered.contains("ProtectHome=true"));
        assert!(rendered.contains("[Install]"));
    }

    #[test]
    fn systemd_render_rejects_whitespace_in_binary_path() {
        let err = render_systemd_unit(SYSTEMD_UNIT, Path::new("/opt/my dir/xiaoguai")).unwrap_err();
        assert!(err.to_string().contains("whitespace"));
        assert!(err.to_string().contains("/usr/local/bin/xiaoguai"));
    }

    #[test]
    fn systemd_render_guards_against_template_drift() {
        let none = render_systemd_unit("[Service]\nUser=x\n", Path::new("/bin/x")).unwrap_err();
        assert!(none.to_string().contains("exactly one ExecStart"));

        let two = render_systemd_unit(
            "ExecStart=/a serve\nExecStart=/b serve\n",
            Path::new("/bin/x"),
        )
        .unwrap_err();
        assert!(two.to_string().contains("found 2"));
    }

    #[test]
    fn systemd_render_handles_exec_start_without_args() {
        let rendered = render_systemd_unit("ExecStart=/old/bin\n", Path::new("/new/bin")).unwrap();
        assert_eq!(rendered, "ExecStart=/new/bin\n");
    }

    // ---- #287: Linux install preflight ----

    #[test]
    fn preflight_passes_for_system_binary_with_config() {
        assert!(preflight_linux_install(Path::new("/usr/local/bin/xiaoguai"), true).is_ok());
        assert!(preflight_linux_install(Path::new("/opt/xiaoguai/xiaoguai"), true).is_ok());
    }

    #[test]
    fn preflight_rejects_home_and_tmp_binaries_with_copy_advice() {
        for bin in [
            "/home/me/.local/bin/xiaoguai", // pip --user
            "/home/me/.cargo/bin/xiaoguai", // cargo install
            "/root/.cargo/bin/xiaoguai",    // cargo as root
            "/tmp/xiaoguai",                // scratch download
            "/var/tmp/build/xiaoguai",      // scratch build
        ] {
            let err = preflight_linux_install(Path::new(bin), true).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("203/EXEC") || msg.contains("ProtectHome"),
                "{bin}: {msg}"
            );
            assert!(
                msg.contains("sudo install -m 0755"),
                "{bin}: must give actionable copy advice"
            );
        }
    }

    #[test]
    fn preflight_rejects_missing_config_with_seed_advice() {
        let err = preflight_linux_install(Path::new("/usr/local/bin/xiaoguai"), false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(SYSTEMD_CONFIG_PATH));
        assert!(msg.contains("config.example.yaml"));
    }

    #[test]
    fn preflight_rejects_relative_binary_path() {
        let err = preflight_linux_install(Path::new("xiaoguai"), true).unwrap_err();
        assert!(err.to_string().contains("absolute"));
    }
}
