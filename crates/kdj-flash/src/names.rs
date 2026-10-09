//! Each kdj's network name: kdj-<adjective>-<noun>, from the word list the
//! Pi image uses too (deploy/pi/names.txt), so several on one network can
//! be told apart and reached as NAME.local.

use std::io::Read;

const WORDS: &str = include_str!("../../../deploy/pi/names.txt");

fn words(key: &str) -> Vec<&'static str> {
    WORDS.lines().find_map(|l| l.strip_prefix(key)).map(|w| w.split_whitespace().collect()).unwrap_or_default()
}

/// A fresh name, picked at random.
#[must_use]
pub fn random() -> String {
    let mut b = [0u8; 2];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !random {
        // No /dev/urandom: the clock's nanoseconds will do for a name.
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        b = [(n & 0xff) as u8, ((n >> 8) & 0xff) as u8];
    }
    let (adjectives, nouns) = (words("adjectives:"), words("nouns:"));
    let pick = |list: &[&'static str], i: u8| list.get(usize::from(i) % list.len().max(1)).copied().unwrap_or("deck");
    format!("kdj-{}-{}", pick(&adjectives, b[0]), pick(&nouns, b[1]))
}

/// Whether `name` is one of ours (so re-flashing a card keeps it).
#[must_use]
pub fn is_ours(name: &str) -> bool {
    let mut parts = name.splitn(3, '-');
    let (Some("kdj"), Some(a), Some(n)) = (parts.next(), parts.next(), parts.next()) else { return false };
    words("adjectives:").contains(&a) && words("nouns:").contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_lists_are_full_and_distinct() {
        let (a, n) = (words("adjectives:"), words("nouns:"));
        assert_eq!((a.len(), n.len()), (64, 64));
        assert!(a.iter().all(|w| !n.contains(w)), "no word in both lists");
    }

    #[test]
    fn random_names_are_ours_and_vary() {
        let names: Vec<String> = (0..20).map(|_| random()).collect();
        assert!(names.iter().all(|n| is_ours(n) && n.len() <= 63), "{names:?}");
        assert!(names.iter().any(|n| n != &names[0]), "20 draws, all the same");
        assert!(!is_ours("kdj") && !is_ours("deck") && !is_ours("kdj-blue-sky") && !is_ours("raspberrypi"));
    }
}
