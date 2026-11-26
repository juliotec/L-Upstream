//! The button prompts of other controllers: the game's button-icon font redrawn per controller face, its textures
//! with button icons and its layouts' places for them, from the player's romfs and Kenney's Input Prompts (CC0,
//! `third_party/kenney_input`). `host/prompts.c` serves them in place of the game's files.
//!
//! The outputs, in the game folder's `prompts/`:
//!
//! - `External.bfotf`: one font with every face's icons ([`glyphs`]);
//! - `<face>/hint.bin`: the ocarina hint's spacing for the face's icons ([`hint`]);
//! - `<face>/textures.bin`: the blocks to write over the game's texture archive for each Switch button on each pad
//!   input ([`textures`]), and `Common.bntx.base`: the game's bytes under them ([`base`]; the host reads the archive
//!   itself from the romfs);
//! - `ring.bin`: where the Talk ring puts each button ([`layout`]), and `Common.blarc.base`: the layouts' bytes under
//!   it and the hints'.
//!
//! A `.bfotf` is an OpenType font behind an 8-byte header and XORed by little-endian words: the header is the magic
//! `0x18029a7f` and the font's size (big-endian), each XORed with the same key as the font.

pub mod base;
pub mod faces;
pub mod glyphs;
pub mod guides;
pub mod icons;
pub mod keys;
pub mod layout;
pub mod textures;

use crate::cache::Cache;
use crate::romfs::Romfs;
use crate::texture::{self, Texture};
use crate::zs::{self, Dictionaries};
use build_core::key::Key;
use build_core::report::Reporter;
use build_core::{Error, IoContext, Result, fsx};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The game's icon font, under the romfs.
pub const FONT: &str = "Font2/Scalable/External.Nin_Ounce_NVN2.bfotf.zs";
const MAGIC: u32 = 0x1802_9A7F;

/// A `.bfotf`'s key and its font.
pub fn unwrap(blob: &[u8]) -> Result<(u32, Vec<u8>)> {
    if blob.len() < 8 {
        return Err(Error::format("bfotf", "cut short"));
    }
    let word = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().expect("4 bytes"));
    let key = word(0) ^ MAGIC;
    let size = (word(4) ^ key).swap_bytes() as usize;
    let mut body = Vec::with_capacity(blob.len() - 8);
    for w in blob[8..].as_chunks::<4>().0 {
        body.extend_from_slice(&(u32::from_le_bytes(*w) ^ key).to_le_bytes());
    }
    if size > body.len() {
        return Err(Error::format("bfotf", "a font larger than its file"));
    }
    body.truncate(size);
    Ok((key, body))
}

/// A font as a `.bfotf` with `key`.
pub fn wrap(key: u32, otf: &[u8]) -> Vec<u8> {
    let mut otf = otf.to_vec();
    otf.resize(otf.len().next_multiple_of(4), 0);
    let mut out = Vec::with_capacity(otf.len() + 8);
    out.extend_from_slice(&(MAGIC ^ key).to_le_bytes());
    out.extend_from_slice(&((otf.len() as u32).swap_bytes() ^ key).to_le_bytes());
    for w in otf.as_chunks::<4>().0 {
        out.extend_from_slice(&(u32::from_le_bytes(*w) ^ key).to_le_bytes());
    }
    out
}

/// The recipes' versions: raised when the same inputs would make another output ([`crate::cache`]).
const FONT_RECIPE: &str = "prompts font 1";
const TEXTURES_RECIPE: &str = "prompts textures 1";
const LAYOUT_RECIPE: &str = "prompts layouts 2";

/// Where the prompts' own inputs are (the repository's, not the game's).
#[derive(Clone, Debug)]
pub struct Sources {
    /// Kenney's Input Prompts fonts (`third_party/kenney_input`).
    pub kenney: PathBuf,
    /// Kenney's keyboard font's folder (`third_party/kenney_input_keyboard`).
    pub kenney_keyboard: PathBuf,
    /// The hand-made pictures, a folder a face (`<faces>/<face>/<layer>.png`).
    pub faces: PathBuf,
}

impl Sources {
    /// The places in a checkout (or a packaged builder's tools) at `root`: the hand-made pictures in
    /// `assets/prompt_faces/`.
    pub fn in_checkout(root: &Path) -> Sources {
        Sources {
            kenney: root.join("third_party/kenney_input"),
            kenney_keyboard: root.join("third_party/kenney_input_keyboard"),
            faces: root.join("assets/prompt_faces"),
        }
    }
}

/// What to make.
#[derive(Clone, Debug)]
pub struct Options {
    /// The faces, in `HOS_PROMPT_*` order (default: all of them).
    pub faces: Vec<&'static str>,
    /// Whether to make the textures (else the game keeps the Switch's icons in them).
    pub textures: bool,
    /// A folder to write every face's layers into as PNG (and the game's textures they go into), to rework one by hand.
    pub dump: Option<PathBuf>,
    /// Workers for the faces' textures.
    pub jobs: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options { faces: faces::all_faces(), textures: true, dump: None, jobs: 1 }
    }
}

/// What [`make`] did: the outputs made now, and those kept.
#[derive(Debug, Default)]
pub struct Made {
    /// Made now.
    pub made: Vec<String>,
    /// Kept from an earlier build (their inputs the same).
    pub kept: Vec<String>,
}

fn kenney_fonts(sources: &Sources, cache: &Cache, key: &mut Key) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut out = BTreeMap::new();
    for dir in [&sources.kenney, &sources.kenney_keyboard] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "otf"))
            .collect();
        paths.sort();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            cache.key_file(key, &name, &p)?;
            out.insert(name, std::fs::read(&p).at(&p)?);
        }
    }
    Ok(out)
}

/// The button prompts in `out` (the game folder's `prompts/`), each output kept while what it is made from stays.
/// Files of an earlier build the game no longer reads are left alone; `host/prompts.c`'s own files beside them too.
pub fn make(
    romfs: &dyn Romfs,
    sources: &Sources,
    out: &Path,
    cache: &Cache,
    o: &Options,
    report: &Reporter,
) -> Result<Made> {
    let mut done = Made::default();
    let names = &o.faces;
    if names.first() != Some(&"switch") || names.iter().zip(faces::all_faces()).any(|(a, b)| *a != b) {
        return Err(Error::format(
            "prompts",
            "the faces must be the Switch, then the others in their order, then the keyboard",
        ));
    }
    let dicts = OnceCell::new();
    let dicts = || -> Result<&Dictionaries> {
        if dicts.get().is_none() {
            let _ = dicts.set(Dictionaries::load(romfs)?);
        }
        Ok(dicts.get().expect("just set"))
    };
    let romfs_key = |key: &mut Key, rel: &str| -> Result<()> {
        cache.key_romfs(key, romfs, rel)?;
        cache.key_romfs(key, romfs, zs::DICTIONARY_PACK)
    };
    let fresh = |stamp: &str, key: &Key, done: &mut Made| {
        let f = o.dump.is_none() && cache.fresh(stamp, key);
        if f {
            done.kept.push(stamp.to_string());
        }
        f
    };
    // (the font and each face's icons' advances, which the hints need)
    let mut font_key = Key::new();
    font_key.text("recipe", FONT_RECIPE).text("faces", &names.join(","));
    romfs_key(&mut font_key, FONT)?;
    if romfs.has(keys::LABEL_FONT) {
        cache.key_romfs(&mut font_key, romfs, keys::LABEL_FONT)?;
    }
    let kenney = kenney_fonts(sources, cache, &mut font_key)?;
    let font_path = out.join("External.bfotf");
    let widths_path = cache.dir().join("prompts-widths.json");
    let game_font = unwrap(&dicts()?.read(romfs, FONT)?)?;
    let widths: Vec<Vec<u16>> = match fresh("prompts.font", &font_key, &mut done)
        .then(|| std::fs::read(&widths_path).ok().and_then(|b| serde_json::from_slice(&b).ok()))
        .flatten()
    {
        Some(w) => w,
        None => {
            cache.forget("prompts.font")?;
            let label = if romfs.has(keys::LABEL_FONT) {
                Some(unwrap(&dicts()?.read(romfs, keys::LABEL_FONT)?)?.1)
            } else {
                None
            };
            let made = glyphs::faces_font(
                &glyphs::Fonts { game: &game_font.1, label: label.as_deref(), kenney: &kenney },
                names,
            )?;
            fsx::write_atomic(&font_path, &wrap(game_font.0, &made.font))?;
            let json = serde_json::to_vec(&made.widths).map_err(|e| Error::format("prompts", e.to_string()))?;
            fsx::write_atomic(&widths_path, &json)?;
            cache.record("prompts.font", &font_key, &[font_path.clone(), widths_path.clone()])?;
            done.made.push("prompts.font".into());
            made.widths
        }
    };
    // (the layouts: the ring and the hints)
    let mut lay_key = Key::new();
    lay_key.text("recipe", LAYOUT_RECIPE);
    romfs_key(&mut lay_key, layout::LAYOUTS)?;
    let mut hint_key = lay_key.clone();
    hint_key.text("font", &font_key.hex());
    let blarc_path = out.join("Common.blarc.base");
    // (an earlier layout's copies of the game's files: the host reads them from the romfs now)
    fsx::remove_file(&out.join("Common.blarc"))?;
    fsx::remove_file(&out.join("Common.bntx"))?;
    let ring_path = out.join("ring.bin");
    let hint_paths: Vec<PathBuf> = names.iter().map(|f| out.join(f).join("hint.bin")).collect();
    if !(fresh("prompts.layouts", &hint_key, &mut done)) {
        cache.forget("prompts.layouts")?;
        let blarc = dicts()?.read(romfs, layout::LAYOUTS)?;
        let hint = layout::hint_panes(&blarc);
        let ring = layout::ring_entries(&blarc);
        let mut outputs = Vec::new();
        if ring.is_some() || hint.is_some() {
            // (the game's bytes under the ring's runs and the hint's panes, and the first 32 the host finds it by)
            let mut ranges = vec![(0, 32)];
            for e in ring.iter().flatten() {
                ranges.extend(e.runs.iter().map(|(o, d)| (*o, d.len())));
            }
            ranges.extend(hint.iter().flatten().map(|(at, _, _)| (*at, 4)));
            fsx::write_atomic(&blarc_path, &base::base(&blarc, &ranges, &[]))?;
            outputs.push(blarc_path.clone());
        } else {
            fsx::remove_file(&blarc_path)?;
        }
        match &ring {
            Some(r) => {
                fsx::write_atomic(&ring_path, &layout::ring_bin(r))?;
                outputs.push(ring_path.clone());
            }
            None => {
                fsx::remove_file(&ring_path)?;
                report
                    .warn("prompts_ring", "The Talk ring keeps the Switch's places: its layout is not the one known.");
            }
        }
        for (path, w) in hint_paths.iter().zip(&widths) {
            match &hint {
                Some(panes) => {
                    fsx::write_atomic(path, &layout::hint_bin(w, panes))?;
                    outputs.push(path.clone());
                }
                None => fsx::remove_file(path)?,
            }
        }
        if hint.is_none() {
            report
                .warn("prompts_hint", "The ocarina's hint keeps the game's spacing: its layout is not the one known.");
        }
        cache.record("prompts.layouts", &hint_key, &outputs)?;
        done.made.push("prompts.layouts".into());
    }
    if !o.textures {
        return Ok(done);
    }
    // (the textures: each face's blocks, each kept by its own inputs; then what the host keeps of the game's file)
    let mut bntx_key = Key::new();
    bntx_key.text("recipe", TEXTURES_RECIPE);
    romfs_key(&mut bntx_key, textures::TEXTURES)?;
    // (the keyboard's: the Switch's textures, host/prompts.c)
    let drawn: Vec<&str> = names.iter().copied().filter(|f| *f != "keyboard").collect();
    let mut todo = Vec::new();
    for &face in &drawn {
        let mut key = bntx_key.clone();
        key.text("face", face);
        romfs_key(&mut key, FONT)?;
        if let Some(f) = faces::face(face) {
            key.text("kenney", &kenney.get(f.font).map(|b| build_core::key::sha1_hex(b)).unwrap_or_default());
        }
        cache.key_tree(&mut key, "hand", &sources.faces.join(face), ".png")?;
        let stamp = format!("prompts.textures.{face}");
        if !fresh(&stamp, &key, &mut done) {
            cache.forget(&stamp)?;
            todo.push((face, key, stamp));
        }
    }
    let mut bntx: Option<Vec<u8>> = None;
    if !todo.is_empty() {
        let b = dicts()?.read(romfs, textures::TEXTURES)?;
        // (an archive this does not read, or whose textures are not the known ones: the game's own icons stay)
        let read = texture::bntx_textures(&b).map(|t| t.into_iter().collect::<HashMap<String, Texture>>());
        let why = match &read {
            Err(e) => Some(e.to_string()),
            Ok(t) => textures::unknown(t),
        };
        if let Some(why) = why {
            report.warn(
                "prompts_textures",
                format!("The button prompts keep the game's own icons: its textures are not the ones known ({why})."),
            );
            for (face, _, _) in &todo {
                fsx::remove_file(&out.join(face).join("textures.bin"))?;
            }
            cache.forget("prompts.bntx")?;
            fsx::remove_file(&out.join("Common.bntx.base"))?;
            return Ok(done);
        }
        let textures = read.unwrap_or_default();
        let next = AtomicUsize::new(0);
        let results: Vec<Mutex<Option<Result<()>>>> = todo.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..o.jobs.clamp(1, todo.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some((face, _, _)) = todo.get(i) else { break };
                        let r = face_file(
                            face,
                            &b,
                            &textures,
                            &game_font.1,
                            &kenney,
                            sources,
                            out,
                            o.dump.as_deref(),
                            report,
                        );
                        *results[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                    }
                });
            }
        });
        for ((face, key, stamp), r) in todo.iter().zip(results) {
            r.into_inner().unwrap_or_else(|p| p.into_inner()).unwrap_or(Ok(()))?;
            cache.record(stamp, key, &[out.join(face).join("textures.bin")])?;
            done.made.push(stamp.clone());
        }
        bntx = Some(b);
    }
    // (the game's bytes under every face's runs: kept while the faces' textures and the game's file are the same)
    let bins: Vec<PathBuf> = drawn.iter().map(|f| out.join(f).join("textures.bin")).collect();
    let mut base_key = bntx_key.clone();
    for p in &bins {
        cache.key_maybe_file(&mut base_key, &p.to_string_lossy(), p)?;
    }
    let base_path = out.join("Common.bntx.base");
    if !fresh("prompts.bntx", &base_key, &mut done) {
        cache.forget("prompts.bntx")?;
        let b = match bntx {
            Some(b) => b,
            None => dicts()?.read(romfs, textures::TEXTURES)?,
        };
        let mut ranges = Vec::new();
        for p in &bins {
            if let Ok(bin) = std::fs::read(p) {
                ranges.extend(base::run_ranges(&bin));
            }
        }
        fsx::write_atomic(&base_path, &base::textures_base(&b, &ranges))?;
        cache.record("prompts.bntx", &base_key, std::slice::from_ref(&base_path))?;
        done.made.push("prompts.bntx".into());
    }
    Ok(done)
}

/// Blocks encoded, by their format and texels.
type BlockMemo = HashMap<(u32, Vec<[u8; 4]>), Vec<u8>>;

/// One face's `textures.bin` (and its layers in `dump`).
#[allow(clippy::too_many_arguments)]
fn face_file(
    face: &str,
    bntx: &[u8],
    textures: &HashMap<String, Texture>,
    game_font: &[u8],
    kenney: &BTreeMap<String, Vec<u8>>,
    sources: &Sources,
    out: &Path,
    dump: Option<&Path>,
    report: &Reporter,
) -> Result<()> {
    let hand = sources.faces.join(face);
    let inp = textures::Inputs {
        bntx,
        textures,
        game_font,
        kenney: faces::face(face).and_then(|f| kenney.get(f.font)).map(Vec::as_slice),
        hand: hand.is_dir().then_some(hand.as_path()),
    };
    let layers = textures::face_layers(face, &inp, &mut |l| report.log(l))?;
    // (a block met again, in another layer or another texture of the same format, is encoded once)
    let seen: RefCell<BlockMemo> = RefCell::new(HashMap::new());
    let encode = |t: &Texture, texels: &[[u8; 4]], srgb: bool| -> Vec<u8> {
        let k = (t.format, texels.to_vec());
        if let Some(b) = seen.borrow().get(&k) {
            return b.clone();
        }
        let b = texture::encode_for(t, texels, srgb);
        seen.borrow_mut().insert(k, b.clone());
        b
    };
    let entries = textures::face_textures(&layers, &inp, &encode)?;
    fsx::write_atomic(&out.join(face).join("textures.bin"), &textures::textures_bin(&entries))?;
    if let Some(d) = dump {
        for l in &layers {
            fsx::write_atomic(&d.join(face).join(&l.name), &l.im.png())?;
            let game = d.join("game").join(format!("{}.png", l.texture));
            if !game.exists() {
                fsx::write_atomic(&game, &texture::decode(bntx, &textures[l.texture])?.png())?;
            }
        }
    }
    report.log(format!("prompts: {face}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bfotf_round_trip() {
        let otf = b"OTTO and a font of 24 by".to_vec();
        let blob = wrap(0x1234_5678, &otf);
        assert_eq!(blob.len(), 8 + 24);
        assert_eq!(unwrap(&blob).unwrap(), (0x1234_5678, otf));
    }
}
