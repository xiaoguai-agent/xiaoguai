//! `xiaoguai manpages` — generate man pages and write them to a directory.
//!
//! This is a hidden subcommand for packaging and advanced users.
//!
//! ```sh
//! xiaoguai manpages /usr/share/man/man1/
//! ```
//!
//! Generates `xiaoguai.1` plus one page per subcommand.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Generate man pages into `outdir`, creating it if it does not exist.
///
/// Returns the list of files written.
///
/// # Errors
/// Returns an error if the output directory cannot be created or a man page
/// file cannot be written.
pub fn run(cmd: &mut clap::Command, outdir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(outdir)
        .with_context(|| format!("create output directory {}", outdir.display()))?;

    let man = clap_mangen::Man::new(cmd.clone());
    let mut written = Vec::new();

    // Top-level page.
    let top_path = outdir.join("xiaoguai.1");
    let mut buf = Vec::new();
    man.render(&mut buf).context("render top-level man page")?;
    std::fs::write(&top_path, &buf).with_context(|| format!("write {}", top_path.display()))?;
    written.push(top_path);

    // One page per subcommand.
    for sub in cmd.get_subcommands_mut() {
        let sub_name = format!("xiaoguai-{}", sub.get_name());
        // Give the subcommand the composite name so the page header is correct.
        sub.set_bin_name(&sub_name);
        let sub_man = clap_mangen::Man::new(sub.clone());
        let sub_path = outdir.join(format!("{sub_name}.1"));
        let mut buf = Vec::new();
        sub_man
            .render(&mut buf)
            .with_context(|| format!("render man page for {sub_name}"))?;
        std::fs::write(&sub_path, &buf).with_context(|| format!("write {}", sub_path.display()))?;
        written.push(sub_path);
    }

    Ok(written)
}
