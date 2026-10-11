//! The Pi's network, for the settings screen: Ethernet and Wi-Fi as
//! `NetworkManager` sees them (`nmcli`), and switching Wi-Fi on and off.
//!
//! Looked at in the background, only while the settings are open; on a
//! machine without `NetworkManager` (a Mac) it says so instead.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const LOOK_EVERY: Duration = Duration::from_secs(3);

/// What the settings show about the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// Wi-Fi's radio is on; None without `NetworkManager`.
    pub wifi_on: Option<bool>,
    /// "Home · 72% · 192.168.1.31", "Not connected", "Off".
    pub wifi: String,
    /// "192.168.1.20", "Not plugged in".
    pub ethernet: String,
}

/// The background look at the network.
pub struct Watch {
    /// Look while this is set (the settings are open).
    pub wanted: Arc<AtomicBool>,
    pub status: mpsc::Receiver<Status>,
}

/// Start looking, every few seconds while `wanted`.
pub fn watch() -> Watch {
    let wanted = Arc::new(AtomicBool::new(false));
    let (tx, status) = mpsc::channel();
    let w = Arc::clone(&wanted);
    let _ = std::thread::Builder::new().name("network".into()).spawn(move || {
        loop {
            if w.load(Ordering::Relaxed) && tx.send(look()).is_err() {
                return;
            }
            std::thread::sleep(LOOK_EVERY);
        }
    });
    Watch { wanted, status }
}

/// Switch Wi-Fi on or off (allowed by /etc/sudoers.d/kdj, which the Pi
/// setup installs). What happened, for the status line.
pub fn set_wifi(on: bool) -> String {
    let state = if on { "on" } else { "off" };
    if !cfg!(target_os = "linux") {
        return format!("Can't switch Wi-Fi {state} from here: that's for the Pi");
    }
    match Command::new("sudo").args(["-n", "/usr/bin/nmcli", "radio", "wifi", state]).status() {
        Ok(s) if s.success() => format!("Wi-Fi {state}"),
        Ok(s) => format!("Couldn't switch Wi-Fi {state} ({s})"),
        Err(e) => format!("Couldn't switch Wi-Fi {state}: {e}"),
    }
}

fn look() -> Status {
    let nmcli = |args: &[&str]| -> Option<String> {
        let out = Command::new("nmcli").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let Some(devices) = nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"]) else {
        return Status { wifi_on: None, wifi: "– (no NetworkManager)".into(), ethernet: "–".into() };
    };
    let radio = nmcli(&["radio", "wifi"]).map(|r| r.trim() == "enabled");
    let address = |dev: &str| nmcli(&["-g", "IP4.ADDRESS", "device", "show", dev]).and_then(|a| first_address(&a));
    let signal = || nmcli(&["-t", "-f", "ACTIVE,SIGNAL", "device", "wifi", "list", "--rescan", "no"]);
    describe(&devices, radio, address, signal)
}

/// The status from `nmcli -t device status` lines, the radio, and ways to
/// get a device's address and the Wi-Fi signal list.
fn describe(
    devices: &str,
    radio: Option<bool>,
    address: impl Fn(&str) -> Option<String>,
    signal: impl Fn() -> Option<String>,
) -> Status {
    let devices: Vec<Vec<String>> = devices.lines().map(fields).collect();
    let find = |kind: &str| devices.iter().find(|d| d.get(1).is_some_and(|t| t == kind));
    let ethernet = match find("ethernet") {
        Some(d) if d.get(2).is_some_and(|s| s == "connected") => {
            address(&d[0]).unwrap_or_else(|| "Connected, no address yet".into())
        }
        Some(d) if d.get(2).is_some_and(|s| s == "unavailable") => "Not plugged in".into(),
        Some(d) => capitalised(d.get(2).map_or("", String::as_str)),
        None => "No Ethernet".into(),
    };
    let wifi = match (find("wifi"), radio) {
        (None, _) => "No Wi-Fi".into(),
        (_, Some(false)) => "Off".into(),
        (Some(d), _) if d.get(2).is_some_and(|s| s == "connected") => {
            let mut parts = vec![d.get(3).cloned().unwrap_or_default()];
            let strength = signal().and_then(|list| {
                list.lines().map(fields).find(|f| f.first().is_some_and(|a| a == "yes")).and_then(|f| f.get(1).cloned())
            });
            parts.extend(strength.map(|s| format!("{s}%")));
            parts.extend(address(&d[0]));
            parts.join(" · ")
        }
        (Some(d), _) if d.get(2).is_some_and(|s| s.starts_with("connecting")) => "Connecting…".into(),
        (Some(_), _) => "Not connected".into(),
    };
    Status { wifi_on: radio, wifi, ethernet }
}

/// An `nmcli -t` line's fields: split at `:`, with `\:` a colon in a field
/// (a network name can have one).
fn fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.last_mut().into_iter().for_each(|f| f.extend(chars.next())),
            ':' => out.push(String::new()),
            c => out.last_mut().into_iter().for_each(|f| f.push(c)),
        }
    }
    out
}

/// The first address in `nmcli -g IP4.ADDRESS` output, without its prefix length.
fn first_address(text: &str) -> Option<String> {
    let first = text.split(['|', '\n']).map(str::trim).find(|a| !a.is_empty())?;
    Some(first.split('/').next().unwrap_or(first).to_owned())
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str =
        "wlan0:wifi:connected:Home\\:Net\neth0:ethernet:unavailable:\nlo:loopback:connected (externally):lo\n";

    #[test]
    fn connected_wifi_shows_its_network_signal_and_address() {
        let address = |dev: &str| (dev == "wlan0").then(|| "192.168.1.31".to_owned());
        let signal = || Some("no:40\nyes:72\n".to_owned());
        let s = describe(DEVICES, Some(true), address, signal);
        assert_eq!(s.wifi, "Home:Net · 72% · 192.168.1.31");
        assert_eq!(s.ethernet, "Not plugged in");
        assert_eq!(s.wifi_on, Some(true));
    }

    #[test]
    fn wifi_off_and_ethernet_plugged_in() {
        let devices = "eth0:ethernet:connected:Wired connection 1\nwlan0:wifi:unavailable:\n";
        let s = describe(devices, Some(false), |_| Some("10.0.0.5".into()), || None);
        assert_eq!((s.wifi.as_str(), s.ethernet.as_str()), ("Off", "10.0.0.5"));
        let s = describe("wlan0:wifi:disconnected:\n", Some(true), |_| None, || None);
        assert_eq!((s.wifi.as_str(), s.ethernet.as_str()), ("Not connected", "No Ethernet"));
    }

    #[test]
    fn nmcli_output_is_read_carefully() {
        assert_eq!(fields("a\\:b:c\\\\d:"), ["a:b", "c\\d", ""]);
        assert_eq!(first_address("192.168.1.31/24 | 10.0.0.2/8\n").as_deref(), Some("192.168.1.31"));
        assert_eq!(first_address("\n"), None);
    }
}
