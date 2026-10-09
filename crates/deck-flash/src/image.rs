//! The deck's image: finding the newest build, what it is, and checking it
//! against its published checksum.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::ui;

#[derive(Debug)]
pub struct Image {
    pub path: PathBuf,
    /// "deck-rpi4-2026-10-09".
    pub name: String,
    /// From the .info file next to it (built, commit, base…).
    pub info: BTreeMap<String, String>,
    /// From the .sha256 file next to it.
    pub sha256: Option<String>,
    pub compressed: u64,
}

impl Image {
    /// # Errors
    /// If the file can't be read.
    pub fn open(path: &Path) -> Result<Self> {
        let compressed = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.len();
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let name = file.trim_end_matches(".xz").trim_end_matches(".img").to_owned();
        let side = |ext: &str| std::fs::read_to_string(format!("{}.{ext}", path.display())).ok();
        let info = side("info").map(|t| crate::inspect::key_values(&t)).unwrap_or_default();
        let sha256 = side("sha256").and_then(|t| t.split_whitespace().next().map(str::to_ascii_lowercase));
        Ok(Self { path: path.to_path_buf(), name, info, sha256, compressed })
    }

    /// Bytes once decompressed, if the .info file says.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.info.get("size").and_then(|s| s.parse().ok())
    }

    /// Hash the file and compare with its .sha256, with a progress bar.
    ///
    /// # Errors
    /// If it doesn't match, or can't be read.
    pub fn verify(&self) -> Result<()> {
        let Some(want) = &self.sha256 else { bail!("no {}.sha256 next to it to check against", self.name) };
        let bar = ui::bytes_bar(self.compressed, "checking");
        let mut f = std::fs::File::open(&self.path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 4 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            bar.inc(n as u64);
        }
        bar.finish_and_clear();
        let got = hex(&h.finalize());
        if got != *want {
            bail!("the image is damaged: its checksum is {got}, not {want}");
        }
        Ok(())
    }
}

/// The newest deck image under `target/image`, looking up from the current
/// directory for the repository.
#[must_use]
pub fn newest() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let dir = cwd.ancestors().map(|d| d.join("target/image")).find(|d| d.is_dir())?;
    let mut images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().is_some_and(|f| {
                let f = f.to_string_lossy();
                f.starts_with("deck-") && f.ends_with(".img.xz")
            })
        })
        .collect();
    images.sort();
    images.pop()
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
