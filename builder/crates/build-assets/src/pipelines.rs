//! The pipeline state lists (`gpu/vk/vk_draw.cpp`: `longshot_pipeline_states.bin`, written as the game is played), and
//! the one a release ships: the repository's sources in `assets/pipeline_states/` merged into the game folder's
//! `longshot_pipeline_states_shipped.bin`, whose pipelines the first start's shader preparation builds before the
//! title (`assets/pipeline_states/README.md`).
//!
//! The sources: `tour.bin.xz` (the pipeline tour's), then `contrib/<name>.bin.xz` (one a contributed list) by name,
//! each beside its note `<source>.md`. They merge in that order, each pipeline once, so the same sources always give
//! the same bytes. A list may be xz-compressed (known by its magic): the repository's are.
//!
//! A list holds no game code: each record is a pipeline's stage code keys (hashes of the game's shaders), the program
//! variant's hash, and the formats and draw state baked into it; or a compute program, by its compute stage's key
//! alone.
//!
//! ```text
//! version 2: "HKSTATE2", u32 record size (552), then the records:
//!     u64 code[4] (vertex or mesh, fragment, task, the header-only stage), u64 variant,
//!     u32 mesh, topology, targets, samples, has depth, depth NVN format,
//!     u32 colour NVN texture formats[8], u32 colour NVN view formats[8], u64 blend[8],
//!     u32 vertex attributes[16], u32 attribute count, u32 strides[16], u32 divisors[16],
//!     u32 blend enables, channel mask, polygon mode, depth clamp, alpha to coverage,
//!     u32 answers, u32 questions, u32 answers[24], u8 questions[32], u32 kind (0 graphics or mesh, 1 compute), u32 pad
//! version 1: "HKSTATE1", u32 record size (528): the recording GPU's Vulkan formats in place of the NVN ones, the
//!     mesh fallback's topology and stage numbering; converted to version 2 when merged with any version 2 list
//! ```

use build_core::{Error, IoContext, Result, fsx};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Version 1's magic and record size.
pub const MAGIC1: &[u8; 8] = b"HKSTATE1";
/// Version 1's record size.
pub const SIZE1: usize = 528;
/// Version 2's magic.
pub const MAGIC2: &[u8; 8] = b"HKSTATE2";
/// Version 2's record size.
pub const SIZE2: usize = 552;
/// Where a version 2 record's kind is.
pub const KIND_AT: usize = SIZE2 - 8;
/// The kind of a compute program's record.
pub const COMPUTE: u32 = 1;
/// The compute programs a shipped list may name (`scripts/pipeline_states.py` has the measurements behind it).
pub const COMPUTE_MOST: usize = 3584;
/// The compute programs one list someone sent may name: twice the game's.
pub const SENT_COMPUTE_MOST: usize = 8192;
/// The bytes of one list someone sent, packed or unpacked.
pub const SENT_MOST: usize = 256 << 20;
/// The shipped list, in the game folder.
pub const SHIPPED: &str = "longshot_pipeline_states_shipped.bin";
/// `VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST`: what a mesh draw is given.
const TRIANGLE_LIST: u32 = 3;
const XZ_MAGIC: &[u8] = b"\xfd7zXZ\x00";
const TOUR: &str = "tour.bin.xz";
const CONTRIB: &str = "contrib";
const EXT: &str = ".bin.xz";

/// A version 1 record's Vulkan format and the NVN format `vk_format` (`gpu/vk/vk_formats.cpp`) makes it on every GPU:
/// the formats a target can be.
const V1_NVN: &[(u32, u32)] = &[
    (4, 0x95),
    (5, 0x3c),
    (6, 0x98),
    (8, 0x3b),
    (9, 0x01),
    (10, 0x02),
    (13, 0x03),
    (14, 0x04),
    (16, 0x0d),
    (17, 0x0e),
    (20, 0x0f),
    (21, 0x10),
    (23, 0x19),
    (24, 0x1a),
    (27, 0x1b),
    (28, 0x1c),
    (37, 0x6d),
    (38, 0x6e),
    (41, 0x6f),
    (42, 0x70),
    (43, 0x37),
    (44, 0x99),
    (50, 0x9b),
    (64, 0x3d),
    (68, 0x3e),
    (70, 0x06),
    (71, 0x07),
    (74, 0x08),
    (75, 0x09),
    (76, 0x05),
    (77, 0x12),
    (78, 0x13),
    (81, 0x14),
    (82, 0x15),
    (83, 0x11),
    (91, 0x72),
    (92, 0x73),
    (95, 0x74),
    (96, 0x75),
    (97, 0x29),
    (98, 0x0b),
    (99, 0x0c),
    (100, 0x0a),
    (101, 0x17),
    (102, 0x18),
    (103, 0x16),
    (106, 0x22),
    (107, 0x77),
    (108, 0x78),
    (109, 0x76),
    (122, 0x3f),
    (123, 0x40),
    (124, 0x32),
    (125, 0x33),
    (126, 0x34),
    (127, 0x31),
    (129, 0x35),
    (130, 0x36),
    (1_000_340_001, 0x39),
];

fn nvn_of(vk: u32) -> Option<u32> {
    V1_NVN.iter().find(|(v, _)| *v == vk).map(|(_, n)| *n)
}

fn u32_at(r: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(r[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(r: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(r[at..at + 8].try_into().expect("8 bytes"))
}

/// A writer that stops past a limit: an xz file a few KB long may unpack to gigabytes.
struct Limited {
    out: Vec<u8>,
    most: usize,
}

impl Write for Limited {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.out.len() + b.len() > self.most {
            return Err(std::io::Error::other("over the limit"));
        }
        self.out.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A list's bytes as the game reads them: unpacked when they are xz. `most`: no more than that many, packed or
/// unpacked (a list someone sent).
pub fn unpack(data: &[u8], what: &str, most: Option<usize>) -> Result<Vec<u8>> {
    let most = most.unwrap_or(usize::MAX);
    if data.len() > most {
        return Err(Error::format(
            what,
            format!("{} bytes, more than a pipeline state list holds ({most})", data.len()),
        ));
    }
    if !data.starts_with(XZ_MAGIC) {
        return Ok(data.to_vec());
    }
    let mut out = Limited { out: Vec::new(), most };
    match lzma_rs::xz_decompress(&mut &data[..], &mut out) {
        Ok(()) => Ok(out.out),
        Err(lzma_rs::error::Error::IoError(e)) if e.to_string() == "over the limit" => {
            Err(Error::format(what, format!("unpacks to more than a pipeline state list holds ({most} bytes)")))
        }
        Err(e) => Err(Error::format(what, format!("a damaged xz file ({e})"))),
    }
}

/// A list's file read and unpacked.
pub fn load(path: &Path, most: Option<usize>) -> Result<Vec<u8>> {
    let data = std::fs::read(path).at(path)?;
    unpack(&data, &path.display().to_string(), most)
}

/// The records of a list's bytes (either version: the header says which). A record cut short at the end (a game that
/// ended while it wrote) is left out, as the game leaves it; `strict`: refused.
pub fn parse<'a>(data: &'a [u8], what: &str, strict: bool) -> Result<Vec<&'a [u8]>> {
    let size = if data.len() >= 12 { u32_at(data, 8) as usize } else { 0 };
    let known = (data.starts_with(MAGIC1) && size == SIZE1) || (data.starts_with(MAGIC2) && size == SIZE2);
    if !known {
        let found = if data.starts_with(&MAGIC1[..7]) {
            String::from_utf8_lossy(&data[..8.min(data.len())]).into_owned()
        } else {
            "no list".into()
        };
        return Err(Error::format(
            what,
            format!(
                "not a pipeline state list this build reads (HKSTATE2, {SIZE2}-byte records, or HKSTATE1, {SIZE1}; \
                 found {found}): make it again with this build (docs/dev/reference/pipeline-states.md)"
            ),
        ));
    }
    let over = (data.len() - 12) % size;
    if over != 0 && strict {
        return Err(Error::format(what, format!("cut short ({over} bytes over the last whole record)")));
    }
    Ok(data[12..data.len() - over].chunks_exact(size).collect())
}

/// A record's kind: 0 a graphics or mesh pipeline (every version 1 record), [`COMPUTE`] a compute program.
pub fn kind(r: &[u8]) -> u32 {
    if r.len() == SIZE2 { u32_at(r, KIND_AT) } else { 0 }
}

/// The record of a compute program: its compute stage's code key, the rest 0 (`vk_draw.cpp state_record_compute`).
pub fn compute_record(key: u64) -> Vec<u8> {
    let mut r = vec![0u8; SIZE2];
    r[..8].copy_from_slice(&key.to_le_bytes());
    r[KIND_AT..KIND_AT + 4].copy_from_slice(&COMPUTE.to_le_bytes());
    r
}

/// The compute programs records name, each once, in the order first seen.
pub fn compute_keys<R: AsRef<[u8]>>(recs: &[R]) -> Vec<u64> {
    let mut seen = HashSet::new();
    recs.iter()
        .map(AsRef::as_ref)
        .filter(|r| kind(r) == COMPUTE)
        .map(|r| u64_at(r, 0))
        .filter(|k| seen.insert(*k))
        .collect()
}

/// Each record of a kind this build knows, a compute program's exactly as the game writes it.
pub fn check_kinds<R: AsRef<[u8]>>(recs: &[R], what: &str) -> Result<()> {
    for (i, r) in recs.iter().map(AsRef::as_ref).enumerate() {
        match kind(r) {
            0 => {}
            COMPUTE => {
                let key = u64_at(r, 0);
                if key == 0 || r != compute_record(key).as_slice() {
                    return Err(Error::format(
                        what,
                        format!(
                            "record {i} names a compute program but holds more than its key (or no key): not one the game wrote"
                        ),
                    ));
                }
            }
            k => {
                return Err(Error::format(
                    what,
                    format!(
                        "record {i} is of kind {k}, which this build does not know: make the list again with this build"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The records of a list someone sent (the "Submit your pipelines" issue's zips): taken only when it is exactly a list
/// (this build's magic and record size, whole records, every record of a known kind, no more compute programs than the
/// game could have, no more than [`SENT_MOST`] bytes packed or unpacked).
pub fn read_sent(data: &[u8], what: &str) -> Result<Vec<Vec<u8>>> {
    let body = unpack(data, what, Some(SENT_MOST))?;
    let recs: Vec<Vec<u8>> = parse(&body, what, true)?.into_iter().map(<[u8]>::to_vec).collect();
    if recs.is_empty() {
        return Err(Error::format(what, "holds no pipelines"));
    }
    check_kinds(&recs, what)?;
    let computes = compute_keys(&recs).len();
    if computes > SENT_COMPUTE_MOST {
        return Err(Error::format(
            what,
            format!(
                "names {computes} compute programs, more than the game has (at most {SENT_COMPUTE_MOST}): not a list the game wrote"
            ),
        ));
    }
    Ok(recs)
}

/// `Program::variant` of packed answers (`vk_shader.cpp answers_hash`).
pub fn answers_hash(answers: &[u32]) -> u64 {
    let mut h = 0u64;
    for &a in answers {
        let (stage, desc, image, typ) =
            (u64::from(a >> 28), u64::from(a >> 12 & 0xFFFF), u64::from(a >> 8 & 1), u64::from(a & 0xFF));
        h = (h ^ (stage << 32 | desc << 16 | typ << 1 | image)).wrapping_mul(1_099_511_628_211);
    }
    h
}

/// A version 1 record as version 2, or `None` when it cannot be (a format the table lacks, a mesh fallback record
/// without its answers). `vk_draw.cpp state_v1_portable` does the same in the game.
pub fn v1_to_v2(r: &[u8]) -> Option<Vec<u8>> {
    let mut variant = u64_at(r, 32);
    let (mesh, mut topo, n, samples) = (u32_at(r, 40), u32_at(r, 44), u32_at(r, 48) as usize, u32_at(r, 52));
    let (depth_format, has_depth) = (u32_at(r, 56), u32_at(r, 60));
    let (nanswers, nquestions) = (u32_at(r, 388) as usize, u32_at(r, 392) as usize);
    let mut answers: Vec<u32> = (0..24).map(|i| u32_at(r, 396 + 4 * i)).collect();
    if nanswers > 24 || nquestions > 32 || n > 8 {
        return None;
    }
    if mesh != 0 {
        // (made with the mesh fallback: its generated vertex stage after (task,) mesh, and the fallback's topology)
        let g = if u64_at(r, 16) != 0 { 2 } else { 1 };
        let fallback = topo != TRIANGLE_LIST || answers[..nanswers].iter().any(|a| a >> 28 > g);
        if fallback {
            if nanswers == 0 {
                return None;
            }
            for a in &mut answers[..nanswers] {
                if *a >> 28 > g {
                    *a = (*a & 0x0FFF_FFFF) | ((*a >> 28) - 1) << 28;
                }
            }
            variant = answers_hash(&answers[..nanswers]);
            topo = TRIANGLE_LIST;
        }
    }
    let mut nvn = [0u32; 8];
    for (k, slot) in nvn.iter_mut().enumerate() {
        let colour = u32_at(r, 64 + 4 * k);
        if k < n && colour != 0 {
            *slot = nvn_of(colour)?;
        }
    }
    let depth_nvn = if has_depth != 0 && depth_format != 0 { nvn_of(depth_format)? } else { 0 };
    let mut out = Vec::with_capacity(SIZE2);
    out.extend_from_slice(&r[..32]);
    out.extend_from_slice(&variant.to_le_bytes());
    for v in [mesh, topo, n as u32, samples, has_depth, depth_nvn] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in nvn.iter().chain(nvn.iter()) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&r[104..168]); // (blend)
    out.extend_from_slice(&r[168..236]); // (vertex attributes and their count)
    out.extend_from_slice(&r[236..364]); // (strides, divisors)
    out.extend_from_slice(&r[364..384]); // (blend enables, channel mask, polygon mode, depth clamp, alpha to coverage)
    for v in [nanswers as u32, nquestions as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for a in &answers {
        out.extend_from_slice(&a.to_le_bytes());
    }
    out.extend_from_slice(&r[492..524]); // (questions)
    out.extend_from_slice(&[0u8; 8]); // (kind 0, pad)
    debug_assert_eq!(out.len(), SIZE2);
    Some(out)
}

/// Lists of records as one version: as they are when all are version 1 (and not `force`), else each version 1 record
/// converted, those that cannot be left out. (The lists, how many were left out.)
pub fn as_v2(lists: Vec<Vec<Vec<u8>>>, force: bool) -> (Vec<Vec<Vec<u8>>>, usize) {
    if !force && lists.iter().flatten().all(|r| r.len() == SIZE1) {
        return (lists, 0);
    }
    let mut lost = 0;
    let out = lists
        .into_iter()
        .map(|recs| {
            recs.into_iter()
                .filter_map(|r| {
                    let conv = if r.len() == SIZE1 { v1_to_v2(&r) } else { Some(r) };
                    lost += usize::from(conv.is_none());
                    conv
                })
                .collect()
        })
        .collect();
    (out, lost)
}

/// The records of lists, each once, in the order first seen.
pub fn unique(lists: Vec<Vec<Vec<u8>>>) -> Vec<Vec<u8>> {
    let mut seen = HashSet::new();
    lists.into_iter().flatten().filter(|r| seen.insert(r.clone())).collect()
}

/// A list's bytes, unpacked, as the game reads them: version 2 when any record is (the version 1 ones converted).
/// (The bytes, how many version 1 records could not be converted.)
pub fn pack(recs: Vec<Vec<u8>>) -> (Vec<u8>, usize) {
    let (mut lists, lost) = as_v2(vec![recs], false);
    let recs = lists.pop().unwrap_or_default();
    let (magic, size) = if recs.first().is_some_and(|r| r.len() == SIZE2) { (MAGIC2, SIZE2) } else { (MAGIC1, SIZE1) };
    let mut out = Vec::with_capacity(12 + size * recs.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&(size as u32).to_le_bytes());
    for r in recs {
        out.extend_from_slice(&r);
    }
    (out, lost)
}

/// What records are: "N pipelines (compute a, graphics b, mesh c) of P programs, version v".
pub fn describe<R: AsRef<[u8]>>(recs: &[R]) -> String {
    use std::collections::BTreeMap;
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut programs = HashSet::new();
    let mut versions = std::collections::BTreeSet::new();
    for r in recs.iter().map(AsRef::as_ref) {
        versions.insert(if r.len() == SIZE1 { 1 } else { 2 });
        let compute = kind(r) == COMPUTE;
        let mesh = u32_at(r, 40);
        programs.insert((r[..32].to_vec(), mesh, compute));
        *kinds
            .entry(if compute {
                "compute"
            } else if mesh != 0 {
                "mesh"
            } else {
                "graphics"
            })
            .or_default() += 1;
    }
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
    let versions: String = versions.iter().map(|v| format!(", version {v}")).collect();
    format!("{} pipelines ({}) of {} programs{versions}", recs.len(), kinds.join(", "), programs.len())
}

fn good_name(name: &str) -> bool {
    // (lower-case letters and digits, in runs joined by single '-', '_' or '.': the same order on every file system)
    !name.is_empty()
        && name
            .split(['-', '_', '.'])
            .all(|run| !run.is_empty() && run.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// The sources of the shipped list in merge order: the tour's (when there is one), then the contributed lists by
/// name. Refused: a file in `contrib/` that is not a list with a name the order can rely on.
pub fn sources(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if dir.join(TOUR).is_file() {
        out.push(dir.join(TOUR));
    }
    let c = dir.join(CONTRIB);
    let mut names = Vec::new();
    if c.is_dir() {
        for e in std::fs::read_dir(&c).at(&c)? {
            names.push(e.at(&c)?.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    for n in &names {
        if let Some(stem) = n.strip_suffix(".md")
            && names.contains(&format!("{stem}{EXT}"))
        {
            continue;
        }
        match n.strip_suffix(EXT) {
            Some(stem) if good_name(stem) => out.push(c.join(n)),
            _ => {
                return Err(Error::format(
                    c.join(n).display().to_string(),
                    format!(
                        "not a contributed list (contrib/ holds <name>{EXT} and <name>.md, the name in lower-case letters, \
                         digits, '-', '_' and '.')"
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// The shipped list's records: the sources', each pipeline once, in merge order. A source that does not read is left
/// out, with a line in `log` (a release's checks refuse it; a build ships the rest).
pub fn merged(dir: &Path, log: &mut dyn FnMut(String)) -> Result<Vec<Vec<u8>>> {
    let mut lists = Vec::new();
    for path in sources(dir)? {
        let what = path.display().to_string();
        let read = load(&path, None).and_then(|data| {
            let recs: Vec<Vec<u8>> = parse(&data, &what, true)?.into_iter().map(<[u8]>::to_vec).collect();
            Ok(recs)
        });
        match read {
            Ok(recs) => lists.push(recs),
            Err(e) => log(format!("left out of the shipped pipeline state list: {e}")),
        }
    }
    let (lists, lost) = as_v2(lists, false);
    if lost > 0 {
        log(format!("{lost} version 1 pipelines could not be converted to version 2: left out"));
    }
    Ok(unique(lists))
}

/// The game folder's shipped list from the sources in `dir`, written only when it changed. The number of pipelines
/// (0 and nothing written when there are no sources or none reads).
pub fn ship(dir: &Path, game: &Path, log: &mut dyn FnMut(String)) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let recs = merged(dir, log)?;
    if recs.is_empty() {
        return Ok(0);
    }
    let n = recs.len();
    let (data, _) = pack(recs);
    fsx::write_if_changed(&game.join(SHIPPED), &data)?;
    Ok(n)
}

/// The pipelines recorded on the development PC (`longshot_pipelines_seed.bin` in the tools: program keys and draw
/// states, no game code), as the game folder's list (`cache/longshot_pipelines.bin`, where the game keeps its notes),
/// so its first start builds them before the title. The file's first 8 bytes are the stamp of the build that made it:
/// a list the folder has from the same build is kept (it grows as the player plays). One loose beside the game goes
/// into `cache/` first, as the game's own start moves it. Whether it was written.
pub fn seed(seed: &Path, game: &Path) -> Result<bool> {
    seed_into(seed, &game.join("cache"), Some(game))
}

/// [`seed`] into the shader cache folder `cache` (the game folder's `cache/`, or the shared one), the loose list of
/// game folder `game` moved in first when it has one.
pub fn seed_into(seed: &Path, cache: &Path, game: Option<&Path>) -> Result<bool> {
    let Ok(data) = std::fs::read(seed) else { return Ok(false) };
    let dst = cache.join("longshot_pipelines.bin");
    std::fs::create_dir_all(cache).at(cache)?;
    let loose = game.map(|g| g.join("longshot_pipelines.bin"));
    if let Some(loose) = loose.filter(|l| l.is_file() && !dst.exists()) {
        let _ = std::fs::rename(&loose, &dst);
    }
    let stamp = &data[..data.len().min(8)];
    let mut head = [0u8; 8];
    let same = std::fs::File::open(&dst)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .is_ok_and(|n| &head[..n] == stamp);
    if same {
        return Ok(false);
    }
    fsx::write_atomic(&dst, &data)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1(code: u64, mesh: u32, topo: u32, colour: u32, answers: &[u32]) -> Vec<u8> {
        let mut r = vec![0u8; SIZE1];
        r[..8].copy_from_slice(&code.to_le_bytes());
        r[32..40].copy_from_slice(&99u64.to_le_bytes());
        for (at, v) in [(40, mesh), (44, topo), (48, 1), (52, 1), (64, colour), (388, answers.len() as u32)] {
            r[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, a) in answers.iter().enumerate() {
            r[396 + 4 * i..400 + 4 * i].copy_from_slice(&a.to_le_bytes());
        }
        r
    }

    #[test]
    fn version_1_converts_its_formats_and_the_mesh_fallback() {
        let plain = v1_to_v2(&v1(7, 0, 3, 37, &[])).unwrap();
        assert_eq!(plain.len(), SIZE2);
        assert_eq!(u32_at(&plain, 64), 0x6d);
        assert_eq!(u32_at(&plain, 96), 0x6d, "the view format is the same");
        assert_eq!(u64_at(&plain, 32), 99, "the variant kept");
        assert!(v1_to_v2(&v1(7, 0, 3, 12345, &[])).is_none(), "a format the table lacks");
        // (a fallback mesh record: stage 2 numbered 1 again, the variant made again, a triangle list)
        let fb = v1_to_v2(&v1(7, 1, 4, 37, &[0x2000_1001])).unwrap();
        assert_eq!(u32_at(&fb, 44), TRIANGLE_LIST);
        assert_eq!(u32_at(&fb, 416), 0x1000_1001);
        assert_eq!(u64_at(&fb, 32), answers_hash(&[0x1000_1001]));
        assert!(v1_to_v2(&v1(7, 1, 4, 37, &[])).is_none(), "a fallback record without its answers");
    }

    #[test]
    fn a_merge_keeps_each_record_once_and_packs_as_version_2() {
        let a = v1(1, 0, 3, 37, &[]);
        let c = compute_record(5);
        let (lists, lost) = as_v2(vec![vec![a.clone(), a.clone()], vec![c.clone(), v1(2, 0, 3, 1, &[])]], false);
        assert_eq!(lost, 1);
        let recs = unique(lists);
        assert_eq!(recs.len(), 2);
        let (bytes, _) = pack(recs);
        let back = parse(&bytes, "t", true).unwrap();
        assert_eq!(back.len(), 2);
        assert!(bytes.starts_with(MAGIC2));
        assert_eq!(compute_keys(&back), [5]);
        assert!(check_kinds(&back, "t").is_ok());
        let mut bad = c.clone();
        bad[100] = 1;
        assert!(check_kinds(&[bad], "t").is_err());
        assert_eq!(describe(&back), "2 pipelines (compute 1, graphics 1) of 2 programs, version 2");
        // (cut short: left out, or refused when strict)
        let cut = &bytes[..bytes.len() - 3];
        assert_eq!(parse(cut, "t", false).unwrap().len(), 1);
        assert!(parse(cut, "t", true).is_err());
        assert!(parse(b"HKSTATE9xxxx", "t", false).is_err());
    }

    #[test]
    fn a_seed_list_is_kept_while_its_build_is_the_same() {
        let d = std::env::temp_dir().join(format!("build-assets-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("seed.bin"), b"STAMP001records").unwrap();
        std::fs::write(d.join("longshot_pipelines.bin"), b"OLDSTAMPloose").unwrap();
        assert!(seed(&d.join("seed.bin"), &d).unwrap(), "a list of another build is replaced");
        assert!(!d.join("longshot_pipelines.bin").exists(), "the loose one moved first");
        std::fs::write(d.join("cache/longshot_pipelines.bin"), b"STAMP001records and more").unwrap();
        assert!(!seed(&d.join("seed.bin"), &d).unwrap(), "the same build's, grown: kept");
        assert!(!seed(&d.join("none.bin"), &d).unwrap());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_tour_s_list_first_each_pipeline_once_and_a_damaged_source_left_out() {
        let d = std::env::temp_dir().join(format!("build-assets-ship-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(CONTRIB)).unwrap();
        let xz = |keys: &[u64]| {
            let (bytes, _) = pack(keys.iter().map(|&k| compute_record(k)).collect());
            let mut out = Vec::new();
            lzma_rs::xz_compress(&mut &bytes[..], &mut out).unwrap();
            out
        };
        // (the contributed lists are merged by name, after the tour's whatever their names: "a" sorts before "tour")
        std::fs::write(d.join(TOUR), xz(&[10, 20])).unwrap();
        std::fs::write(d.join(CONTRIB).join("a.bin.xz"), xz(&[30, 10])).unwrap();
        std::fs::write(d.join(CONTRIB).join("a.md"), "- GPU: made up\n").unwrap();
        std::fs::write(d.join(CONTRIB).join("b.bin.xz"), b"\xfd7zXZ\x00 cut short").unwrap();
        std::fs::write(d.join(CONTRIB).join("c.bin.xz"), xz(&[40, 20, 50])).unwrap();
        let mut lines = Vec::new();
        let recs = merged(&d, &mut |l| lines.push(l)).unwrap();
        assert_eq!(compute_keys(&recs), [10, 20, 30, 40, 50], "the tour's first, then by name, each once");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("left out of the shipped pipeline state list: ") && lines[0].contains("b.bin.xz"));
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        assert_eq!(ship(&d, &game, &mut |_| {}).unwrap(), 5);
        let shipped = std::fs::read(game.join(SHIPPED)).unwrap();
        assert_eq!(compute_keys(&parse(&shipped, "shipped", true).unwrap()), [10, 20, 30, 40, 50]);
        // (no source reads: nothing shipped)
        std::fs::remove_dir_all(&game).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        let only_bad = d.join("bad");
        std::fs::create_dir_all(only_bad.join(CONTRIB)).unwrap();
        std::fs::write(only_bad.join(CONTRIB).join("b.bin.xz"), b"junk").unwrap();
        assert_eq!(ship(&only_bad, &game, &mut |_| {}).unwrap(), 0);
        assert!(!game.join(SHIPPED).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_the_order_can_rely_on() {
        assert!(good_name("ap1017-rtx-4090"));
        assert!(good_name("a.b_c"));
        assert!(!good_name("A"));
        assert!(!good_name("a--b"));
        assert!(!good_name("-a"));
    }
}
