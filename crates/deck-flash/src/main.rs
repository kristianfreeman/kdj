//! deck-flash: put the deck on an SD card, and see what's on one.
//!
//!   deck-flash                 overview: the image here, the cards plugged in
//!   deck-flash list            removable disks and what's on them
//!   deck-flash status [DISK]   one card in detail
//!   deck-flash flash [IMAGE]   write the deck image to a card, then verify
//!   deck-flash login [DISK]    add your SSH login to a card (no keyboard needed)
//!   deck-flash format [DISK]   erase a card to one FAT32 partition
//!
//! Only ever touches removable, external disks; shows what's on one and
//! asks you to type its name before erasing it. macOS and Linux.

mod disk;
mod image;
mod inspect;
mod login;
mod ui;
mod write;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use console::style;

use crate::disk::Disk;
use crate::image::Image;
use crate::inspect::{Booted, Contents};

#[derive(Debug, Parser)]
#[command(version, about = "Put the deck on an SD card, and see what's on one", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

/// How you'll log in to the Pi: over SSH with your key, never a keyboard.
#[derive(Debug, clap::Args)]
struct LoginArgs {
    /// Login name on the Pi (default: yours here)
    #[arg(long)]
    user: Option<String>,
    /// SSH public key file (default: `~/.ssh/id_ed25519.pub` or similar)
    #[arg(long)]
    key: Option<PathBuf>,
    /// The Pi's hostname: reach it as NAME.local
    #[arg(long, default_value = "deck")]
    hostname: String,
    /// Don't set up a login (the deck still runs; nobody can SSH in)
    #[arg(long)]
    no_login: bool,
    /// The key itself, passed on when re-run with sudo
    #[arg(long = "login-key", hide = true)]
    key_lines: Vec<String>,
}

impl LoginArgs {
    /// The login to write, or None (with why) if there won't be one.
    fn resolve(&self) -> Result<Option<login::Login>> {
        if self.no_login {
            return Ok(None);
        }
        let keys =
            if self.key_lines.is_empty() { login::find_keys(self.key.as_deref())? } else { self.key_lines.clone() };
        if keys.is_empty() {
            ui::warn("No SSH public key in ~/.ssh, so no login: the deck runs, but you can't SSH in.");
            ui::hint("Make one with `ssh-keygen -t ed25519`, then `deck-flash login` adds it to the card.");
            return Ok(None);
        }
        let user = self.user.clone().unwrap_or_else(login::default_user);
        login::Login::new(&user, keys, &self.hostname).map(Some)
    }

    /// Arguments that carry `login` to the same command under sudo.
    fn pass_on(login: Option<&login::Login>) -> Vec<OsString> {
        let Some(l) = login else { return vec!["--no-login".into()] };
        let mut args: Vec<OsString> =
            vec!["--user".into(), l.user.clone().into(), "--hostname".into(), l.hostname.clone().into()];
        for k in &l.keys {
            args.push("--login-key".into());
            args.push(k.into());
        }
        args
    }
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Removable disks and what's on them
    List,
    /// One card in detail: partitions, what's on it, which build
    Status {
        /// The disk (disk4, sdb…); the only card if there's one
        disk: Option<String>,
    },
    /// Write the deck image to a card and verify it
    Flash {
        /// The image; the newest target/image/deck-*.img.xz if omitted
        image: Option<PathBuf>,
        /// The disk to write (disk4, sdb…); asks if there's more than one card
        #[arg(long)]
        disk: Option<String>,
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
        /// Skip reading the card back afterwards
        #[arg(long)]
        no_verify: bool,
        /// Write to this file instead of a disk (for testing)
        #[arg(long, hide = true)]
        to: Option<PathBuf>,
        /// The image's checksum was already checked (set when re-run with sudo)
        #[arg(long, hide = true)]
        checked: bool,
        #[command(flatten)]
        login: LoginArgs,
    },
    /// Add your SSH login to a card: your user and public key, SSH on.
    /// Applies at its next boot, even if it has booted before.
    Login {
        /// The disk (disk4, sdb…); the only card if there's one
        disk: Option<String>,
        #[command(flatten)]
        login: LoginArgs,
    },
    /// Erase a card to one FAT32 partition (not needed before flashing)
    Format {
        /// The disk (disk4, sdb…); asks if there's more than one card
        disk: Option<String>,
        /// The new volume's name
        #[arg(long, default_value = "SDCARD")]
        name: String,
        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => overview(),
        Some(Cmd::List) => list(),
        Some(Cmd::Status { disk }) => status(disk.as_deref()),
        Some(Cmd::Flash { image, disk, yes, no_verify, to, checked, login }) => {
            flash(&FlashArgs { image, disk, yes, verify: !no_verify, to, checked, login })
        }
        Some(Cmd::Login { disk, login }) => add_login(disk.as_deref(), &login),
        Some(Cmd::Format { disk, name, yes }) => format(disk.as_deref(), &name, yes),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n  {} {e:#}", ui::danger().apply_to("✗"));
            ExitCode::FAILURE
        }
    }
}

/// No command: the image here, every card, and what to do next.
fn overview() -> Result<()> {
    let local = local_image();
    ui::heading("Image");
    match &local {
        Some(img) => describe_image(img),
        None => ui::warn("No deck image here yet: build one with `make image`"),
    }
    let disks = disk::list()?;
    ui::heading("Cards");
    if disks.is_empty() {
        ui::info("No SD card or USB drive found. Insert one and run this again.");
        return Ok(());
    }
    for d in &disks {
        describe_disk(d, local.as_ref(), false);
    }
    let usable: Vec<&Disk> = disks.iter().filter(|d| d.refusal().is_none()).collect();
    ui::heading("Next");
    match (usable.as_slice(), &local) {
        ([], _) => ui::info("None of these can be written."),
        ([d], Some(_)) => {
            ui::step(&format!("deck-flash flash          put the deck on {} ({})", d.id, ui::size(d.size)));
            ui::step(&format!("deck-flash status {:<8} look closer", d.id));
        }
        (_, Some(_)) => ui::step("deck-flash flash --disk DISK   put the deck on one of these"),
        (_, None) => ui::step("make image, then deck-flash flash"),
    }
    Ok(())
}

fn list() -> Result<()> {
    let disks = disk::list()?;
    if disks.is_empty() {
        ui::info("No SD card or USB drive found.");
    }
    let local = local_image();
    for d in &disks {
        describe_disk(d, local.as_ref(), false);
    }
    Ok(())
}

fn status(id: Option<&str>) -> Result<()> {
    let d = pick(id, false)?;
    describe_disk(&d, local_image().as_ref(), true);
    Ok(())
}

#[derive(Debug)]
struct FlashArgs {
    image: Option<PathBuf>,
    disk: Option<String>,
    yes: bool,
    verify: bool,
    to: Option<PathBuf>,
    checked: bool,
    login: LoginArgs,
}

fn flash(a: &FlashArgs) -> Result<()> {
    let path = match &a.image {
        Some(p) => p.clone(),
        None => image::newest().context("no deck image found: build one with `make image`, or give its path")?,
    };
    let img = Image::open(&path)?;
    // The card first: refuse a wrong one before spending time on the image.
    let d = if a.to.is_some() { None } else { Some(pick(a.disk.as_deref(), true)?) };
    ui::heading("Image");
    describe_image(&img);
    if !a.checked {
        img.verify()?;
        ui::ok("Checksum matches");
    }

    // Writing to a file: the same pipeline, no disk involved.
    let (Some(d), None) = (d, &a.to) else {
        return a.to.as_ref().map_or(Ok(()), |to| write_and_verify(&img, to, a.verify).map(drop));
    };
    ui::heading("Card");
    describe_disk(&d, Some(&img), false);
    let login = a.login.resolve()?;
    ui::heading("Login");
    show_login(login.as_ref());
    if !a.yes {
        confirm(&d, &format!("Write {} to it", img.name))?;
    }
    if !is_root() && !can_write(&d.raw) {
        let mut args: Vec<OsString> = vec![
            "flash".into(),
            img.path.canonicalize()?.into(),
            "--disk".into(),
            d.id.into(),
            "--yes".into(),
            "--checked".into(),
        ];
        args.extend(LoginArgs::pass_on(login.as_ref()));
        return again_with_sudo(&args, a.verify);
    }
    ui::heading("Writing");
    disk::unmount(&d).context("unmounting the card (close anything using it)")?;
    ui::ok(&format!("Unmounted {}", d.id));
    let started = Instant::now();
    write_and_verify(&img, &d.raw, a.verify)?;
    if let Some(l) = &login {
        match write_login(&d.id, l) {
            Ok(()) => ui::ok(&format!("Login written: ssh {}@{}.local once it's up", l.user, l.hostname)),
            Err(e) => {
                ui::warn(&format!("Couldn't add the login ({e:#})"));
                ui::hint("The card is fine; add it with `deck-flash login`.");
            }
        }
    }
    disk::eject(&d);
    ui::ok(&format!("Done in {:.0} s. Ejected: safe to take the card out.", started.elapsed().as_secs_f64()));
    ui::heading("Next");
    ui::step("Put the card in the Pi (DAC hat on) and power it on.");
    ui::hint("First boot grows the card and restarts once, then the deck comes up full screen.");
    ui::hint("No keyboard needed, ever. Plug in Ethernet for SSH and Ableton Link.");
    if let Some(l) = &login {
        ui::step(&format!("ssh {}@{}.local", l.user, l.hostname));
    }
    Ok(())
}

/// Mount the freshly written card's boot partition and put the login there.
fn write_login(id: &str, l: &login::Login) -> Result<()> {
    let (dir, ours) = disk::mount_boot_rw(id)?;
    let written = l.write(&dir);
    if ours {
        disk::peek_unmount(&dir);
    }
    written
}

/// `deck-flash login`: the login onto a card that's already flashed.
fn add_login(id: Option<&str>, a: &LoginArgs) -> Result<()> {
    let d = pick(id, true)?;
    describe_disk(&d, local_image().as_ref(), false);
    let Some(l) = a.resolve()? else { bail!("nothing to add: no login") };
    ui::heading("Login");
    show_login(Some(&l));
    if cfg!(target_os = "linux") && !is_root() {
        let mut args: Vec<OsString> = vec!["login".into(), d.id.into()];
        args.extend(LoginArgs::pass_on(Some(&l)));
        return again_with_sudo(&args, true);
    }
    write_login(&d.id, &l)?;
    ui::ok("Written. It applies at the card's next boot (first boot or not).");
    ui::step(&format!("ssh {}@{}.local", l.user, l.hostname));
    Ok(())
}

fn show_login(l: Option<&login::Login>) {
    let Some(l) = l else {
        ui::info("None: the deck runs, but nobody can SSH in. (--user / --key to set one.)");
        return;
    };
    ui::field("ssh", &format!("{}@{}.local", l.user, l.hostname));
    for k in &l.keys {
        let mut parts = k.split_whitespace();
        let (kind, body, comment) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next());
        // The comment if it has one ("me@laptop"), else the key's last characters.
        let tail = body.get(body.len().saturating_sub(12)..).unwrap_or(body);
        ui::field("key", &format!("{kind} {}", comment.map_or_else(|| format!("…{tail}"), str::to_owned)));
    }
    ui::hint("No password: the key is the way in. SSH is switched on at first boot.");
}

fn write_and_verify(img: &Image, target: &Path, verify: bool) -> Result<(u64, String)> {
    let (len, hash) = write::write(img, target)?;
    ui::ok(&format!("Wrote {} to {}", ui::size(len), target.display()));
    if verify {
        write::verify(target, len, &hash)?;
        ui::ok("Read back: the card matches the image");
    }
    Ok((len, hash))
}

fn format(id: Option<&str>, name: &str, yes: bool) -> Result<()> {
    if name.is_empty() || name.len() > 11 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("a FAT32 name is 1–11 letters, digits or _");
    }
    let d = pick(id, true)?;
    describe_disk(&d, local_image().as_ref(), false);
    if !yes {
        confirm(&d, &format!("Erase it to one FAT32 volume called {name}"))?;
    }
    if cfg!(target_os = "linux") && !is_root() {
        let args: Vec<OsString> = vec!["format".into(), d.id.into(), "--name".into(), name.into(), "--yes".into()];
        return again_with_sudo(&args, true);
    }
    disk::unmount(&d).context("unmounting the card")?;
    disk::format(&d, &name.to_ascii_uppercase())?;
    ui::ok(&format!("{} is now one FAT32 volume, {}", d.id, name.to_ascii_uppercase()));
    Ok(())
}

/// The disk to work on: the one named, or the only card, or a choice.
/// With `writable`, refuses disks we won't write.
fn pick(id: Option<&str>, writable: bool) -> Result<Disk> {
    if let Some(id) = id {
        let d = disk::find(id)?;
        if let (true, Some(why)) = (writable, d.refusal()) {
            bail!("won't write to {}: {why}", d.id);
        }
        return Ok(d);
    }
    // Only cards are picked for you; name a disk to see any other.
    let disks: Vec<Disk> = disk::list()?.into_iter().filter(|d| d.refusal().is_none()).collect();
    match disks.len() {
        0 => bail!("no SD card found: insert one (or name a disk: deck-flash status disk4)"),
        1 => Ok(disks.into_iter().next().context("one disk")?),
        _ if !console::user_attended() => {
            bail!(
                "{} cards: name one ({})",
                disks.len(),
                disks.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", ")
            )
        }
        _ => {
            let items: Vec<String> =
                disks.iter().map(|d| format!("{}  {}  {}", d.id, ui::size(d.size), d.model)).collect();
            let i = dialoguer::Select::new().with_prompt("Which card?").items(&items).default(0).interact()?;
            disks.into_iter().nth(i).context("no such disk")
        }
    }
}

/// Make them type the disk's name: erasing the wrong disk is the one
/// mistake this tool exists to prevent.
fn confirm(d: &Disk, what: &str) -> Result<()> {
    if !console::user_attended() {
        bail!("not asking without a terminal: add --yes if you're sure");
    }
    println!(
        "\n  {} {what}: {} on {} ({}, {}).",
        ui::danger().apply_to("This erases everything"),
        ui::danger().apply_to("everything"),
        style(&d.id).bold(),
        ui::size(d.size),
        if d.model.is_empty() { "disk" } else { &d.model },
    );
    let typed: String = dialoguer::Input::new()
        .with_prompt(format!("  Type {} to go ahead", style(&d.id).bold()))
        .allow_empty(true)
        .interact_text()?;
    if typed.trim() != d.id {
        bail!("cancelled: nothing was changed");
    }
    Ok(())
}

/// Whether we can already write `dev` (a disk image we attached, a Linux
/// user in the disk group): then no sudo.
fn can_write(dev: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(dev.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and can't fail.
    unsafe { libc::geteuid() == 0 }
}

/// Run this same tool again under sudo for the part that needs it, after the
/// questions have been asked.
fn again_with_sudo(args: &[OsString], verify: bool) -> Result<()> {
    ui::info("Writing to a disk needs admin rights: sudo will ask for your password.");
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(exe).args(args);
    if !verify {
        cmd.arg("--no-verify");
    }
    let status = cmd.status().context("running sudo")?;
    if !status.success() {
        bail!("stopped ({status})");
    }
    Ok(())
}

fn local_image() -> Option<Image> {
    image::newest().and_then(|p| Image::open(&p).ok())
}

fn describe_image(img: &Image) {
    println!("  {} {}", style("●").cyan(), style(&img.name).bold());
    ui::field("file", &format!("{} ({})", img.path.display(), ui::size(img.compressed)));
    if let Some(n) = img.size() {
        ui::field("on the card", &ui::size(n));
    }
    for key in ["built", "deck", "commit", "base"] {
        if let Some(v) = img.info.get(key) {
            ui::field(key, v);
        }
    }
}

fn describe_disk(d: &Disk, local: Option<&Image>, detail: bool) {
    let bus = if d.bus.is_empty() { String::new() } else { format!(" · {}", d.bus) };
    let model = if d.model.is_empty() { "disk" } else { &d.model };
    let dot = if d.refusal().is_some() { style("●").dim() } else { style("●").green() };
    println!("  {dot} {}  {}  {}{}", style(&d.id).bold(), ui::size(d.size), model, style(bus).dim());
    if let Some(why) = d.refusal() {
        ui::hint(&format!("Won't write to it: {why}."));
        if !detail {
            return;
        }
    }
    if detail || d.partitions.len() <= 4 {
        for p in &d.partitions {
            let label = if p.label.is_empty() { String::new() } else { format!(" \"{}\"", p.label) };
            let mount = p.mount.as_ref().map(|m| format!(" at {}", m.display())).unwrap_or_default();
            ui::hint(&format!("{:<10} {:>8}  {}{label}{mount}", p.id, ui::size(p.size), p.kind));
        }
    }
    match inspect::inspect(d) {
        Contents::Empty => ui::info("Blank: no partitions."),
        Contents::Other => ui::info("Something else (not the deck, not Raspberry Pi OS)."),
        Contents::Unreadable => ui::warn("Couldn't look inside its boot partition."),
        Contents::RaspberryPiOs { release, booted, login } => {
            ui::info(&format!("Raspberry Pi OS, without the deck: {release}"));
            booted_line(booted);
            login_line(&login);
        }
        Contents::Deck { info, booted, login } => {
            let name = info.get("image").map_or("the deck", String::as_str);
            ui::ok(&format!("The deck: {}", style(name).bold()));
            if detail {
                for key in ["built", "deck", "commit", "base"] {
                    if let Some(v) = info.get(key) {
                        ui::field(key, v);
                    }
                }
            }
            if let Some(cmp) = inspect::compare(&info, local) {
                if cmp.starts_with("the same") { ui::ok(&cmp) } else { ui::warn(&cmp) }
            }
            booted_line(booted);
            login_line(&login);
        }
    }
}

fn login_line(l: &inspect::Login) {
    let ssh = if l.ssh { ", SSH switched on at next boot" } else { "" };
    match &l.found {
        login::Found::DeckFlash(user) => ui::ok(&format!("Login: {user}, by SSH key{ssh}")),
        login::Found::Other => ui::info(&format!("Login set up by Raspberry Pi Imager or by hand{ssh}")),
        login::Found::None => ui::info(&format!("No login on the card{ssh} (deck-flash login adds one)")),
    }
}

fn booted_line(b: Booted) {
    match b {
        Booted::Yes => ui::info("Has booted on a Pi (its storage has been grown to fill the card)."),
        Booted::No => ui::info("Not booted yet."),
        Booted::Unknown => {}
    }
}
