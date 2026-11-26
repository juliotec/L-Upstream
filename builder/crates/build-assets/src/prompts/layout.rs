//! The places in the game's layouts (`UI/Layout2Archive/Common.*.blarc`, BFFH layouts) that follow the controls: the
//! Talk ring's buttons (`ring.bin`) and the ocarina hint's icons (`<face>/hint.bin`). `host/prompts.c` writes what
//! they say over the unpacked archive (`prompts/Common.blarc`) for the controls in use.
//!
//! ```text
//! ring.bin: "HPRG", u32 count, then each: u8 group (HOS_SW_A..Y), u8 the Switch button placed, u8 place (east south
//!           north west), u8 0, u32 runs, the runs (u32 offset, u32 length, the bytes)
//! hint.bin: "HPHN", u32 count of pad inputs, each one's icon's extra width (f32, layout units), u32 panes, then each:
//!           u32 offset (of its x in Common.blarc), u8 slot (0-4 the notes' icons in the hint's order, 5 the text),
//!           u8 0 x3, f32 its x
//! ```

use super::faces::sw;

/// The layout archive, under the romfs.
pub const LAYOUTS: &str = "UI/Layout2Archive/Common.Product.100.Nin_Ounce_NVN2.blarc.zs";

fn f32_at(d: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn u64_at(d: &[u8], at: usize) -> Option<usize> {
    Some(u64::from_le_bytes(d.get(at..at.checked_add(8)?)?.try_into().ok()?) as usize)
}

fn find(hay: &[u8], needle: &[u8], from: usize, to: usize) -> Option<usize> {
    hay.get(from..to)?.windows(needle.len()).position(|w| w == needle).map(|i| from + i)
}

/// The (start, end) of each BFFH layout in the archive that is the layout `name`.
pub fn layout_copies(blarc: &[u8], name: &str) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(i) = find(blarc, b"BFFH", at, blarc.len()) {
        starts.push(i);
        at = i + 4;
    }
    let mut mark = vec![0];
    mark.extend_from_slice(name.as_bytes());
    mark.extend_from_slice(b"\0RootPane\0");
    let ends: Vec<usize> = starts.iter().skip(1).copied().chain(std::iter::once(blarc.len())).collect();
    starts.into_iter().zip(ends).filter(|&(a, z)| find(blarc, &mark, a, z).is_some_and(|i| i > 0)).collect()
}

/// A run of bytes to write over the archive.
pub type Run = (usize, Vec<u8>);

/// `runs` without those that reach past a file of `len` bytes (the host writes a run where the game's file is).
pub fn keep_within(runs: &mut Vec<Run>, len: usize) {
    runs.retain(|(o, v)| o.checked_add(v.len()).is_some_and(|end| end <= len));
}

/// One of `ring.bin`'s entries.
pub struct RingEntry {
    /// The group (`HOS_SW_A..Y`).
    pub group: u8,
    /// The Switch button placed.
    pub placed: u8,
    /// The place (east, south, north, west).
    pub place: u8,
    /// What to write.
    pub runs: Vec<Run>,
}

/// The Talk ring's places (layout units from its centre): east, south, north, west.
const RING_PLACES: [(f32, f32); 4] = [(80.0, 0.0), (0.0, -80.0), (0.0, 80.0), (-80.0, 0.0)];
/// A ring group: its Switch button, its base's pane, the others' letters' panes, its big button's pane.
type RingGroup = (&'static str, usize, [(&'static str, usize); 3], usize);

/// Each group's panes (in the layout's order): its base, the others' letters, its big button (then its five children).
const RING_GROUPS: [RingGroup; 4] = [
    ("a", 5, [("b", 6), ("x", 7), ("y", 8)], 9),
    ("b", 16, [("a", 17), ("x", 18), ("y", 19)], 20),
    ("x", 27, [("a", 28), ("b", 29), ("y", 30)], 31),
    ("y", 38, [("a", 39), ("b", 40), ("x", 41)], 42),
];
/// The group whose big button is at each place gives the base's look and the button's banner.
const RING_AT: [&str; 4] = ["a", "b", "x", "y"];

/// `ring.bin`'s entries (`DoCommand_00`, four copies): one group per Switch button an action can be on, each its base,
/// its big button and the three others' letters, put where each pad input is. `None` when the layout is not the one
/// known.
pub fn ring_entries(blarc: &[u8]) -> Option<Vec<RingEntry>> {
    // (a pane's record: its transform (rotation, scale, translation) first, then parent, size, material)
    let pane = |k: usize| 0x968 + k * 0x44 - 0x28;
    let copies: Vec<usize> = layout_copies(blarc, "DoCommand_00").into_iter().map(|c| c.0).collect();
    let first = *copies.first()?;
    let lay = blarc.get(first..first.checked_add(0x2000)?)?;
    for (_, base, _, _) in RING_GROUPS {
        if (f32_at(lay, pane(base) + 0x2c)?, f32_at(lay, pane(base) + 0x30)?) != (320.0, 320.0) {
            return None;
        }
    }
    let group_of = |name: &str| RING_GROUPS.iter().find(|g| g.0 == name).expect("a group");
    let mut out = Vec::new();
    for &(group, base, small, btn) in &RING_GROUPS {
        for (place, xy) in RING_PLACES.iter().enumerate() {
            let &(_, tb, _, tbtn) = group_of(RING_AT[place]);
            let mut parts: Vec<(usize, &[u8])> = vec![
                (pane(base) + 0x10, &lay[pane(tb) + 0x10..pane(tb) + 0x18]), // (the base's scale, its material)
                (pane(base) + 0x3c, &lay[pane(tb) + 0x3c..pane(tb) + 0x40]),
                (pane(btn) + 0x1c, &lay[pane(tbtn) + 0x1c..pane(tbtn) + 0x24]), // (the button, its banner and text)
            ];
            for c in 1..6 {
                parts.push((pane(btn + c), &lay[pane(tbtn + c)..pane(tbtn + c) + 0x24]));
            }
            let runs = copies.iter().flat_map(|&c| parts.iter().map(move |(o, v)| (c + o, v.to_vec()))).collect();
            out.push(RingEntry { group: sw(group) as u8, placed: sw(group) as u8, place: place as u8, runs });
            for &(other, k) in &small {
                let mut xyb = xy.0.to_le_bytes().to_vec();
                xyb.extend_from_slice(&xy.1.to_le_bytes());
                let runs = copies.iter().map(|&c| (c + pane(k) + 0x1c, xyb.clone())).collect();
                out.push(RingEntry { group: sw(group) as u8, placed: sw(other) as u8, place: place as u8, runs });
            }
        }
    }
    // (a copy too near the archive's end to hold its panes: its runs would write past the file, so they are left out)
    for e in &mut out {
        keep_within(&mut e.runs, blarc.len());
    }
    Some(out)
}

/// `ring.bin`'s bytes.
pub fn ring_bin(entries: &[RingEntry]) -> Vec<u8> {
    let mut out = b"HPRG".to_vec();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        out.extend_from_slice(&[e.group, e.placed, e.place, 0]);
        put_runs(&mut out, &e.runs);
    }
    out
}

/// Runs as `u32 count`, then each `u32 offset, u32 length, the bytes`.
pub fn put_runs(out: &mut Vec<u8>, runs: &[Run]) {
    out.extend_from_slice(&(runs.len() as u32).to_le_bytes());
    for (off, data) in runs {
        out.extend_from_slice(&(*off as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
}

/// The ocarina's hints ("Play a note" and the notes' icons, and while a song is taught to the scarecrow): each icon a
/// text pane of its own, drawn from its left, 90 apart, room for the game's icons.
const HINTS: [&str; 3] = ["PaGuideKeyOcarinaInput_00", "PaGuideKeyOcarinaInput_01", "PaGuideKeyCapture_00"];
const HINT_PANES: [&str; 6] = ["T_Icon_00", "T_Icon_01", "T_Icon_02", "T_Icon_03", "T_Icon_04", "T_Guide_00"];
/// The widest advance the game's spacing holds (its own shoulder icons; font units).
const HINT_FITS: f64 = 720.0;
/// Layout units per font unit (the icons' panes draw the font at 100 per 1000 em).
const HINT_SCALE: f64 = 0.1;

/// A BFFH layout's panes: (name, its record's offset); the header gives the names' table (+0x118), the records' table
/// (+0x120) and the count (+0x130).
fn layout_panes(lay: &[u8]) -> Option<Vec<(String, usize)>> {
    let (names, recs, n) = (u64_at(lay, 0x118)?, u64_at(lay, 0x120)?, u32_at(lay, 0x130)? as usize);
    // (each pane is 8 bytes of each table: no more of them than the layout holds)
    let n = n.min(lay.len() / 8);
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let name = u64_at(lay, names.checked_add(8 * k)?)?;
        let end = lay.get(name..)?.iter().position(|&b| b == 0)? + name;
        out.push((String::from_utf8_lossy(&lay[name..end]).into_owned(), u64_at(lay, recs.checked_add(8 * k)?)?));
    }
    Some(out)
}

/// The hint's panes in every copy: (offset of an x, slot, x); `None` when the layouts are not the ones known. Each
/// pane's x, and the keys of its x in the layout's animation (a curve's values are an array (u64 offset of the floats,
/// u32 count, u32 4, the floats) of (value, slope) pairs, taken when every value is a pane's x and every slope 0).
pub fn hint_panes(blarc: &[u8]) -> Option<Vec<(usize, u8, f32)>> {
    let mut out = Vec::new();
    for name in HINTS {
        for (a, z) in layout_copies(blarc, name) {
            let lay = &blarc[a..z];
            let panes = layout_panes(lay)?;
            let mut xs: Vec<(f32, u8)> = Vec::new();
            for (slot, p) in HINT_PANES.iter().enumerate() {
                let rec = panes.iter().find(|(n, _)| n == p)?.1.checked_add(0x1c)? - 0x1c;
                let x = f32_at(lay, rec + 0x1c)?;
                if !xs.iter().any(|e| e.0 == x) {
                    xs.push((x, slot as u8));
                }
                out.push((a + rec + 0x1c, slot as u8, x));
            }
            let mut o = 0;
            while o + 16 < lay.len() {
                let (at, n, size) = (u64_at(lay, o)?, u32_at(lay, o + 8)? as usize, u32_at(lay, o + 12)?);
                if at == o + 16 && size == 4 && (2..=64).contains(&n) && n % 2 == 0 && o + 16 + 4 * n <= lay.len() {
                    let v: Vec<f32> = (0..n).map(|k| f32_at(lay, at + 4 * k).unwrap_or(0.0)).collect();
                    if let Some(&(_, slot)) = xs.iter().find(|e| e.0 == v[0])
                        && v.iter().step_by(2).all(|&k| k == v[0])
                        && v.iter().skip(1).step_by(2).all(|&s| s == 0.0)
                    {
                        out.extend((0..n / 2).map(|k| (a + at + 8 * k, slot, v[0])));
                    }
                }
                o += 8;
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `hint.bin`'s bytes, of a face's icons' advances (per pad input; the keyboard's per key code).
pub fn hint_bin(widths: &[u16], panes: &[(usize, u8, f32)]) -> Vec<u8> {
    let mut out = b"HPHN".to_vec();
    out.extend_from_slice(&(widths.len() as u32).to_le_bytes());
    for &w in widths {
        let extra = ((f64::from(w) - HINT_FITS).max(0.0) * HINT_SCALE).round_ties_even();
        out.extend_from_slice(&(extra as f32).to_le_bytes());
    }
    out.extend_from_slice(&(panes.len() as u32).to_le_bytes());
    for &(at, slot, x) in panes {
        out.extend_from_slice(&(at as u32).to_le_bytes());
        out.extend_from_slice(&[slot, 0, 0, 0]);
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_past_the_file_are_left_out() {
        let mut runs: Vec<Run> = vec![(0, vec![1; 4]), (96, vec![2; 4]), (98, vec![3; 4]), (usize::MAX, vec![4])];
        keep_within(&mut runs, 100);
        assert_eq!(runs, [(0, vec![1; 4]), (96, vec![2; 4])]);
    }

    #[test]
    fn a_hint_s_widths_past_what_fits() {
        let b = hint_bin(&[700, 900], &[(16, 2, 45.0)]);
        assert_eq!(&b[..8], b"HPHN\x02\0\0\0");
        assert_eq!(f32::from_le_bytes(b[8..12].try_into().unwrap()), 0.0);
        assert_eq!(f32::from_le_bytes(b[12..16].try_into().unwrap()), 18.0);
        assert_eq!(b.len(), 16 + 4 + 12);
    }

    #[test]
    fn copies_by_name() {
        let mut a = b"BFFH....\0One\0RootPane\0".to_vec();
        a.extend_from_slice(b"BFFH\0Two\0RootPane\0");
        assert_eq!(layout_copies(&a, "Two"), [(22, a.len())]);
        assert!(ring_entries(&a).is_none());
        assert!(hint_panes(&a).is_none());
    }
}
