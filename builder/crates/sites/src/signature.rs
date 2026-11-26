//! A signature: a few instruction words in a row, some bits masked, that finds a place in the game's code.
//!
//! Written as 8 hex digits a word, separated by spaces, as the CPU reads them (little-endian words); a word may mask
//! nibbles with `?` (`f940??68`) or bits with `/mask` (`34000009/ff00001f`: the bits set in the mask must match).

use std::fmt;

/// At most this many words: a signature names as few as make it unique (the repository may hold no more of the game).
pub const MAX_WORDS: usize = 16;

/// One word of a signature: the bits of `value` that `mask` keeps must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaskedWord {
    /// The bits to match (none outside the mask).
    pub value: u32,
    /// The bits that count.
    pub mask: u32,
}

impl MaskedWord {
    /// Whether `word` matches.
    pub fn matches(self, word: u32) -> bool {
        word & self.mask == self.value
    }

    /// Whether every bit counts.
    pub fn is_whole(self) -> bool {
        self.mask == u32::MAX
    }
}

/// A checked signature: 1 to [`MAX_WORDS`] words, at least one of them whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature(Vec<MaskedWord>);

impl Signature {
    /// The signature `text` writes, or what is wrong with it.
    pub fn parse(text: &str) -> Result<Signature, String> {
        if text.trim().is_empty() {
            return Err("a signature is a string of instruction words".into());
        }
        let mut words = Vec::new();
        for w in text.to_ascii_lowercase().split_whitespace() {
            words.push(
                parse_word(w).ok_or_else(|| {
                    format!("{w:?} is not a word: 8 hex digits, ? for a masked nibble, or value/mask")
                })??,
            );
        }
        if words.len() > MAX_WORDS {
            return Err(format!(
                "{} words: a signature has at most {MAX_WORDS} (as few as make it unique)",
                words.len()
            ));
        }
        if !words.iter().any(|w| w.is_whole()) {
            return Err("no word is unmasked: one whole word is what the search starts from".into());
        }
        Ok(Signature(words))
    }

    /// Its words.
    pub fn words(&self) -> &[MaskedWord] {
        &self.0
    }

    /// How many words it has.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none (a parsed signature always has one).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether its words are at word index `i` of `code`.
    pub fn matches_at(&self, code: &[u32], i: usize) -> bool {
        code.get(i..i + self.len()).is_some_and(|ws| ws.iter().zip(&self.0).all(|(&w, m)| m.matches(w)))
    }

    /// The addresses where its words are, in `code` (instructions from address `base`), in address order: at most
    /// `limit` of them. The search starts from its first whole word.
    pub fn find(&self, code: &[u32], base: u64, limit: usize) -> Vec<u64> {
        let k = self.0.iter().position(|w| w.is_whole()).unwrap_or(0);
        let first = self.0[k].value;
        let mut out = Vec::new();
        for (i, &w) in code.iter().enumerate() {
            if w != first || i < k {
                continue;
            }
            if self.matches_at(code, i - k) {
                out.push(base + 4 * (i - k) as u64);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    /// (for a signature found nowhere) The places where all its words but one match: (address, the index of the word
    /// that differs), at most `limit`, which say what moved.
    pub fn near(&self, code: &[u32], base: u64, limit: usize) -> Vec<(u64, usize)> {
        let n = self.len();
        if n < 2 || code.len() < n {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..=code.len() - n {
            let mut wrong = None;
            let mut misses = 0;
            for (j, m) in self.0.iter().enumerate() {
                if !m.matches(code[i + j]) {
                    misses += 1;
                    wrong.get_or_insert(j);
                    if misses > 1 {
                        break;
                    }
                }
            }
            if misses == 1 {
                out.push((base + 4 * i as u64, wrong.unwrap_or(0)));
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }
}

/// `Some(Ok(word))`, `Some(Err(why))` for a word whose value sets bits its mask does not keep, `None` for no word.
fn parse_word(w: &str) -> Option<Result<MaskedWord, String>> {
    if let Some((value, mask)) = w.split_once('/') {
        let (value, mask) = (hex8(value)?, hex8(mask)?);
        if value & !mask != 0 {
            return Some(Err(format!("{w:?} sets bits its mask does not keep")));
        }
        return Some(Ok(MaskedWord { value, mask }));
    }
    if w.len() != 8 || !w.chars().all(|c| c.is_ascii_hexdigit() || c == '?') {
        return None;
    }
    let mut value = 0u32;
    let mut mask = 0u32;
    for c in w.chars() {
        value <<= 4;
        mask <<= 4;
        if let Some(d) = c.to_digit(16) {
            value |= d;
            mask |= 0xf;
        }
    }
    Some(Ok(MaskedWord { value, mask }))
}

fn hex8(s: &str) -> Option<u32> {
    (s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

impl fmt::Display for Signature {
    /// The text back: nibble masks where the mask allows it, else value/mask.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, w) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            let nibbles: Vec<u32> = (0..8).map(|i| (w.mask >> (28 - 4 * i)) & 15).collect();
            if nibbles.iter().all(|&n| n == 0 || n == 15) {
                for (i, &n) in nibbles.iter().enumerate() {
                    if n == 0 {
                        f.write_str("?")?;
                    } else {
                        write!(f, "{:x}", (w.value >> (28 - 4 * i)) & 15)?;
                    }
                }
            } else {
                write!(f, "{:08x}/{:08x}", w.value, w.mask)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x8000_1000;

    fn sig(text: &str) -> Signature {
        Signature::parse(text).unwrap()
    }

    #[test]
    fn parses_nibbles_and_masks() {
        let s = sig("d10043ff 9100?3fd f9400000/ffc003e0");
        assert_eq!(
            s.words(),
            &[
                MaskedWord { value: 0xd10043ff, mask: 0xffffffff },
                MaskedWord { value: 0x910003fd, mask: 0xffff0fff },
                MaskedWord { value: 0xf9400000, mask: 0xffc003e0 },
            ]
        );
        assert_eq!(s.to_string(), "d10043ff 9100?3fd f9400000/ffc003e0");
    }

    #[test]
    fn refuses_what_is_not_a_signature() {
        for (text, what) in [
            ("", "a string of instruction words"),
            ("d10043ff 9100", "not a word"),
            ("d10043fg", "not a word"),
            ("????????", "unmasked"),
            ("d10043ff 00000003/00000001", "bits its mask"),
        ] {
            let e = Signature::parse(text).unwrap_err();
            assert!(e.contains(what), "{text:?}: {e}");
        }
        let long = vec!["d10043ff"; 17].join(" ");
        assert!(Signature::parse(&long).unwrap_err().contains("at most"));
    }

    #[test]
    fn finds_masked_words_on_the_grid_only() {
        let s = sig("d10043ff 9100?3fd f9400000/ffc003e0");
        assert_eq!(s.find(&[0xd503201f, 0xd10043ff, 0x910043fd, 0xf9400c08, 0xd65f03c0], BASE, 8), [BASE + 4]);
        assert_eq!(s.find(&[0xd10043ff, 0x9100f3fd, 0xf9400000 | 5 | (0x3f << 10), 0], BASE, 8), [BASE]);
        assert!(s.find(&[0xd10043ff, 0x9100f3fc, 0xf9400c08], BASE, 8).is_empty());
        assert!(s.find(&[0xd10043ff, 0x910043fd], BASE, 8).is_empty(), "the code ends before the last word");
        let any = sig("d10043ff ???????? d65f03c0");
        assert_eq!(any.find(&[0xd10043ff, 0x12345678, 0xd65f03c0], BASE, 8), [BASE]);
        assert_eq!(sig("11223344").find(&[0x11223344; 20], BASE, 8).len(), 8);
    }

    #[test]
    fn names_the_nearest_misses() {
        let s = sig("d10043ff 910043fd f9400c08");
        let code = [0xd10043ff, 0x910043fd, 0xf9400c09, 0xd503201f, 0xd10043ff, 0x12345678];
        assert_eq!(s.near(&code, BASE, 4), [(BASE, 2)]);
    }
}
