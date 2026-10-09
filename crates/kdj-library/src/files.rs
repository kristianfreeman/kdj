//! Reading what a USB stick says, without trusting it: stored paths stay on
//! the stick, and files are size-checked before they're read whole.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Analysis files are tens to hundreds of KB.
pub const MAX_ANLZ_BYTES: u64 = 64 << 20;
/// A 10,000-track export.pdb is a few tens of MB.
pub const MAX_PDB_BYTES: u64 = 256 << 20;

/// Read a regular file whole, refusing anything bigger than `max` bytes.
///
/// # Errors
/// If it isn't a regular file, is too big, or can't be read.
pub fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} isn't a file", path.display());
    }
    if meta.len() > max {
        bail!("{} is too big ({} bytes)", path.display(), meta.len());
    }
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// `stored` (a path as the export recorded it, `/Contents/…`) under `root`.
/// None if it would climb out of `root` or is empty.
#[must_use]
pub fn under(root: &Path, stored: &str) -> Option<PathBuf> {
    let rel = Path::new(stored.trim_start_matches('/'));
    let mut parts = rel.components().peekable();
    parts.peek()?;
    parts.all(|c| matches!(c, Component::Normal(_) | Component::CurDir)).then(|| root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_paths_stay_on_the_stick() {
        let root = Path::new("/media/usb");
        assert_eq!(under(root, "/Contents/a/b.mp3"), Some(PathBuf::from("/media/usb/Contents/a/b.mp3")));
        assert_eq!(under(root, "/../../dev/zero"), None);
        assert_eq!(under(root, "/Contents/../../etc/passwd"), None);
        assert_eq!(under(root, ""), None);
    }

    #[test]
    fn devices_and_directories_are_refused() {
        assert!(read_capped(Path::new("/dev/zero"), 1024).is_err());
        assert!(read_capped(Path::new("/"), 1024).is_err());
    }
}
