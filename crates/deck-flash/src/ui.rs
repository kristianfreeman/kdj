//! How things look: colour, symbols, sizes, progress bars.

use std::time::Duration;

use console::{Style, style};
use indicatif::{ProgressBar, ProgressStyle};

pub fn heading(s: &str) {
    println!("\n{}", style(s).bold());
}

pub fn ok(s: &str) {
    println!("  {} {s}", style("✓").green().bold());
}

pub fn warn(s: &str) {
    println!("  {} {s}", style("!").yellow().bold());
}

pub fn info(s: &str) {
    println!("  {} {s}", style("·").dim());
}

pub fn step(s: &str) {
    println!("  {} {s}", style("→").cyan().bold());
}

/// A key and value, lined up.
pub fn field(key: &str, value: &str) {
    println!("    {:<14} {value}", style(key).dim());
}

pub fn hint(s: &str) {
    println!("    {}", style(s).dim());
}

pub const fn danger() -> Style {
    Style::new().red().bold()
}

/// Bytes as people say them: 31.9 GB, 604 MB (decimal, as cards are sold).
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "a size shown to one decimal")]
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        n if n >= 1_000_000_000_000 => format!("{:.1} TB", b / 1e12),
        n if n >= 1_000_000_000 => format!("{:.1} GB", b / 1e9),
        n if n >= 1_000_000 => format!("{:.0} MB", b / 1e6),
        n if n >= 1_000 => format!("{:.0} kB", b / 1e3),
        n => format!("{n} B"),
    }
}

/// indicatif's template (its placeholders, not Rust's).
const BAR: &str = "  {spinner:.cyan} {msg:<10} [{bar:32.cyan/blue}] {bytes:>9} / {total_bytes:<9} {binary_bytes_per_sec:>11}  {eta:>4} left";

/// A progress bar over `total` bytes, labelled `what`.
#[must_use]
pub fn bytes_bar(total: u64, what: &str) -> ProgressBar {
    let bar = ProgressBar::new(total);
    let style =
        ProgressStyle::with_template(BAR).map_or_else(|_| ProgressStyle::default_bar(), |s| s.progress_chars("━╸─"));
    bar.set_style(style);
    bar.set_message(what.to_owned());
    bar.enable_steady_tick(Duration::from_millis(120));
    bar
}
