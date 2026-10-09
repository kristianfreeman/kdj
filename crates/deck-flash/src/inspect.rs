//! What's on a card: the deck (which build, booted yet?), plain Raspberry
//! Pi OS, something else, or nothing. Reads the boot partition, which any
//! computer can, so it's the same answer on macOS and Linux.

use std::collections::BTreeMap;
use std::path::Path;

use crate::disk::{Disk, is_fat};
use crate::image::Image;

#[derive(Debug)]
pub enum Contents {
    /// No partitions.
    Empty,
    /// The deck's image; `info` from its deck-image.txt.
    Deck { info: BTreeMap<String, String>, booted: Booted, login: Login },
    /// Raspberry Pi OS without the deck; `release` from issue.txt.
    RaspberryPiOs { release: String, booted: Booted, login: Login },
    /// Partitions, but nothing we recognise.
    Other,
    /// A boot partition we couldn't look inside.
    Unreadable,
}

/// Whether the Pi has started from the card yet: its first boot grows the
/// system partition to fill the card.
/// How you'd log in to a card's Pi, as its boot partition says.
#[derive(Debug, PartialEq, Eq)]
pub struct Login {
    pub found: crate::login::Found,
    /// SSH is switched on at next boot (the `ssh` file is there).
    pub ssh: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Booted {
    Yes,
    No,
    Unknown,
}

#[must_use]
pub fn inspect(disk: &Disk) -> Contents {
    if disk.partitions.is_empty() {
        return Contents::Empty;
    }
    let booted = booted(disk);
    let Some((dir, mounted_here)) = crate::disk::peek_mount(disk) else {
        return if disk.boot_partition().is_some() { Contents::Unreadable } else { Contents::Other };
    };
    let contents = read(&dir, booted);
    if mounted_here {
        crate::disk::peek_unmount(&dir);
    }
    contents
}

fn read(dir: &Path, booted: Booted) -> Contents {
    let login = || Login { found: crate::login::found(dir), ssh: dir.join("ssh").exists() };
    if let Ok(text) = std::fs::read_to_string(dir.join("deck-image.txt")) {
        return Contents::Deck { info: key_values(&text), booted, login: login() };
    }
    if dir.join("config.txt").exists() && dir.join("cmdline.txt").exists() {
        let release = std::fs::read_to_string(dir.join("issue.txt"))
            .ok()
            .and_then(|t| t.lines().next().map(str::to_owned))
            .unwrap_or_else(|| "Raspberry Pi OS".into());
        // The deck without a build marker: installed with setup.sh, or an
        // image from before the marker.
        let config = std::fs::read_to_string(dir.join("config.txt")).unwrap_or_default();
        if config.contains("# --- deck:") {
            let mut info = BTreeMap::new();
            info.insert("image".into(), "an earlier image, or setup.sh".into());
            return Contents::Deck { info, booted, login: login() };
        }
        return Contents::RaspberryPiOs { release, booted, login: login() };
    }
    Contents::Other
}

/// Raspberry Pi OS ships its system partition small; first boot grows it to
/// the end of the card.
fn booted(disk: &Disk) -> Booted {
    let fat = disk.partitions.iter().any(|p| is_fat(&p.kind));
    let Some(system) = disk.partitions.iter().find(|p| !is_fat(&p.kind) && p.size > 0) else { return Booted::Unknown };
    if !fat {
        return Booted::Unknown;
    }
    let used: u64 = disk.partitions.iter().map(|p| p.size).sum();
    // Within 64 MB of the end: grown.
    if disk.size.saturating_sub(used) < 64_000_000 && system.size > 4_000_000_000 { Booted::Yes } else { Booted::No }
}

#[must_use]
pub fn key_values(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

/// How a card's deck compares with the image here.
#[must_use]
pub fn compare(info: &BTreeMap<String, String>, local: Option<&Image>) -> Option<String> {
    let local = local?;
    let theirs = info.get("image")?;
    let same_commit = info.get("commit").is_some_and(|c| Some(c) == local.info.get("commit"));
    Some(if *theirs == local.name && same_commit {
        "the same build as the image here".into()
    } else if info.get("built").is_some_and(|b| local.info.get("built").is_some_and(|l| b < l)) {
        format!("older than the image here ({})", local.name)
    } else {
        format!("different from the image here ({})", local.name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_values_skip_comments() {
        let kv = key_values("# about\nimage=deck-rpi4-2026-10-09\ncommit = abc123\n\nnot a pair\n");
        assert_eq!(kv.get("image").map(String::as_str), Some("deck-rpi4-2026-10-09"));
        assert_eq!(kv.get("commit").map(String::as_str), Some("abc123"));
        assert_eq!(kv.len(), 2);
    }
}
