//! Our analysis-file reader against rekordcrate's, on the demo export.

use std::path::Path;

use anyhow::Result;
use binrw::BinRead;
use deck_library::anlz;
use rekordcrate::anlz::{ANLZ, Content, CueListType};

#[test]
fn analysis_files_match_rekordcrate() -> Result<()> {
    let usbanlz = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_export/PIONEER/USBANLZ");
    let dats = anlz::scan(&usbanlz);
    assert_eq!(dats.len(), 2);
    for dat in dats {
        let ours = anlz::read(&dat)?;
        let theirs = ANLZ::read(&mut std::io::BufReader::new(std::fs::File::open(&dat)?))?;
        let (mut beats, mut memory, mut hot) = (0, Vec::new(), Vec::new());
        for section in theirs.sections {
            match section.content {
                Content::Path(p) => assert_eq!(ours.audio.as_deref(), Some(p.path.to_string().as_str())),
                Content::BeatGrid(g) => {
                    beats = g.beats.len();
                    for (o, t) in ours.beats.iter().zip(&g.beats) {
                        assert_eq!((o.number, o.ms), (t.beat_number, t.time));
                        assert!((o.bpm - f64::from(t.tempo) / 100.0).abs() < 1e-9);
                    }
                }
                Content::CueList(l) if l.list_type == CueListType::MemoryCues => {
                    memory.extend(l.cues.iter().map(|c| c.time));
                }
                Content::CueList(l) => hot.extend(l.cues.iter().map(|c| (c.hot_cue, c.time))),
                _ => {}
            }
        }
        assert!(beats > 100, "{}: a real grid", dat.display());
        assert_eq!(ours.beats.len(), beats);
        assert_eq!(ours.memory_cues, memory);
        assert_eq!(ours.hot_cues.iter().map(|&(s, t, _)| (s, t)).collect::<Vec<_>>(), hot);
        println!("{}: {beats} beats, {} memory cues, {} hot cues", dat.display(), memory.len(), hot.len());
    }
    Ok(())
}
