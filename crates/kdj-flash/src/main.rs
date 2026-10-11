//! kdj-flash: put the deck on an SD card, and see what's on one.
//!
//!   kdj-flash                 overview: the image here, the cards plugged in
//!   kdj-flash list            removable disks and what's on them
//!   kdj-flash status [DISK]   one card in detail
//!   kdj-flash flash [IMAGE]   write the deck image to a card, then verify
//!   kdj-flash login [DISK]    add your SSH login to a card (no keyboard needed)
//!   kdj-flash format [DISK]   erase a card to one FAT32 partition
//!
//! Only ever touches removable, external disks; shows what's on one and
//! asks you to type its name before erasing it. macOS and Linux.

mod disk;
mod image;
mod inspect;
mod login;
mod names;
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
    /// The Pi's network name, reached as NAME.local (default: the card's own
    /// if it has one, else a new kdj-word-word)
    #[arg(long)]
    hostname: Option<String>,
    /// Don't set up a login (the deck still runs; nobody can SSH in)
    #[arg(long)]
    no_login: bool,
    /// The key itself, passed on when re-run with sudo
    #[arg(long = "login-key", hide = true)]
    key_lines: Vec<String>,
    /// Join this Wi-Fi network too (its password comes from your Mac's
    /// keychain, or is asked for)
    #[arg(long, value_name = "NETWORK")]
    wifi: Option<String>,
    /// The Wi-Fi country, two letters like US or GB (default: this
    /// computer's region)
    #[arg(long, value_name = "CC", requires = "wifi")]
    wifi_country: Option<String>,
    /// The Wi-Fi password comes on standard input (set when re-run with sudo)
    #[arg(long, hide = true)]
    wifi_password_stdin: bool,
}

impl LoginArgs {
    /// What to write to `disk`: its name (asked for, the card's own, or a
    /// new one) and the login, unless there won't be one (with why).
    fn resolve(&self, disk: &Disk) -> Result<login::Setup> {
        // Keep the name of a card that's one of ours; otherwise a new one.
        let keep = || card_name(disk).filter(|n| names::is_ours(n)).unwrap_or_else(names::random);
        let hostname = self.hostname.clone().unwrap_or_else(keep);
        let mut setup = login::Setup::new(&hostname, self.login()?)?;
        setup.wifi = self.wifi()?;
        Ok(setup)
    }

    /// The Wi-Fi network to join, with its password and country, if any.
    fn wifi(&self) -> Result<Option<login::Wifi>> {
        let Some(ssid) = &self.wifi else { return Ok(None) };
        let country = match &self.wifi_country {
            Some(c) => c.clone(),
            None => login::local_country()
                .context("couldn't tell this computer's region: give the Wi-Fi country with --wifi-country US")?,
        };
        let password = if self.wifi_password_stdin {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).context("reading the Wi-Fi password")?;
            line.trim_end_matches(['\r', '\n']).to_owned()
        } else {
            wifi_password(ssid)?
        };
        login::Wifi::new(ssid, &password, &country).map(Some)
    }

    fn login(&self) -> Result<Option<login::Login>> {
        if self.no_login {
            return Ok(None);
        }
        let keys =
            if self.key_lines.is_empty() { login::find_keys(self.key.as_deref())? } else { self.key_lines.clone() };
        if keys.is_empty() {
            ui::warn("No SSH public key in ~/.ssh, so no login: the deck runs, but you can't SSH in.");
            ui::hint("Make one with `ssh-keygen -t ed25519`, then `kdj-flash login` adds it to the card.");
            return Ok(None);
        }
        let user = self.user.clone().unwrap_or_else(login::default_user);
        login::Login::new(&user, keys).map(Some)
    }

    /// Arguments that carry `setup` to the same command under sudo (the
    /// Wi-Fi password goes on its standard input, never on its command line).
    fn pass_on(setup: &login::Setup) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--hostname".into(), setup.hostname.clone().into()];
        if let Some(w) = &setup.wifi {
            args.extend(["--wifi".into(), w.ssid.clone().into(), "--wifi-country".into(), w.country.clone().into()]);
            args.push("--wifi-password-stdin".into());
        }
        let Some(l) = &setup.login else {
            args.push("--no-login".into());
            return args;
        };
        args.extend(["--user".into(), l.user.clone().into()]);
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
        /// The image; the newest target/image/kdjos-*.img.xz if omitted
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
            ui::step(&format!("kdj-flash flash          put the deck on {} ({})", d.id, ui::size(d.size)));
            ui::step(&format!("kdj-flash status {:<8} look closer", d.id));
        }
        (_, Some(_)) => ui::step("kdj-flash flash --disk DISK   put the deck on one of these"),
        (_, None) => ui::step("make image, then kdj-flash flash"),
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
    let setup = a.login.resolve(&d)?;
    ui::heading("Name and login");
    show_setup(&setup);
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
        args.extend(LoginArgs::pass_on(&setup));
        return again_with_sudo(&args, a.verify, setup.wifi.as_ref().map(|w| w.password.as_str()));
    }
    ui::heading("Writing");
    disk::unmount(&d).context("unmounting the card (close anything using it)")?;
    ui::ok(&format!("Unmounted {}", d.id));
    let started = Instant::now();
    write_and_verify(&img, &d.raw, a.verify)?;
    match write_setup(&d.id, &setup) {
        Ok(()) => {
            let with = if setup.login.is_some() { ", with your login" } else { "" };
            ui::ok(&format!("Named it {}{with}", setup.hostname));
        }
        Err(e) => {
            ui::warn(&format!("Couldn't write its name and login ({e:#})"));
            ui::hint("The card is fine; add them with `kdj-flash login`.");
        }
    }
    disk::eject(&d);
    ui::ok(&format!("Done in {:.0} s. Ejected: safe to take the card out.", started.elapsed().as_secs_f64()));
    ui::heading("Next");
    ui::step("Put the card in the Pi (DAC hat on) and power it on.");
    ui::hint("First boot grows the card and restarts once, then the deck comes up full screen.");
    if setup.wifi.is_some() {
        ui::hint("No keyboard needed, ever. It joins the Wi-Fi on first boot; Settings → Network switches it off.");
    } else {
        ui::hint("No keyboard needed, ever. Plug in Ethernet (or flash with --wifi) for SSH and Ableton Link.");
    }
    if let Some(l) = &setup.login {
        ui::step(&format!("ssh {}@{}.local", l.user, setup.hostname));
    }
    Ok(())
}

/// Mount the freshly written card's boot partition and put its name and
/// login there.
fn write_setup(id: &str, setup: &login::Setup) -> Result<()> {
    let (dir, ours) = disk::mount_boot_rw(id)?;
    let written = setup.write(&dir);
    if ours {
        disk::peek_unmount(&dir);
    }
    written
}

/// `kdj-flash login`: a name and login onto a card that's already flashed.
fn add_login(id: Option<&str>, a: &LoginArgs) -> Result<()> {
    let d = pick(id, true)?;
    describe_disk(&d, local_image().as_ref(), false);
    let setup = a.resolve(&d)?;
    ui::heading("Name and login");
    show_setup(&setup);
    if cfg!(target_os = "linux") && !is_root() {
        let mut args: Vec<OsString> = vec!["login".into(), d.id.into()];
        args.extend(LoginArgs::pass_on(&setup));
        return again_with_sudo(&args, true, setup.wifi.as_ref().map(|w| w.password.as_str()));
    }
    write_setup(&d.id, &setup)?;
    ui::ok("Written. It applies at the card's next boot (first boot or not).");
    if let Some(l) = &setup.login {
        ui::step(&format!("ssh {}@{}.local", l.user, setup.hostname));
    }
    Ok(())
}

/// A Wi-Fi network's password: from the Mac's keychain if it's there (macOS
/// asks to allow it), else typed in, unseen. Empty for an open network.
fn wifi_password(ssid: &str) -> Result<String> {
    if cfg!(target_os = "macos") {
        ui::info(&format!("Looking for {ssid}'s password in your keychain: macOS may ask you to allow it."));
        let found = std::process::Command::new("security")
            .args(["find-generic-password", "-D", "AirPort network password", "-a", ssid, "-w"])
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(out) = found
            && out.status.success()
            && let Ok(pw) = String::from_utf8(out.stdout)
        {
            ui::ok("Found it");
            return Ok(pw.trim_end_matches('\n').to_owned());
        }
        ui::info("Not in the keychain (or not allowed).");
    }
    dialoguer::Password::new()
        .with_prompt(format!("Password for {ssid} (empty for an open network)"))
        .allow_empty_password(true)
        .interact()
        .context("reading the Wi-Fi password")
}

/// The name a card's user-data gives its Pi, if any.
fn card_name(d: &Disk) -> Option<String> {
    let (dir, ours) = disk::peek_mount(d)?;
    let name = login::hostname(&dir);
    if ours {
        disk::peek_unmount(&dir);
    }
    name
}

fn show_setup(s: &login::Setup) {
    ui::field("name", &format!("{}.local", s.hostname));
    if let Some(w) = &s.wifi {
        let open = if w.password.is_empty() { ", open" } else { "" };
        ui::field("wifi", &format!("{} ({}{open})", w.ssid, w.country));
    }
    let Some(l) = &s.login else {
        ui::info("No login: the deck runs, but nobody can SSH in. (--user / --key to set one.)");
        return;
    };
    ui::field("ssh", &format!("{}@{}.local", l.user, s.hostname));
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
        return again_with_sudo(&args, true, None);
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
        0 => bail!("no SD card found: insert one (or name a disk: kdj-flash status disk4)"),
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
/// `secret` goes to it on standard input (sudo itself asks on the terminal).
fn again_with_sudo(args: &[OsString], verify: bool, secret: Option<&str>) -> Result<()> {
    ui::info("Writing to a disk needs admin rights: sudo will ask for your password.");
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(exe).args(args);
    if !verify {
        cmd.arg("--no-verify");
    }
    if secret.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn().context("running sudo")?;
    if let (Some(secret), Some(mut stdin)) = (secret, child.stdin.take()) {
        use std::io::Write as _;
        writeln!(stdin, "{secret}").context("passing on the Wi-Fi password")?;
    }
    let status = child.wait().context("running sudo")?;
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
    for key in ["built", "kdj", "deck", "commit", "base"] {
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
                for key in ["built", "kdj", "deck", "commit", "base"] {
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
    if let Some(name) = &l.name {
        ui::field("name", &format!("{name}.local"));
    }
    if let Some(w) = &l.wifi {
        ui::field("wifi", w);
    }
    let ssh = if l.ssh { ", SSH switched on at next boot" } else { "" };
    match &l.found {
        login::Found::KdjFlash(user) => ui::ok(&format!("Login: {user}, by SSH key{ssh}")),
        login::Found::Other => ui::info(&format!("Login set up by Raspberry Pi Imager or by hand{ssh}")),
        login::Found::None => ui::info(&format!("No login on the card{ssh} (kdj-flash login adds one)")),
    }
}

fn booted_line(b: Booted) {
    match b {
        Booted::Yes => ui::info("Has booted on a Pi (its storage has been grown to fill the card)."),
        Booted::No => ui::info("Not booted yet."),
        Booted::Unknown => {}
    }
}
