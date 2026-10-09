//! What a card's Pi is called and how you log in to it, without ever
//! plugging in a keyboard: its network name, and optionally your user and
//! SSH public key with SSH on. Written to the card's boot partition for
//! Raspberry Pi OS to apply on first boot (cloud-init's user-data, and the
//! `ssh` file its sshswitch service looks for). No password anywhere: the
//! account's password is locked, the key is the way in.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Marks user-data we wrote, so status can say so.
const MARK: &str = "# Written by kdj-flash";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Login {
    pub user: String,
    /// Public keys, one per line as in `authorized_keys`.
    pub keys: Vec<String>,
}

/// Everything written to a card: its name, and a login if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The Pi's network name: reach it as NAME.local.
    pub hostname: String,
    pub login: Option<Login>,
}

/// Groups the default Raspberry Pi OS user is in that matter here (all
/// exist on Raspberry Pi OS, so cloud-init can add the user to them).
const GROUPS: &str = "[adm, audio, video, plugdev, input, netdev, render, sudo, users]";

impl Login {
    /// # Errors
    /// If the user name or a key isn't usable.
    pub fn new(user: &str, keys: Vec<String>) -> Result<Self> {
        if !valid_name(user, 32) || ["root", "kdj"].contains(&user) {
            bail!("{user:?} can't be the login name: lower-case letters, digits, - and _, not root or kdj");
        }
        if keys.is_empty() {
            bail!("no SSH public key to log in with");
        }
        for k in &keys {
            check_key(k)?;
        }
        Ok(Self { user: user.to_owned(), keys })
    }
}

fn valid_name(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl Setup {
    /// # Errors
    /// If the hostname isn't usable.
    pub fn new(hostname: &str, login: Option<Login>) -> Result<Self> {
        if !valid_name(hostname, 63) || hostname.contains('_') {
            bail!("{hostname:?} isn't a usable hostname: lower-case letters, digits and -");
        }
        Ok(Self { hostname: hostname.to_owned(), login })
    }

    /// cloud-init user-data: the name, and the login if there is one.
    #[must_use]
    pub fn user_data(&self) -> String {
        let mut y = String::new();
        let _ = writeln!(y, "#cloud-config");
        match &self.login {
            Some(l) => {
                let _ = writeln!(y, "{MARK}: {}, an SSH login for {}, no password.", self.hostname, l.user);
            }
            None => {
                let _ = writeln!(y, "{MARK}: {}, no login.", self.hostname);
            }
        }
        let _ = writeln!(y, "hostname: {}", self.hostname);
        let _ = writeln!(y, "manage_etc_hosts: true");
        if let Some(l) = &self.login {
            let _ = writeln!(y, "users:");
            let _ = writeln!(y, "  - name: {}", l.user);
            let _ = writeln!(y, "    groups: {GROUPS}");
            let _ = writeln!(y, "    shell: /bin/bash");
            let _ = writeln!(y, "    sudo: ALL=(ALL) NOPASSWD:ALL");
            let _ = writeln!(y, "    lock_passwd: true");
            let _ = writeln!(y, "    ssh_authorized_keys:");
            for k in &l.keys {
                let _ = writeln!(y, "      - \"{}\"", k.replace('\\', "\\\\").replace('"', "\\\""));
            }
            let _ = writeln!(y, "ssh_pwauth: false");
        }
        // Never the "create a user" dialog, even on images from before it
        // was switched off: early in boot, before it could start.
        let _ = writeln!(y, "bootcmd:");
        let _ = writeln!(y, "  - [systemctl, mask, --now, userconfig.service]");
        y
    }

    /// Write it to the boot partition mounted at `boot`. A new instance id
    /// makes cloud-init apply it even on a card that has booted before.
    ///
    /// # Errors
    /// If the files can't be written.
    pub fn write(&self, boot: &Path) -> Result<()> {
        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let meta = format!("{MARK}.\ndsmode: local\ninstance_id: deck-{id}\n");
        std::fs::write(boot.join("user-data"), self.user_data()).context("writing user-data")?;
        std::fs::write(boot.join("meta-data"), meta).context("writing meta-data")?;
        // Raspberry Pi OS's sshswitch turns SSH on when this exists, then removes it.
        if self.login.is_some() {
            std::fs::write(boot.join("ssh"), b"").context("writing ssh")?;
        }
        // macOS leaves ._ files for extended attributes on FAT: tidy them.
        if cfg!(target_os = "macos") {
            let _ = std::process::Command::new("dot_clean").arg("-m").arg(boot).status();
        }
        Ok(())
    }
}

/// A public key line: type, base64, optional comment. Nothing that could
/// break out of the YAML around it.
fn check_key(k: &str) -> Result<()> {
    let mut parts = k.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    let known = kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-");
    if !known || body.len() < 32 || k.contains(['\n', '\r']) {
        bail!("that doesn't look like an SSH public key: {}", k.chars().take(40).collect::<String>());
    }
    Ok(())
}

/// Your SSH public keys: `file` if given, else the usual ones in ~/.ssh.
///
/// # Errors
/// If `file` can't be read.
pub fn find_keys(file: Option<&Path>) -> Result<Vec<String>> {
    let read = |p: &Path| -> Result<Vec<String>> {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        Ok(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect())
    };
    if let Some(f) = file {
        return read(f);
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Ok(Vec::new()) };
    for name in ["id_ed25519.pub", "id_ecdsa.pub", "id_ed25519_sk.pub", "id_rsa.pub"] {
        let p = home.join(".ssh").join(name);
        if p.is_file() {
            return read(&p);
        }
    }
    Ok(Vec::new())
}

/// The login name to suggest: who's running this, if it's a usable name.
#[must_use]
pub fn default_user() -> String {
    let user = std::env::var("SUDO_USER").or_else(|_| std::env::var("USER")).unwrap_or_default();
    user.to_ascii_lowercase()
}

/// What a card's boot partition says about logging in.
#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    /// Ours: the user it sets up.
    KdjFlash(String),
    /// Someone's user-data with users in it (Raspberry Pi Imager, by hand).
    Other,
    None,
}

#[must_use]
pub fn found(boot: &Path) -> Found {
    let Ok(text) = std::fs::read_to_string(boot.join("user-data")) else { return Found::None };
    let user = text.lines().find_map(|l| l.trim().strip_prefix("- name:")).map(str::trim);
    match (text.contains(MARK), user) {
        (true, Some(u)) => Found::KdjFlash(u.to_owned()),
        (false, _) if text.lines().any(|l| l.starts_with("users:")) => Found::Other,
        _ => Found::None,
    }
}

/// The name a card's user-data gives its Pi, if any.
#[must_use]
pub fn hostname(boot: &Path) -> Option<String> {
    let text = std::fs::read_to_string(boot.join("user-data")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("hostname:")).map(|h| h.trim().to_owned()).filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeKeyForTestsOnly0123456789abcdefg me@laptop";

    #[test]
    fn user_data_is_cloud_config_with_the_key_and_no_password() {
        let l = Login::new("kristian", vec![KEY.into()]).expect("valid");
        let y = Setup::new("kdj-amber-heron", Some(l)).expect("valid").user_data();
        assert!(y.contains("hostname: kdj-amber-heron\n"));
        assert!(y.starts_with("#cloud-config\n"));
        assert!(y.contains("  - name: kristian\n"));
        assert!(y.contains(&format!("      - \"{KEY}\"\n")));
        assert!(y.contains("lock_passwd: true") && y.contains("ssh_pwauth: false"));
        assert!(y.contains("  - [systemctl, mask, --now, userconfig.service]"), "never the keyboard dialog");
        assert!(!y.contains("passwd:") || !y.contains("    passwd:"), "no password hash");
    }

    #[test]
    fn bad_names_and_keys_are_refused() {
        assert!(Login::new("root", vec![KEY.into()]).is_err());
        assert!(Login::new("kdj", vec![KEY.into()]).is_err());
        assert!(Login::new("Kristian Freeman", vec![KEY.into()]).is_err());
        assert!(Login::new("kristian", vec!["not a key".into()]).is_err());
        assert!(Login::new("kristian", vec![format!("{KEY}\nusers: []")]).is_err(), "no YAML injection");
        assert!(Login::new("kristian", Vec::new()).is_err());
        assert!(Setup::new("kdj_x", None).is_err() && Setup::new("Kdj", None).is_err());
    }

    #[test]
    fn written_files_are_recognised() {
        let dir = std::env::temp_dir().join(format!("kdj-flash-login-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        assert_eq!(found(&dir), Found::None);
        Setup::new("kdj-amber-heron", None).expect("valid").write(&dir).expect("write");
        assert_eq!((found(&dir), hostname(&dir).as_deref()), (Found::None, Some("kdj-amber-heron")));
        assert!(!dir.join("ssh").exists(), "no login, no SSH");
        let l = Login::new("kristian", vec![KEY.into()]).expect("valid");
        Setup::new("kdj-amber-heron", Some(l)).expect("valid").write(&dir).expect("write");
        assert_eq!(found(&dir), Found::KdjFlash("kristian".into()));
        assert!(dir.join("ssh").is_file());
        let meta = std::fs::read_to_string(dir.join("meta-data")).expect("meta");
        assert!(meta.contains("dsmode: local") && meta.contains("instance_id: deck-"));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
