//! Decompress the image straight onto the card, then read it back and
//! compare. The image is hashed as it's written, so the read-back proves
//! the card holds exactly that.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use liblzma::read::XzDecoder;
use sha2::{Digest, Sha256};

use crate::image::{Image, hex};
use crate::ui;

/// Bytes per write: big enough to be fast, a whole number of sectors.
const CHUNK: usize = 4 << 20;
const SECTOR: usize = 512;

/// Write `image` to `target` (a raw disk device, or a file for testing);
/// returns the bytes written and their SHA-256.
///
/// # Errors
/// If the image can't be read or the target written.
pub fn write(image: &Image, target: &Path) -> Result<(u64, String)> {
    let input = File::open(&image.path).with_context(|| format!("opening {}", image.path.display()))?;
    let mut xz = XzDecoder::new_multi_decoder(std::io::BufReader::with_capacity(1 << 20, input));
    let mut out = OpenOptions::new()
        .write(true)
        .create(!is_device(target))
        .truncate(!is_device(target))
        .open(target)
        .with_context(|| format!("opening {} to write", target.display()))?;
    let bar = ui::bytes_bar(image.size().unwrap_or(0), "writing");
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = fill(&mut xz, &mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        // Raw devices take whole sectors; the image is too, but be safe.
        let padded = n.div_ceil(SECTOR) * SECTOR;
        buf[n..padded].fill(0);
        out.write_all(&buf[..padded]).with_context(|| format!("writing at {}", ui::size(total)))?;
        total += n as u64;
        if image.size().is_none() {
            bar.set_length(total);
        }
        bar.set_position(total);
    }
    bar.set_message("finishing");
    flush(&out, target)?;
    bar.finish_and_clear();
    drop_cache(&out);
    Ok((total, hex(&hash.finalize())))
}

/// Read the first `len` bytes of `target` back and check their SHA-256.
///
/// # Errors
/// If they differ, or can't be read.
pub fn verify(target: &Path, len: u64, want: &str) -> Result<()> {
    let mut f = File::open(target).with_context(|| format!("opening {} to read back", target.display()))?;
    drop_cache(&f);
    let bar = ui::bytes_bar(len, "verifying");
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut left = len;
    while left > 0 {
        // Read whole sectors; hash only what was written.
        let want_n = usize::try_from(left).unwrap_or(CHUNK).min(CHUNK);
        let n = fill(&mut f, &mut buf[..want_n.div_ceil(SECTOR) * SECTOR])?;
        if n == 0 {
            bail!("the card ended {} early", ui::size(left));
        }
        let used = n.min(want_n);
        hash.update(&buf[..used]);
        left -= used as u64;
        bar.inc(used as u64);
    }
    bar.finish_and_clear();
    let got = hex(&hash.finalize());
    if got != want {
        bail!("what's on the card doesn't match the image (the card may be failing): {got} ≠ {want}");
    }
    Ok(())
}

/// Make sure everything written is on the card. For a file, a full sync.
/// A raw disk device is unbuffered on macOS, and refuses the full-flush
/// request std's `sync_all` makes there (`F_FULLFSYNC`: "Inappropriate ioctl
/// for device"), so a device gets a plain fsync, and that refusal is fine.
fn flush(out: &File, target: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;
    if !is_device(target) {
        return out.sync_all().context("flushing to the file");
    }
    // SAFETY: `out` is an open file descriptor for the duration of the call.
    if unsafe { libc::fsync(out.as_raw_fd()) } == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOTTY | libc::EINVAL | libc::ENOTSUP) => Ok(()),
        _ => Err(err).context("flushing to the card"),
    }
}

/// Read until `buf` is full or the input ends.
fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(n)
}

fn is_device(p: &Path) -> bool {
    p.starts_with("/dev")
}

/// Make the next read come from the card, not memory (Linux caches block
/// devices; macOS's raw devices don't).
#[cfg(target_os = "linux")]
fn drop_cache(f: &File) {
    use std::os::fd::AsRawFd;
    // SAFETY: `f` is open for the duration of the call; advice has no
    // other effect on it.
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}

#[cfg(not(target_os = "linux"))]
const fn drop_cache(_: &File) {}
