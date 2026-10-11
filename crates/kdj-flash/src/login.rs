//! What a card's Pi is called and how you log in to it, without ever
//! plugging in a keyboard: its network name, and optionally your user and
//! SSH public key with SSH on. Written to the card's boot partition for
//! Raspberry Pi OS to apply on first boot (cloud-init's user-data, and the
//! `ssh` file its sshswitch service looks for). No password anywhere: the
//! account's password is locked, the key is the way in.
//!
//! Optionally a Wi-Fi network too, in cloud-init's `network-config` (as
//! Raspberry Pi Imager does it). Its password is on the boot partition, as
//! with Imager: anyone holding the card can read it.

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

/// Everything written to a card: its name, a login and a Wi-Fi network if
/// there are any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The Pi's network name: reach it as NAME.local.
    pub hostname: String,
    pub login: Option<Login>,
    pub wifi: Option<Wifi>,
}

/// A Wi-Fi network for the Pi to join.
#[derive(Clone, PartialEq, Eq)]
pub struct Wifi {
    pub ssid: String,
    /// Empty for an open network.
    pub password: String,
    /// Where it is, two letters ("US"): which channels it may use. Raspberry
    /// Pi OS keeps Wi-Fi off until it's set.
    pub country: String,
}

impl std::fmt::Debug for Wifi {
    // Not derived: never the password, not even in a debug print.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wifi").field("ssid", &self.ssid).field("country", &self.country).finish_non_exhaustive()
    }
}

impl Wifi {
    /// # Errors
    /// If the network name, password or country isn't usable.
    pub fn new(ssid: &str, password: &str, country: &str) -> Result<Self> {
        if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
            bail!("{ssid:?} isn't a Wi-Fi network name (1 to 32 bytes, no control characters)");
        }
        let hex = password.len() == 64 && password.chars().all(|c| c.is_ascii_hexdigit());
        let passphrase =
            (8..=63).contains(&password.len()) && password.chars().all(|c| c.is_ascii() && !c.is_control());
        if !password.is_empty() && !hex && !passphrase {
            bail!("a Wi-Fi password is 8 to 63 characters (or 64 hex digits)");
        }
        let country = country.to_ascii_uppercase();
        if country.len() != 2 || !country.chars().all(|c| c.is_ascii_uppercase()) {
            bail!("{country:?} isn't a country code: two letters, like US or GB");
        }
        Ok(Self { ssid: ssid.to_owned(), password: password.to_owned(), country })
    }
}

/// `s` as a YAML double-quoted string (no control characters get this far).
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
        Ok(Self { hostname: hostname.to_owned(), login, wifi: None })
    }

    /// cloud-init network-config: Ethernet as always, and the Wi-Fi network.
    #[must_use]
    pub fn network_config(&self) -> Option<String> {
        let w = self.wifi.as_ref()?;
        let mut y = String::new();
        let _ = writeln!(y, "{MARK}: Ethernet, and the Wi-Fi network {}.", quoted(&w.ssid));
        let _ = writeln!(y, "network:");
        let _ = writeln!(y, "  version: 2");
        let _ = writeln!(y, "  ethernets:");
        let _ = writeln!(y, "    eth0:");
        let _ = writeln!(y, "      dhcp4: true");
        let _ = writeln!(y, "      optional: true");
        let _ = writeln!(y, "  wifis:");
        let _ = writeln!(y, "    wlan0:");
        let _ = writeln!(y, "      dhcp4: true");
        let _ = writeln!(y, "      optional: true");
        let _ = writeln!(y, "      regulatory-domain: {}", quoted(&w.country));
        let _ = writeln!(y, "      access-points:");
        if w.password.is_empty() {
            let _ = writeln!(y, "        {}: {{}}", quoted(&w.ssid));
        } else {
            let _ = writeln!(y, "        {}:", quoted(&w.ssid));
            let _ = writeln!(y, "          password: {}", quoted(&w.password));
        }
        Some(y)
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
        // Raspberry Pi OS keeps Wi-Fi blocked until the country is set. Once
        // (not every boot), so Wi-Fi switched off in the deck's settings stays off.
        if let Some(w) = &self.wifi {
            let _ = writeln!(y, "runcmd:");
            let _ = writeln!(y, "  - [rfkill, unblock, wifi]");
            let _ = writeln!(y, "  - [raspi-config, nonint, do_wifi_country, {}]", w.country);
        }
        y
    }

    /// Write it to the boot partition mounted at `boot`. A new instance id
    /// makes cloud-init apply it even on a card that has booted before.
    ///
    /// # Errors
    /// If the files can't be written.
    pub fn write(&self, boot: &Path) -> Result<()> {
        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // `instance-id`: cloud-init ignores Raspberry Pi's `instance_id`, and
        // only a changed id makes it apply user-data on a card that's booted.
        let meta = format!("{MARK}.\ndsmode: local\ninstance-id: kdj-{id}\n");
        std::fs::write(boot.join("user-data"), self.user_data()).context("writing user-data")?;
        std::fs::write(boot.join("meta-data"), meta).context("writing meta-data")?;
        // Without Wi-Fi, the card's network-config is left as it is: one
        // from an earlier `--wifi` keeps working.
        if let Some(n) = self.network_config() {
            std::fs::write(boot.join("network-config"), n).context("writing network-config")?;
        }
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

/// This computer's region as a Wi-Fi country ("US"): the Mac's region, or
/// the locale's (`LANG=en_GB.UTF-8`).
#[must_use]
pub fn local_country() -> Option<String> {
    let mac = || {
        let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output().ok()?;
        String::from_utf8(out.stdout).ok()
    };
    let locale = if cfg!(target_os = "macos") { mac() } else { None }
        .or_else(|| std::env::var("LC_ALL").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LANG").ok())?;
    country_of(&locale)
}

/// The country in a locale name: `en_US.UTF-8` or `en_GB@rg=gbzzzz` is US or GB.
fn country_of(locale: &str) -> Option<String> {
    let region = locale.trim().split(['.', '@']).next()?.split(['_', '-']).nth(1)?;
    (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic())).then(|| region.to_ascii_uppercase())
}

/// The Wi-Fi network a card's network-config (one we wrote) joins.
#[must_use]
pub fn wifi_name(boot: &Path) -> Option<String> {
    let text = std::fs::read_to_string(boot.join("network-config")).ok()?;
    if !text.starts_with(MARK) {
        return None;
    }
    let mut lines = text.lines().skip_while(|l| l.trim() != "access-points:");
    let entry = lines.nth(1)?.trim();
    let name = entry.strip_suffix(": {}").or_else(|| entry.strip_suffix(':'))?;
    let inner = name.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// A card's Pi's name: the one its user-data gives it, else the one it
/// gave itself on a boot (kdj-name leaves it in kdj-name.txt).
#[must_use]
pub fn hostname(boot: &Path) -> Option<String> {
    let given = std::fs::read_to_string(boot.join("user-data")).ok().and_then(|t| {
        t.lines().find_map(|l| l.strip_prefix("hostname:")).map(|h| h.trim().to_owned()).filter(|h| !h.is_empty())
    });
    given.or_else(|| {
        let own = std::fs::read_to_string(boot.join("kdj-name.txt")).ok()?;
        Some(own.trim().to_owned()).filter(|h| !h.is_empty())
    })
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
    fn wifi_goes_in_network_config_with_ethernet_and_the_country() {
        let mut setup = Setup::new("kdj-oak-pike", None).expect("valid");
        assert_eq!(setup.network_config(), None, "no Wi-Fi, no network-config");
        setup.wifi = Some(Wifi::new("Home \"5G\"", "correct horse", "us").expect("valid"));
        let n = setup.network_config().expect("Wi-Fi");
        assert!(n.contains("    eth0:\n      dhcp4: true\n"), "Ethernet still works:\n{n}");
        assert!(n.contains("      regulatory-domain: \"US\"\n"));
        assert!(n.contains("        \"Home \\\"5G\\\"\":\n          password: \"correct horse\"\n"), "{n}");
        let y = setup.user_data();
        assert!(y.contains("  - [raspi-config, nonint, do_wifi_country, US]\n"), "{y}");
        assert!(!format!("{:?}", setup.wifi).contains("horse"), "the password stays out of debug prints");

        let dir = std::env::temp_dir().join(format!("kdj-flash-wifi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        setup.write(&dir).expect("written");
        assert_eq!(wifi_name(&dir).as_deref(), Some("Home \"5G\""));
        setup.wifi = Some(Wifi::new("Cafe", "", "GB").expect("an open network"));
        assert!(setup.network_config().expect("Wi-Fi").contains("        \"Cafe\": {}\n"));
        setup.write(&dir).expect("written");
        assert_eq!(wifi_name(&dir).as_deref(), Some("Cafe"));
        std::fs::remove_dir_all(&dir).expect("cleaned up");
    }

    #[test]
    fn the_country_comes_from_the_locale() {
        assert_eq!(country_of("en_US").as_deref(), Some("US"));
        assert_eq!(country_of("en_GB.UTF-8").as_deref(), Some("GB"));
        assert_eq!(country_of("de_DE@rg=dezzzz").as_deref(), Some("DE"));
        assert_eq!(country_of("en-AU").as_deref(), Some("AU"));
        assert_eq!(country_of("C.UTF-8"), None);
        assert_eq!(country_of("POSIX"), None);
    }

    #[test]
    fn unusable_wifi_is_refused() {
        assert!(Wifi::new("", "correct horse", "US").is_err(), "no name");
        assert!(Wifi::new(&"x".repeat(33), "correct horse", "US").is_err(), "too long a name");
        assert!(Wifi::new("Home\nusers: []", "correct horse", "US").is_err(), "no YAML injection");
        assert!(Wifi::new("Home", "short", "US").is_err(), "too short a password");
        assert!(Wifi::new("Home", "correct horse", "USA").is_err());
        assert!(Wifi::new("Home", &"a".repeat(64), "US").is_ok(), "a 64-hex-digit key");
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
        assert_eq!((found(&dir), hostname(&dir)), (Found::None, None));
        std::fs::write(dir.join("kdj-name.txt"), "kdj-oak-pike\n").expect("write");
        assert_eq!(hostname(&dir).as_deref(), Some("kdj-oak-pike"), "the name the Pi gave itself");
        Setup::new("kdj-amber-heron", None).expect("valid").write(&dir).expect("write");
        assert_eq!((found(&dir), hostname(&dir).as_deref()), (Found::None, Some("kdj-amber-heron")));
        assert!(!dir.join("ssh").exists(), "no login, no SSH");
        let l = Login::new("kristian", vec![KEY.into()]).expect("valid");
        Setup::new("kdj-amber-heron", Some(l)).expect("valid").write(&dir).expect("write");
        assert_eq!(found(&dir), Found::KdjFlash("kristian".into()));
        assert!(dir.join("ssh").is_file());
        let meta = std::fs::read_to_string(dir.join("meta-data")).expect("meta");
        assert!(meta.contains("dsmode: local") && meta.contains("instance-id: kdj-"));
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
