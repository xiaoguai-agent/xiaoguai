//! `xiaoguai self-update` — fetch and apply the latest release binary.
//!
//! # Flow
//!
//! 1. Query `https://api.github.com/repos/xiaoguai-agent/xiaoguai/releases/latest`
//!    and parse `tag_name` + asset list.
//! 2. Compare `tag_name` against the running binary's `CARGO_PKG_VERSION`.
//!    Exit with an informational message if already up-to-date.
//! 3. Download the matching tarball, `.sig`, and `.pem` for the current platform.
//! 4. Verify the cosign signature by shelling out to `cosign verify-blob` with
//!    the pinned identity regexp and OIDC issuer from the v1.1.6.3 runbook.
//! 5. Extract the binary from the tarball and replace the running executable
//!    atomically via tempfile + rename.
//!
//! `--check` exits after step 2 without downloading anything.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// ── constants ──────────────────────────────────────────────────────────────

const RELEASES_URL: &str = "https://api.github.com/repos/xiaoguai-agent/xiaoguai/releases/latest";

/// Certificate identity regexp from the v1.1.6.3 runbook.
const CERT_IDENTITY_REGEXP: &str =
    "https://github.com/xiaoguai-agent/xiaoguai/.github/workflows/release-tarball.yml@.*";

const CERT_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

// ── GitHub API types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

// ── platform target ────────────────────────────────────────────────────────

/// Return the Rust target triple for the current platform.
fn current_target() -> &'static str {
    // These are compiled in via cfg; we cover the two main Linux targets and
    // macOS.  Add more as needed.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-unknown-linux-gnu";

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-unknown-linux-gnu";

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin";

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin";

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return "unknown";
}

// ── version comparison ─────────────────────────────────────────────────────

/// Return true if `remote_tag` (e.g. `v1.2.15`) is strictly newer than the
/// compiled-in `CARGO_PKG_VERSION`.
pub fn is_newer(remote_tag: &str) -> bool {
    let remote = remote_tag.trim_start_matches('v');
    parse_semver(remote) > parse_semver(env!("CARGO_PKG_VERSION"))
}

/// Parse a semver string into `(major, minor, patch)`.  Exposed for testing.
pub fn parse_semver_pub(s: &str) -> (u64, u64, u64) {
    parse_semver(s)
}

fn parse_semver(s: &str) -> (u64, u64, u64) {
    let mut parts = s.splitn(3, '.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

// ── cosign verification ────────────────────────────────────────────────────

/// Shell out to `cosign verify-blob` with the bundled trust policy.
///
/// `tarball`, `sig`, and `pem` are local filesystem paths.
///
/// Returns `Ok(())` if verification passes, `Err` otherwise.
pub fn cosign_verify(tarball: &Path, sig: &Path, pem: &Path) -> Result<()> {
    let cosign = which_cosign().context(
        "cosign not found on PATH. Install cosign from https://github.com/sigstore/cosign/releases",
    )?;

    let status = std::process::Command::new(&cosign)
        .args([
            "verify-blob",
            "--certificate",
            &pem.to_string_lossy(),
            "--signature",
            &sig.to_string_lossy(),
            "--certificate-identity-regexp",
            CERT_IDENTITY_REGEXP,
            "--certificate-oidc-issuer",
            CERT_OIDC_ISSUER,
        ])
        .arg(tarball)
        .status()
        .context("spawn cosign")?;

    if !status.success() {
        bail!(
            "cosign verification FAILED for {}. Do not use this release.",
            tarball.display()
        );
    }
    Ok(())
}

fn which_cosign() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let c = dir.join("cosign");
            if c.is_file() {
                Some(c)
            } else {
                None
            }
        })
    })
}

// ── tarball extraction ─────────────────────────────────────────────────────

/// Extract the `xiaoguai` binary from `tarball_bytes` (gzip-compressed tar).
///
/// Looks for an entry named `xiaoguai` or `bin/xiaoguai` in the archive.
pub fn extract_binary(tarball_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = flate2::read::GzDecoder::new(tarball_bytes);
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .context("decompress tarball")?;

    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let path = entry.path().context("tar entry path")?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "xiaoguai" {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).context("read binary data")?;
            return Ok(data);
        }
    }
    bail!("could not find 'xiaoguai' binary in release tarball");
}

// ── HTTP helpers ───────────────────────────────────────────────────────────

/// Fetch a URL and return the response body bytes (blocking via reqwest sync
/// feature isn't available; callers run this inside tokio).
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("xiaoguai/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build HTTP client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} → HTTP {}", resp.status());
    }
    resp.bytes()
        .await
        .context("read response body")
        .map(|b| b.to_vec())
}

// ── self-update ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SelfUpdateArgs {
    /// If true, only check and report; do not download or apply.
    pub check: bool,
    /// Override the GitHub releases API URL (for testing).
    pub api_url: Option<String>,
}

/// Run `xiaoguai self-update`.
pub async fn run_self_update(args: SelfUpdateArgs) -> Result<()> {
    let api_url = args.api_url.as_deref().unwrap_or(RELEASES_URL);

    // 1. Fetch release metadata.
    let body = fetch_bytes(api_url)
        .await
        .context("fetch release metadata from GitHub")?;
    let release: GithubRelease =
        serde_json::from_slice(&body).context("parse GitHub release JSON")?;

    let remote_tag = &release.tag_name;

    // 2. Version comparison.
    if !is_newer(remote_tag) {
        println!(
            "Already up-to-date (running {}, latest is {remote_tag}).",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    println!(
        "New release available: {} (running {})",
        remote_tag,
        env!("CARGO_PKG_VERSION")
    );

    if args.check {
        println!("Run without --check to apply the update.");
        return Ok(());
    }

    // 3. Find tarball + sig + pem for this platform.
    let target = current_target();
    let version_no_v = remote_tag.trim_start_matches('v');
    let base_name = format!("xiaoguai-{version_no_v}-{target}");

    let tarball_asset = find_asset(&release.assets, &format!("{base_name}.tar.gz"))
        .with_context(|| format!("no tarball asset for {base_name}"))?;
    let sig_asset = find_asset(&release.assets, &format!("{base_name}.tar.gz.sig"))
        .with_context(|| format!("no .sig asset for {base_name}"))?;
    let pem_asset = find_asset(&release.assets, &format!("{base_name}.tar.gz.pem"))
        .with_context(|| format!("no .pem asset for {base_name}"))?;

    // 4. Download to a temp dir.
    let tmp_dir = tempfile::TempDir::new().context("create temp dir for download")?;

    let tarball_path = tmp_dir.path().join(format!("{base_name}.tar.gz"));
    let sig_path = tmp_dir.path().join(format!("{base_name}.tar.gz.sig"));
    let pem_path = tmp_dir.path().join(format!("{base_name}.tar.gz.pem"));

    println!("Downloading {}…", tarball_asset.name);
    let tarball_bytes = fetch_bytes(&tarball_asset.browser_download_url)
        .await
        .context("download tarball")?;
    std::fs::write(&tarball_path, &tarball_bytes).context("write tarball to temp")?;

    println!("Downloading signature artefacts…");
    let sig_bytes = fetch_bytes(&sig_asset.browser_download_url)
        .await
        .context("download .sig")?;
    std::fs::write(&sig_path, &sig_bytes).context("write .sig to temp")?;

    let pem_bytes = fetch_bytes(&pem_asset.browser_download_url)
        .await
        .context("download .pem")?;
    std::fs::write(&pem_path, &pem_bytes).context("write .pem to temp")?;

    // 5. Cosign verification.
    println!("Verifying cosign signature…");
    cosign_verify(&tarball_path, &sig_path, &pem_path).context("cosign verification")?;
    println!("Signature verified OK.");

    // 6. Extract binary.
    let new_binary = extract_binary(&tarball_bytes).context("extract xiaoguai binary")?;

    // 7. Atomic replace.
    let current_exe = std::env::current_exe().context("locate current executable")?;
    let exe_dir = current_exe.parent().unwrap_or_else(|| Path::new("."));

    let mut tmp_exe =
        tempfile::NamedTempFile::new_in(exe_dir).context("create temp file for new binary")?;
    tmp_exe
        .write_all(&new_binary)
        .context("write new binary to temp")?;

    // Set executable permission on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tmp_exe.as_file().set_permissions(perms).ok();
    }

    tmp_exe
        .persist(&current_exe)
        .with_context(|| format!("replace {} with new binary", current_exe.display()))?;

    println!("Updated to {remote_tag} successfully.");
    Ok(())
}

fn find_asset<'a>(assets: &'a [GithubAsset], name: &str) -> Option<&'a GithubAsset> {
    assets.iter().find(|a| a.name == name)
}
