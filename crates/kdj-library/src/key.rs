//! Musical keys on the Camelot wheel, for harmonic mixing.
//!
//! The wheel puts the 24 keys on 12 hours, minor keys on the inner ring (A)
//! and major on the outer (B). Two tracks mix without clashing when their
//! keys are the same, one hour apart on the same ring, or on the same hour
//! across the rings (relative major and minor).

use std::fmt;

/// A key's place on the Camelot wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Camelot {
    /// The hour, 1–12.
    hour: u8,
    minor: bool,
}

/// How well a track's key goes with the one playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The same key.
    Same,
    /// A neighbour on the wheel: one hour either way, or the relative
    /// major or minor.
    Near,
}

/// Pitch class (C = 0) of the note names.
const NOTES: [(char, u8); 7] = [('C', 0), ('D', 2), ('E', 4), ('F', 5), ('G', 7), ('A', 9), ('B', 11)];

/// Key names by hour (index 0 is hour 1), as Rekordbox spells them.
const MINOR: [&str; 12] = ["Abm", "Ebm", "Bbm", "Fm", "Cm", "Gm", "Dm", "Am", "Em", "Bm", "F#m", "Dbm"];
const MAJOR: [&str; 12] = ["B", "F#", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E"];

impl Camelot {
    /// `hour` 1–12; None outside it.
    #[must_use]
    pub const fn new(hour: u8, minor: bool) -> Option<Self> {
        if hour >= 1 && hour <= 12 { Some(Self { hour, minor }) } else { None }
    }

    /// A key as Rekordbox or a tag names it: classic ("Fm", "F#", "Bbm",
    /// "A minor"), Camelot ("4A", "08B") or Open Key ("9m", "1d"). None for
    /// anything else, an empty name included.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let s = name.trim();
        if s.starts_with(|c: char| c.is_ascii_digit()) {
            return Self::parse_numbered(s);
        }
        let mut chars = s.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let &(_, natural) = NOTES.iter().find(|(n, _)| *n == letter)?;
        let rest = chars.as_str();
        let (pc, rest) = match rest.chars().next() {
            Some('#' | '♯') => (natural + 1, &rest[rest.chars().next().map_or(0, char::len_utf8)..]),
            Some('b' | '♭') => (natural + 11, &rest[rest.chars().next().map_or(0, char::len_utf8)..]),
            _ => (natural, rest),
        };
        let minor = match rest.trim().to_ascii_lowercase().as_str() {
            "" | "maj" | "major" => false,
            "m" | "min" | "minor" => true,
            _ => return None,
        };
        Some(Self::from_pitch(pc % 12, minor))
    }

    /// Camelot ("4A") or Open Key ("9m").
    fn parse_numbered(s: &str) -> Option<Self> {
        let split = s.find(|c: char| !c.is_ascii_digit())?;
        let n: u8 = s[..split].parse().ok()?;
        let hour = |minor| Self::new(n, minor);
        match s[split..].trim() {
            "A" | "a" => hour(true),
            "B" | "b" => hour(false),
            // Open Key's 1 is Camelot's 8.
            m @ ("m" | "d") => Self::new(n, true).map(|_| Self { hour: (n + 6) % 12 + 1, minor: m == "m" }),
            _ => None,
        }
    }

    /// A key from its tonic's pitch class (C = 0) and mode.
    const fn from_pitch(pc: u8, minor: bool) -> Self {
        // Each hour up the wheel is a fifth up; C major is 8B, A minor 8A.
        let major_pc = if minor { (pc + 3) % 12 } else { pc };
        Self { hour: (7 * major_pc + 7) % 12 + 1, minor }
    }

    /// The hour, 1–12.
    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }

    #[must_use]
    pub const fn minor(self) -> bool {
        self.minor
    }

    /// How a track in this key goes with one in `playing`: None if it clashes.
    #[must_use]
    pub const fn fit(self, playing: Self) -> Option<Fit> {
        let apart = (self.hour + 12 - playing.hour) % 12;
        if self.minor == playing.minor {
            match apart {
                0 => Some(Fit::Same),
                1 | 11 => Some(Fit::Near),
                _ => None,
            }
        } else if apart == 0 {
            Some(Fit::Near)
        } else {
            None
        }
    }

    /// One byte, for the network: 1–24 (0 is left for "no key").
    #[must_use]
    pub const fn code(self) -> u8 {
        self.hour + if self.minor { 0 } else { 12 }
    }

    /// The key a `code` stands for; None for 0 or anything out of range.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1..=12 => Some(Self { hour: code, minor: true }),
            13..=24 => Some(Self { hour: code - 12, minor: false }),
            _ => None,
        }
    }

    /// The key's name as Rekordbox spells it ("Fm").
    #[must_use]
    pub const fn name(self) -> &'static str {
        let i = self.hour as usize - 1;
        if self.minor { MINOR[i] } else { MAJOR[i] }
    }
}

impl fmt::Display for Camelot {
    /// The Camelot name ("4A").
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.hour, if self.minor { 'A' } else { 'B' })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Camelot {
        Camelot::parse(s).unwrap_or_else(|| panic!("{s} parses"))
    }

    #[test]
    fn every_rekordbox_name_lands_on_its_hour() {
        for (i, (minor, major)) in MINOR.iter().zip(MAJOR).enumerate() {
            let hour = u8::try_from(i + 1).unwrap();
            assert_eq!(k(minor).to_string(), format!("{hour}A"), "{minor}");
            assert_eq!(k(major).to_string(), format!("{hour}B"), "{major}");
            assert_eq!(k(minor).name(), *minor);
            assert_eq!(k(major).name(), major);
        }
    }

    #[test]
    fn sharps_flats_and_other_spellings_agree() {
        assert_eq!(k("G#m"), k("Abm"));
        assert_eq!(k("C#"), k("Db"));
        assert_eq!(k("A♭m"), k("Abm"));
        assert_eq!(k("F♯"), k("Gb"));
        assert_eq!(k("a minor"), k("Am"));
        assert_eq!(k("E maj"), k("E"));
        assert_eq!(k("8A"), k("Am"));
        assert_eq!(k("08b"), k("C"));
        assert_eq!(k("1d"), k("C"), "Open Key");
        assert_eq!(k("1m"), k("Am"));
        assert_eq!(k("2m"), k("Em"));
        assert_eq!(k("12m"), k("Dm"));
        assert_eq!(k("Cb"), k("B"));
        assert_eq!(k("B#m"), k("Cm"));
    }

    #[test]
    fn anything_else_is_no_key() {
        for s in ["", " ", "H", "Fx", "13A", "0B", "4C", "Am7", "#", "Open", "4", "m"] {
            assert_eq!(Camelot::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn neighbours_on_the_wheel_fit_and_the_rest_clash() {
        let playing = k("Fm"); // 4A
        assert_eq!(k("Fm").fit(playing), Some(Fit::Same));
        assert_eq!(k("Bbm").fit(playing), Some(Fit::Near), "3A");
        assert_eq!(k("Cm").fit(playing), Some(Fit::Near), "5A");
        assert_eq!(k("Ab").fit(playing), Some(Fit::Near), "4B, the relative major");
        assert_eq!(k("Gm").fit(playing), None, "6A");
        assert_eq!(k("Eb").fit(playing), None, "5B");
        assert_eq!(k("F").fit(playing), None, "7B: the parallel major");
        // Round the top of the wheel.
        assert_eq!(k("1A").fit(k("12A")), Some(Fit::Near));
        assert_eq!(k("12B").fit(k("1B")), Some(Fit::Near));
    }

    #[test]
    fn codes_round_trip() {
        for code in 1..=24 {
            assert_eq!(Camelot::from_code(code).map(Camelot::code), Some(code));
        }
        assert_eq!(Camelot::from_code(0), None);
        assert_eq!(Camelot::from_code(25), None);
    }
}
