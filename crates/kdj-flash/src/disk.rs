//! Finding disks, and deciding which ones we may touch.
//!
//! Only removable, external disks are candidates: never the disk the
//! computer runs from, never an internal one, never anything bigger than an
//! SD card plausibly is. macOS asks `diskutil`; Linux asks `lsblk`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Larger than any SD card we'd write to: refuse, it's probably a drive.
pub const MAX_CARD_BYTES: u64 = 1024 * 1_000_000_000;

#[derive(Clone, Debug)]
pub struct Partition {
    pub id: String,
    pub size: u64,
    /// Filesystem or partition type, as the OS names it.
    pub kind: String,
    pub label: String,
    pub mount: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Disk {
    /// "disk4" on macOS, "sdb" or "mmcblk0" on Linux.
    pub id: String,
    /// The device to read and write (macOS: the faster raw /dev/rdiskN).
    pub raw: PathBuf,
    pub size: u64,
    /// What it calls itself ("SD Card Reader", "Mass Storage Device").
    pub model: String,
    /// "USB", "Secure Digital", "mmc"…
    pub bus: String,
    pub removable: bool,
    /// The computer runs from it (or it holds a mounted system path).
    pub system: bool,
    pub partitions: Vec<Partition>,
}

impl Disk {
    /// Why we won't write to it, if we won't.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        if self.system {
            Some("the computer is running from it".into())
        } else if !self.removable {
            Some("it isn't a removable disk".into())
        } else if self.size > MAX_CARD_BYTES {
            Some(format!("at {} it's bigger than an SD card", crate::ui::size(self.size)))
        } else if self.size == 0 {
            Some("there's no card in it".into())
        } else {
            None
        }
    }

    /// Partitions in order, with the boot one (FAT) first if there is one.
    #[must_use]
    pub fn boot_partition(&self) -> Option<&Partition> {
        self.partitions.iter().find(|p| is_fat(&p.kind))
    }
}

/// A FAT filesystem or partition type, by any of the names the OSes use.
#[must_use]
pub fn is_fat(kind: &str) -> bool {
    let k = kind.to_ascii_lowercase();
    k.contains("fat") || k.contains("dos") || k == "vfat" || k == "0xc" || k == "0xb"
}

/// Every disk worth showing: removable and external ones, plus anything the
/// OS lists as external (shown, with the reason we won't touch it).
///
/// # Errors
/// If the OS's disk tool can't be run or its output can't be read.
pub fn list() -> Result<Vec<Disk>> {
    platform::list()
}

/// The disk named `id` ("disk4", "/dev/disk4", "sdb", "/dev/sdb").
///
/// # Errors
/// If it isn't among the listed disks.
pub fn find(id: &str) -> Result<Disk> {
    let id = id.strip_prefix("/dev/").unwrap_or(id);
    // macOS's raw device name for the same disk.
    let id = id.strip_prefix("rdisk").map_or_else(|| id.to_owned(), |n| format!("disk{n}"));
    list()?.into_iter().find(|d| d.id == id).with_context(|| format!("no removable disk called {id}"))
}

/// Unmount every partition of `disk` so it can be written.
///
/// # Errors
/// If the OS refuses (something still has files open on it).
pub fn unmount(disk: &Disk) -> Result<()> {
    platform::unmount(disk)
}

/// Eject `disk` so it can be pulled out (best effort).
pub fn eject(disk: &Disk) {
    platform::eject(disk);
}

/// Erase `disk` to one FAT32 partition called `name`.
///
/// # Errors
/// If the OS's tools fail.
pub fn format(disk: &Disk, name: &str) -> Result<()> {
    platform::format(disk, name)
}

/// Mount the boot partition read-only to look inside, if it isn't mounted:
/// returns where, and whether we mounted it (and should unmount it after).
#[must_use]
pub fn peek_mount(disk: &Disk) -> Option<(PathBuf, bool)> {
    let p = disk.boot_partition()?;
    if let Some(m) = &p.mount {
        return Some((m.clone(), false));
    }
    platform::mount_read_only(p).map(|m| (m, true))
}

/// Mount the boot partition of disk `id` writable, waiting a little for the
/// OS to see a partition table just written. Returns where, and whether we
/// mounted it (and should unmount it after).
///
/// # Errors
/// If it doesn't show up or won't mount.
pub fn mount_boot_rw(id: &str) -> Result<(PathBuf, bool)> {
    for _ in 0..30 {
        platform::reprobe(id);
        if let Ok(d) = find(id)
            && let Some(p) = d.boot_partition()
        {
            if let Some(m) = &p.mount {
                return Ok((m.clone(), false));
            }
            if let Some(m) = platform::mount_rw(p) {
                return Ok((m, true));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    bail!("the card's boot partition didn't mount")
}

/// Undo `peek_mount`.
pub fn peek_unmount(path: &std::path::Path) {
    platform::unmount_path(path);
}

fn run(cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(cmd).args(args).output().with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        bail!("{cmd} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// `run`, giving up (and killing it) after `limit`: disk tools can hang on a
/// card reader.
#[cfg_attr(not(target_os = "macos"), expect(dead_code, reason = "only macOS's diskutil hangs"))]
fn run_for(cmd: &str, args: &[&str], limit: std::time::Duration) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {cmd}"))?;
    // Read output on a thread so a full pipe can't block the wait.
    let mut stdout = child.stdout.take().context("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{cmd} {} didn't answer in {} s", args.join(" "), limit.as_secs());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let out = reader.join().map_err(|_| anyhow::anyhow!("reading {cmd}'s output"))??;
    if !status.success() {
        bail!("{cmd} {} failed ({status})", args.join(" "));
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use plist::Value;

    use super::{Disk, Partition, run, run_for};

    /// diskutil can hang asking a card reader about the whole disk; give up
    /// and ask a partition instead. Not too soon: with a few disk images
    /// mounted (Time Machine, simulators) a plain `diskutil list` takes 5 s.
    const ASK: Duration = Duration::from_secs(20);

    fn plist(args: &[&str]) -> Result<Value> {
        let out = run_for("diskutil", args, ASK)?;
        Value::from_reader_xml(out.as_slice()).context("reading diskutil's answer")
    }

    fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
        v.as_dictionary().and_then(|d| d.get(key))
    }

    fn text<'a>(v: &'a Value, key: &str) -> &'a str {
        get(v, key).and_then(Value::as_string).unwrap_or_default()
    }

    fn num(v: &Value, key: &str) -> u64 {
        get(v, key).and_then(Value::as_unsigned_integer).unwrap_or_default()
    }

    fn flag(v: &Value, key: &str) -> bool {
        get(v, key).and_then(Value::as_boolean).unwrap_or(false)
    }

    /// The partitions macOS runs from (the APFS stores behind "/").
    fn system_partitions() -> Vec<String> {
        let Ok(root) = plist(&["info", "-plist", "/"]) else { return Vec::new() };
        let mut ids = vec![text(&root, "ParentWholeDisk").to_owned()];
        if let Some(stores) = get(&root, "APFSPhysicalStores").and_then(Value::as_array) {
            ids.extend(stores.iter().map(|s| text(s, "APFSPhysicalStore").to_owned()));
        }
        ids.retain(|i| !i.is_empty());
        ids
    }

    pub fn list() -> Result<Vec<Disk>> {
        // Every physical disk: a MacBook's built-in SD slot counts as internal.
        let all = plist(&["list", "-plist", "physical"])?;
        let system = system_partitions();
        let mut entries = get(&all, "AllDisksAndPartitions").and_then(Value::as_array).cloned().unwrap_or_default();
        // For testing: attached disk images (RAM disks, .dmg) too, and
        // nothing else virtual (never APFS containers).
        if std::env::var_os("KDJ_FLASH_DISK_IMAGES").is_some() {
            let virt = plist(&["list", "-plist", "virtual"])?;
            let images = get(&virt, "AllDisksAndPartitions").and_then(Value::as_array).cloned().unwrap_or_default();
            entries.extend(images.into_iter().filter(|e| {
                let id = text(e, "DeviceIdentifier");
                // Never macOS's own images (simulator runtimes, backups):
                // nothing with a Mac filesystem on it.
                let mac = get(e, "Partitions")
                    .and_then(Value::as_array)
                    .is_some_and(|ps| ps.iter().any(|p| text(p, "Content").starts_with("Apple_")))
                    || text(e, "Content").starts_with("Apple_");
                !mac && plist(&["info", "-plist", id]).is_ok_and(|i| text(&i, "BusProtocol") == "Disk Image")
            }));
        }
        let mut disks = Vec::new();
        for e in &entries {
            let id = text(e, "DeviceIdentifier").to_owned();
            let partitions: Vec<Partition> = get(e, "Partitions")
                .and_then(Value::as_array)
                .map(|ps| ps.iter().map(partition).collect())
                .unwrap_or_default();
            // A partition answers for the media (removable, bus) and doesn't
            // hang the way the whole disk can in a card reader.
            let info = partitions
                .first()
                .and_then(|p| plist(&["info", "-plist", &p.id]).ok())
                .or_else(|| plist(&["info", "-plist", &id]).ok());
            let info = info.as_ref();
            let get_flag = |k: &str| info.is_some_and(|i| flag(i, k));
            let get_text = |k: &str| info.map(|i| text(i, k).trim().to_owned()).unwrap_or_default();
            let removable = get_flag("RemovableMedia") || get_flag("Removable") || get_flag("Ejectable");
            let mut model = get_text("MediaName");
            if model.is_empty() || model.contains(&id) {
                model = if get_text("BusProtocol") == "Secure Digital" { "SD card".into() } else { String::new() };
            }
            let on_system = system.iter().any(|s| *s == id || partitions.iter().any(|p| p.id == *s));
            disks.push(Disk {
                raw: PathBuf::from(format!("/dev/r{id}")),
                size: num(e, "Size"),
                model,
                bus: get_text("BusProtocol"),
                removable,
                system: on_system,
                partitions,
                id,
            });
        }
        Ok(disks)
    }

    fn partition(p: &Value) -> Partition {
        let mount = text(p, "MountPoint");
        Partition {
            id: text(p, "DeviceIdentifier").to_owned(),
            size: num(p, "Size"),
            kind: text(p, "Content").to_owned(),
            label: text(p, "VolumeName").to_owned(),
            mount: (!mount.is_empty()).then(|| PathBuf::from(mount)),
        }
    }

    pub fn unmount(disk: &Disk) -> Result<()> {
        run("diskutil", &["unmountDisk", &format!("/dev/{}", disk.id)]).map(drop)
    }

    pub fn eject(disk: &Disk) {
        let _ = run_for("diskutil", &["eject", &format!("/dev/{}", disk.id)], Duration::from_secs(20));
    }

    pub fn format(disk: &Disk, name: &str) -> Result<()> {
        run("diskutil", &["eraseDisk", "FAT32", name, "MBRFormat", &format!("/dev/{}", disk.id)]).map(drop)
    }

    /// Ask macOS to look at the disk again (after a raw write) and mount it.
    pub fn reprobe(id: &str) {
        let _ = run_for("diskutil", &["mountDisk", &format!("/dev/{id}")], ASK);
    }

    pub fn mount_rw(p: &Partition) -> Option<PathBuf> {
        run_for("diskutil", &["mount", &p.id], ASK).ok()?;
        let info = plist(&["info", "-plist", &p.id]).ok()?;
        let m = text(&info, "MountPoint");
        (!m.is_empty()).then(|| PathBuf::from(m))
    }

    pub fn mount_read_only(p: &Partition) -> Option<PathBuf> {
        run_for("diskutil", &["mount", "readOnly", &p.id], ASK).ok()?;
        let info = plist(&["info", "-plist", &p.id]).ok()?;
        let m = text(&info, "MountPoint");
        (!m.is_empty()).then(|| PathBuf::from(m))
    }

    pub fn unmount_path(path: &Path) {
        let _ = run_for("diskutil", &["unmount", &path.to_string_lossy()], ASK);
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};
    use serde_json::Value;

    use super::{Disk, Partition, run};

    /// Mount points that mean the system runs from this disk.
    const SYSTEM_MOUNTS: [&str; 5] = ["/", "/boot", "/boot/firmware", "/usr", "/home"];

    fn text(v: &Value, key: &str) -> String {
        v.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_owned()
    }

    fn num(v: &Value, key: &str) -> u64 {
        v.get(key).and_then(|n| n.as_u64().or_else(|| n.as_str()?.parse().ok())).unwrap_or_default()
    }

    /// lsblk's booleans are true/false in new versions, "1"/"0" in old.
    fn flag(v: &Value, key: &str) -> bool {
        v.get(key).is_some_and(|f| f.as_bool().unwrap_or_else(|| f.as_str() == Some("1") || f.as_u64() == Some(1)))
    }

    fn mounts(v: &Value) -> Vec<String> {
        let list = v
            .get("mountpoints")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
        list.unwrap_or_else(|| {
            v.get("mountpoint").and_then(Value::as_str).map(|m| vec![m.to_owned()]).unwrap_or_default()
        })
    }

    pub fn list() -> Result<Vec<Disk>> {
        let out = run(
            "lsblk",
            &["-J", "-b", "-o", "NAME,PATH,SIZE,RM,HOTPLUG,TRAN,TYPE,MODEL,LABEL,FSTYPE,PARTTYPE,MOUNTPOINTS"],
        )?;
        let all: Value = serde_json::from_slice(&out).context("reading lsblk's answer")?;
        let devices = all.get("blockdevices").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut disks = Vec::new();
        for d in devices.iter().filter(|d| text(d, "type") == "disk") {
            let children = d.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
            let tran = text(d, "tran");
            let removable = flag(d, "rm") || flag(d, "hotplug") || tran == "usb" || tran == "mmc";
            let system = std::iter::once(d)
                .chain(&children)
                .flat_map(mounts)
                .any(|m| SYSTEM_MOUNTS.contains(&m.as_str()) || m == "[SWAP]");
            let name = text(d, "name");
            let path = text(d, "path");
            disks.push(Disk {
                raw: PathBuf::from(if path.is_empty() { format!("/dev/{name}") } else { path }),
                size: num(d, "size"),
                model: text(d, "model"),
                bus: tran,
                removable,
                system,
                partitions: children.iter().map(partition).collect(),
                id: name,
            });
        }
        Ok(disks)
    }

    fn partition(p: &Value) -> Partition {
        let fs = text(p, "fstype");
        Partition {
            id: text(p, "name"),
            size: num(p, "size"),
            kind: if fs.is_empty() { text(p, "parttype") } else { fs },
            label: text(p, "label"),
            mount: mounts(p).into_iter().next().map(PathBuf::from),
        }
    }

    pub fn unmount(disk: &Disk) -> Result<()> {
        for p in &disk.partitions {
            if p.mount.is_some() {
                run("umount", &[&format!("/dev/{}", p.id)])?;
            }
        }
        Ok(())
    }

    pub fn eject(disk: &Disk) {
        let dev = disk.raw.to_string_lossy();
        if run("udisksctl", &["power-off", "-b", &dev]).is_err() {
            let _ = run("eject", &[&dev]);
        }
    }

    pub fn format(disk: &Disk, name: &str) -> Result<()> {
        let dev = disk.raw.to_string_lossy().into_owned();
        run("wipefs", &["-a", &dev])?;
        // One FAT32 (LBA) partition over the whole card, from 4 MiB.
        let mut child = std::process::Command::new("sfdisk")
            .args(["--quiet", "--label", "dos", &dev])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("running sfdisk")?;
        if let Some(stdin) = child.stdin.as_mut() {
            std::io::Write::write_all(stdin, b"start=8192, type=c\n")?;
        }
        anyhow::ensure!(child.wait()?.success(), "sfdisk failed");
        let _ = run("partprobe", &[&dev]);
        let part = if dev.ends_with(|c: char| c.is_ascii_digit()) { format!("{dev}p1") } else { format!("{dev}1") };
        run("mkfs.vfat", &["-F", "32", "-n", name, &part]).context("mkfs.vfat (install dosfstools)").map(drop)
    }

    /// Re-read the partition table after a raw write.
    pub fn reprobe(id: &str) {
        let _ = run("partprobe", &[&format!("/dev/{id}")]);
    }

    pub fn mount_rw(p: &Partition) -> Option<PathBuf> {
        let dev = format!("/dev/{}", p.id);
        // SAFETY: geteuid has no preconditions and can't fail.
        if unsafe { libc::geteuid() } == 0 {
            let dir = std::env::temp_dir().join(format!("kdj-flash-{}", p.id));
            std::fs::create_dir_all(&dir).ok()?;
            run("mount", &[&dev, &dir.to_string_lossy()]).ok()?;
            return Some(dir);
        }
        let out = run("udisksctl", &["mount", "-b", &dev]).ok()?;
        let s = String::from_utf8_lossy(&out);
        s.rsplit(" at ").next().map(|m| PathBuf::from(m.trim().trim_end_matches('.')))
    }

    pub fn mount_read_only(p: &Partition) -> Option<PathBuf> {
        // udisks mounts for the desktop user without root, under /media or /run/media.
        let out = run("udisksctl", &["mount", "-o", "ro", "-b", &format!("/dev/{}", p.id)]).ok()?;
        let s = String::from_utf8_lossy(&out);
        s.rsplit(" at ").next().map(|m| PathBuf::from(m.trim().trim_end_matches('.')))
    }

    pub fn unmount_path(path: &Path) {
        let _ = run("umount", &[&path.to_string_lossy()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(size: u64) -> Disk {
        Disk {
            id: "disk9".into(),
            raw: PathBuf::from("/dev/rdisk9"),
            size,
            model: "SD card".into(),
            bus: "Secure Digital".into(),
            removable: true,
            system: false,
            partitions: Vec::new(),
        }
    }

    #[test]
    fn only_plausible_cards_may_be_written() {
        assert_eq!(card(32_000_000_000).refusal(), None);
        assert!(Disk { system: true, ..card(32_000_000_000) }.refusal().is_some_and(|r| r.contains("running from")));
        assert!(Disk { removable: false, ..card(32_000_000_000) }.refusal().is_some());
        assert!(card(2_000_000_000_000).refusal().is_some_and(|r| r.contains("bigger than an SD card")));
        assert!(card(0).refusal().is_some(), "an empty reader");
        // The system wins over everything else.
        assert!(Disk { system: true, ..card(8_000_000_000) }.refusal().is_some());
    }

    #[test]
    fn fat_is_recognised_by_every_name() {
        for k in ["Windows_FAT_32", "DOS_FAT_32", "vfat", "0xc", "MS-DOS FAT32"] {
            assert!(is_fat(k), "{k}");
        }
        for k in ["Linux", "ext4", "Apple_APFS", ""] {
            assert!(!is_fat(k), "{k}");
        }
    }
}
