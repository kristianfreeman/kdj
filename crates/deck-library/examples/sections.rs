//! List the sections of the analysis files next to a given ANLZ0000.DAT, and
//! value ranges for the 3-band waveform.
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let dat = std::path::PathBuf::from(std::env::args().nth(1).context("usage: sections ANLZ0000.DAT")?);
    for ext in ["DAT", "EXT", "2EX"] {
        let Ok(b) = std::fs::read(dat.with_extension(ext)) else { continue };
        println!("{ext}: {}", tags(&b).join(" "));
    }
    Ok(())
}

/// Each section as `TAG(length)`, printing waveform stats on the way. Stops
/// at the first header that doesn't fit the file.
fn tags(b: &[u8]) -> Vec<String> {
    let mut tags = Vec::new();
    let Some(mut o) = be_usize(b, 4) else { return tags }; // after the PMAI header
    while o.checked_add(12).is_some_and(|end| end <= b.len()) {
        let (Some(hl), Some(tl)) = (be_usize(b, o + 4), be_usize(b, o + 8)) else { break };
        let tag = String::from_utf8_lossy(&b[o..o + 4]).into_owned();
        if tag == "PWV7" || tag == "PWV6" {
            let n = be(b, o + 16).unwrap_or(0);
            if let Some(body) = b.get(o + hl..o + tl) {
                print_waveform_stats(&tag, n, body);
            }
        }
        tags.push(format!("{tag}({tl})"));
        match o.checked_add(tl) {
            Some(next) if tl > 0 => o = next,
            _ => break,
        }
    }
    tags
}

fn print_waveform_stats(tag: &str, n: u32, body: &[u8]) {
    let columns = body.as_chunks::<3>().0;
    let max = |i: usize| columns.iter().map(|c| c[i]).max().unwrap_or(0);
    let mean = |i: usize| columns.iter().map(|c| f64::from(c[i])).sum::<f64>() / f64::from(n.max(1));
    println!(
        "  {tag}: {n} entries, max mid/high/low {}/{}/{}, mean {:.0}/{:.0}/{:.0}",
        max(0),
        max(1),
        max(2),
        mean(0),
        mean(1),
        mean(2)
    );
}

fn be(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn be_usize(b: &[u8], o: usize) -> Option<usize> {
    usize::try_from(be(b, o)?).ok()
}
