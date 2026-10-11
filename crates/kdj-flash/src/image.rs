//! The deck's image: finding the newest build, what it is, and checking it
//! against its published checksum.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::ui;

/// An image built from older code than the checkout's (`Image::behind`).
#[derive(Debug)]
pub struct Behind {
    /// The commit it was built from ("494cca8", or "…-dirty" for
    /// uncommitted changes on top of it).
    pub built: String,
    pub head: String,
    /// Commits since, if git knows.
    pub commits: Option<usize>,
    pub repo: PathBuf,
}

impl Behind {
    /// "built from 494cca8, 2 commits behind the code (70fbe2c)".
    #[must_use]
    pub fn describe(&self) -> String {
        let how_far = match self.commits {
            Some(0) => "built from uncommitted changes".to_owned(),
            Some(1) => "1 commit behind the code".to_owned(),
            Some(n) => format!("{n} commits behind the code"),
            None => "not built from the code here".to_owned(),
        };
        format!("built from {}, {how_far} ({})", self.built, self.head)
    }
}

#[derive(Debug)]
pub struct Image {
    pub path: PathBuf,
    /// "kdjos-rpi4-2026-10-09".
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

    /// Where the image stands against the code it was built from, if it's
    /// in a git checkout (as `make image` leaves it): None when it's of the
    /// latest commit.
    #[must_use]
    pub fn behind(&self) -> Option<Behind> {
        let built = self.info.get("commit")?;
        let repo = self.path.ancestors().find(|d| d.join(".git").exists())?;
        let git = |args: &[&str]| -> Option<String> {
            let out = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        let head = git(&["rev-parse", "--short", "HEAD"])?;
        let clean = built.strip_suffix("-dirty");
        let base = clean.unwrap_or(built);
        if clean.is_none() && head.starts_with(base) {
            return None;
        }
        let commits = git(&["rev-list", "--count", &format!("{base}..HEAD")]).and_then(|n| n.parse().ok());
        Some(Behind { built: built.clone(), head, commits, repo: repo.to_path_buf() })
    }

    /// Bytes once decompressed: from the .info file, or the xz index.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.info.get("size").and_then(|s| s.parse().ok()).or_else(|| xz_size(&self.path))
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

/// The uncompressed size an .xz file records in its index (at the end of
/// the file), so a write can show real progress. One stream, optionally
/// followed by stream padding; None for anything else.
///
/// Layout (xz file format 1.0.4): a 12-byte stream footer ends the file:
/// CRC32, backward size ((index size / 4) - 1), flags, "YZ". The index
/// before it: 0x00, record count, then (unpadded, uncompressed) size pairs,
/// all as 7-bit little-endian varints.
#[must_use]
pub fn xz_size(path: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    // Skip stream padding (zero bytes, in fours) after the footer.
    let mut end = len;
    let mut word = [0u8; 4];
    while end >= 16 {
        f.seek(SeekFrom::Start(end - 4)).ok()?;
        f.read_exact(&mut word).ok()?;
        if word != [0; 4] {
            break;
        }
        end -= 4;
    }
    let mut footer = [0u8; 12];
    f.seek(SeekFrom::Start(end.checked_sub(12)?)).ok()?;
    f.read_exact(&mut footer).ok()?;
    if footer[10..] != *b"YZ" {
        return None;
    }
    let backward = u64::from(u32::from_le_bytes([footer[4], footer[5], footer[6], footer[7]]));
    let index_len = (backward + 1) * 4;
    let mut index = vec![0u8; usize::try_from(index_len).ok()?];
    f.seek(SeekFrom::Start(end.checked_sub(12 + index_len)?)).ok()?;
    f.read_exact(&mut index).ok()?;
    index_total(&index)
}

/// Sum of the uncompressed sizes in an xz index.
fn index_total(index: &[u8]) -> Option<u64> {
    let (&indicator, mut rest) = index.split_first()?;
    if indicator != 0 {
        return None;
    }
    let records = varint(&mut rest)?;
    let mut total = 0u64;
    for _ in 0..records {
        let _unpadded = varint(&mut rest)?;
        total = total.checked_add(varint(&mut rest)?)?;
    }
    Some(total)
}

/// One xz multibyte integer: 7 bits a byte, low first, at most 9 bytes.
fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..9 {
        let (&b, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
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
                (f.starts_with("kdjos-") || f.starts_with("deck-")) && f.ends_with(".img.xz")
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn the_uncompressed_size_comes_from_the_xz_index() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut enc = liblzma::write::XzEncoder::new(Vec::new(), 1);
        enc.write_all(&data).expect("compress");
        let mut xz = enc.finish().expect("finish");
        let path = std::env::temp_dir().join(format!("kdj-flash-test-{}.xz", std::process::id()));
        std::fs::write(&path, &xz).expect("write");
        assert_eq!(xz_size(&path), Some(300_000));
        // Stream padding after the footer is allowed.
        xz.extend_from_slice(&[0; 8]);
        std::fs::write(&path, &xz).expect("write");
        assert_eq!(xz_size(&path), Some(300_000));
        std::fs::write(&path, b"not xz at all, just some bytes").expect("write");
        assert_eq!(xz_size(&path), None);
        std::fs::remove_file(&path).expect("clean up");
    }

    /// The real image, if one's been built: its index gives the size the
    /// write produced (3,858,759,680 bytes for 2026-10-09's).
    #[test]
    #[ignore = "needs an image in target/image"]
    fn the_built_image_reports_its_size() {
        let p = newest().expect("an image in target/image");
        let size = xz_size(&p).expect("a readable index");
        println!("{}: {size} bytes", p.display());
        assert!(size > 1_000_000_000 && size.is_multiple_of(512), "{size}");
    }
}
