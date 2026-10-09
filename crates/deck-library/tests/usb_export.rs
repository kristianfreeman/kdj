//! A Rekordbox USB export resolves its tracks relative to the stick's root.

use std::path::Path;

use deck_library::{anlz, library};

#[test]
fn usb_export_tracks_resolve_against_the_stick_root() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_export");
    let stick = std::env::temp_dir().join(format!("deck-usb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stick);
    copy_dir(&fixture, &stick).unwrap();

    // Put a placeholder audio file wherever each analysis file says it lives.
    let usbanlz = stick.join("PIONEER/USBANLZ");
    let mut expected = Vec::new();
    for dat in anlz::scan(&usbanlz) {
        let stored = anlz::read(&dat).unwrap().audio.expect("analysis files store the audio path");
        assert!(stored.starts_with('/'), "USB paths are stick-relative: {stored}");
        let audio = stick.join(stored.trim_start_matches('/'));
        std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
        std::fs::write(&audio, b"").unwrap();
        expected.push(audio);
    }
    assert_eq!(expected.len(), 2);

    let pdb = Some(stick.join("PIONEER/rekordbox/export.pdb"));
    let source = library::Source { name: "DEMO".into(), usbanlz, root: Some(stick.clone()), pdb };
    let lib = library::scan(&source);
    assert_eq!(lib.missing, 0);
    assert_eq!(lib.entries.len(), 2);
    for e in &lib.entries {
        assert!(expected.contains(&e.audio), "{}", e.audio.display());
        assert!(e.bpm.is_some_and(|b| b > 60.0), "{}: bpm {:?}", e.title, e.bpm);
        assert_eq!(e.artist, "Loopmasters");
        assert!(e.title.starts_with("Demo Track"), "title from export.pdb: {}", e.title);
        assert!(e.anlz.is_file(), "analysis path resolves: {}", e.anlz.display());
        println!("{} — {} ({:.2} bpm)", e.artist, e.title, e.bpm.unwrap());
    }
    std::fs::remove_dir_all(&stick).unwrap();
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            std::fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}
