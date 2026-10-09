//! Print what the browser would show for each source, or for a given export root.
use std::time::Duration;

use deck_library::library::{self, Source};

fn main() {
    let sources = std::env::args().nth(1).map_or_else(library::sources, |root| {
        let root = std::path::PathBuf::from(root);
        let pdb = Some(root.join("PIONEER/rekordbox/export.pdb")).filter(|p| p.is_file());
        vec![Source { name: "given".into(), usbanlz: root.join("PIONEER/USBANLZ"), root: Some(root), pdb }]
    });
    for s in sources {
        let t = std::time::Instant::now();
        let lib = library::scan(&s);
        println!(
            "{}: {} tracks, {} missing audio ({:.0} ms)",
            s.name,
            lib.entries.len(),
            lib.missing,
            t.elapsed().as_secs_f64() * 1000.0
        );
        for e in lib.entries.iter().take(6) {
            println!(
                "  {:<28} {:<30} {:>7} {:>6}",
                e.artist,
                e.title,
                e.bpm.map_or_else(String::new, |b| format!("{b:.2}")),
                e.secs.and_then(|s| Duration::try_from_secs_f64(s).ok()).map_or_else(String::new, |d| {
                    let secs = d.as_secs();
                    format!("{}:{:02}", secs / 60, secs % 60)
                })
            );
        }
    }
}
