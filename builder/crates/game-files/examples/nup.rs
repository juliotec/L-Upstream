//! A .nup listed, verified, its exefs extracted or its romfs indexed (a developer's look at one, its counts and sizes).
//!
//! ```text
//! cargo run --release --example nup -- <file.nup>                                   list the titles
//! cargo run --release --example nup -- <file.nup> verify [exefs|romfs]              check hashes and extents
//! cargo run --release --example nup -- <file.nup> extract <dir> [exefs|romfs]       unpack (the newest title)
//! cargo run --release --example nup -- <file.nup> index <game dir>                  index the romfs in place
//! cargo run --release --example nup -- <file.nup> check                             the supported-build check
//! ```
//! Only counts and sizes are printed: nothing of the game.

use game_files::nup::{self, Nup, Part};
use std::path::PathBuf;

fn part(a: Option<String>) -> Option<Part> {
    match a.as_deref() {
        Some("exefs") => Some(Part::Exefs),
        Some("romfs") => Some(Part::Romfs),
        _ => None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("a .nup")?);
    let t0 = std::time::Instant::now();
    let nup = Nup::open(&path)?;
    match args.next().as_deref() {
        None => {
            println!("{:.2} GB, {} titles, {} files", nup.len as f64 / 1e9, nup.titles.len(), nup.files.len());
            for t in &nup.titles {
                let (ex, ro) = (nup.part(t, Part::Exefs), nup.part(t, Part::Romfs));
                let bytes: u64 = ro.iter().map(|(_, i)| nup.files[*i].size).sum();
                let packed: Vec<u64> =
                    ro.iter().filter(|(_, i)| nup.files[*i].codec == 1).map(|(_, i)| nup.files[*i].size).collect();
                let largest = packed.iter().max().copied().unwrap_or(0);
                println!(
                    "title {} v{}: exefs {} files, romfs {} files, {:.2} GB; {} zstd-stored, {:.2} GB of files, largest {:.1} MB",
                    t.index,
                    t.version,
                    ex.len(),
                    ro.len(),
                    bytes as f64 / 1e9,
                    packed.len(),
                    packed.iter().sum::<u64>() as f64 / 1e9,
                    largest as f64 / 1e6
                );
            }
        }
        Some("verify") => {
            let v = nup::verify(&nup, None, part(args.next()), &mut |_, _| {})?;
            for b in &v.bad {
                println!("{b}");
            }
            println!("verified {} records, {} extents: {} bad", v.checked, v.extents, v.bad.len());
        }
        Some("extract") => {
            let out = PathBuf::from(args.next().ok_or("a folder")?);
            let only = part(args.next());
            let t = nup.pick_title(None)?.clone();
            let got = nup::extract(&nup, &t, &out, only, &mut |_| {})?;
            if only != Some(Part::Romfs) {
                for (m, from) in nup::heal(&nup, &t, &out)? {
                    println!("{m}: taken from title {from}");
                }
                for m in game_files::nso::MODULE_ORDER {
                    let p = out.join("exefs").join(m);
                    if p.exists() && game_files::nso::unpack_in_place(&p)? == game_files::nso::Unpacked::Plain {
                        println!("{m}: ZBIC -> plain NSO");
                    }
                }
            }
            println!("{} files, {:.2} GB written, {} damaged", got.files, got.bytes as f64 / 1e9, got.damaged.len());
        }
        Some("index") => {
            let game = PathBuf::from(args.next().ok_or("a folder")?);
            std::fs::create_dir_all(&game)?;
            let t = nup.pick_title(None)?.clone();
            let made = game_files::romfs::index(&nup, &t, &game, &mut |_| {})?;
            // (the builder's stand-ins against the host's table for a romfs folder, game/oot_damaged_files.c: the same?)
            let table =
                std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../game/oot_damaged_files.c"))?;
            for st in &made.stand_ins {
                let pair = format!("{{\"{}\", \"{}\"}}", st.damaged, st.with.as_deref().unwrap_or("-"));
                println!(
                    "stand-in: {}",
                    if table.contains(&pair) { "the host table's" } else { "NOT the host table's" }
                );
            }
            println!(
                "{} files, {:.2} GB indexed; {} damaged, {} with stand-ins",
                made.entries,
                made.bytes as f64 / 1e9,
                made.stand_ins.len(),
                made.stand_ins.iter().filter(|s| s.with.is_some()).count()
            );
        }
        Some("compare") => {
            // (the index's files against an unpacked romfs folder: every unpacked one and every 200th other)
            let idx = game_files::romfs::Index::open(
                &PathBuf::from(args.next().ok_or("the game folder")?).join(game_files::romfs::INDEX_FILE),
            )?;
            let folder = PathBuf::from(args.next().ok_or("a romfs folder")?);
            let every = if args.next().as_deref() == Some("all") { 1 } else { 200 };
            let (mut same, mut differ, mut missing, mut bytes) = (0, 0, 0, 0u64);
            for (k, e) in idx.entries().iter().enumerate() {
                if e.codec == 1 || k % every == 0 {
                    let a = idx.read(&e.name)?;
                    let Ok(b) = std::fs::read(folder.join(&e.name)) else {
                        missing += 1;
                        continue;
                    };
                    bytes += a.len() as u64;
                    if a == b { same += 1 } else { differ += 1 }
                }
            }
            println!(
                "{same} files the same, {differ} differ, {missing} not in the folder ({:.2} GB compared)",
                bytes as f64 / 1e9
            );
        }
        Some("damaged") => {
            let t = nup.pick_title(None)?.clone();
            let list = game_files::romfs::entries(&nup, &t, &mut |_, _| {})?;
            let bad = game_files::romfs::damaged(&nup, &list, &mut |_, _| {})?;
            let mut kinds = std::collections::BTreeMap::new();
            for b in &bad {
                let e = list.iter().find(|e| &e.name == b).ok_or("entry")?;
                let i = nup.part(&t, Part::Romfs).into_iter().find(|(n, _)| n == b).ok_or("record")?.1;
                let head = nup.head(i, 4)?;
                *kinds
                    .entry((e.codec, nup.files[i].codec, head.iter().map(|x| format!("{x:02x}")).collect::<String>()))
                    .or_insert(0) += 1;
            }
            println!("{} damaged: {kinds:?}", bad.len());
        }
        Some("bench") => {
            // (the zstd-stored files decoded in memory: ruzstd, and this crate's decoder; the frames written to `out` for
            // the zstd command: the 4 largest one by one, and every other one in one file of many frames)
            let out = PathBuf::from(args.next().ok_or("a folder")?);
            std::fs::create_dir_all(&out)?;
            let t = nup.pick_title(None)?.clone();
            let mut packed: Vec<(u64, usize)> = nup
                .part(&t, Part::Romfs)
                .into_iter()
                .filter(|(_, i)| nup.files[*i].codec == 1)
                .map(|(_, i)| (nup.files[i].size, i))
                .collect();
            packed.sort();
            let (mut frames, mut sizes) = (Vec::new(), Vec::new());
            for &(size, i) in &packed {
                let r = &nup.files[i];
                let mut b = vec![0u8; r.stored as usize];
                game_files::nup::read_exact(&nup, &mut b, r.offset)?;
                frames.push(b);
                sizes.push(size);
            }
            let stored: usize = frames.iter().map(Vec::len).sum();
            let total: u64 = sizes.iter().sum();
            for (label, range) in [("4 largest", frames.len() - 4..frames.len()), ("the rest", 0..frames.len() - 4)] {
                let t0 = std::time::Instant::now();
                for k in range.clone() {
                    let mut dec = ruzstd::decoding::StreamingDecoder::new(&frames[k][..])?;
                    let mut o = Vec::with_capacity(sizes[k] as usize);
                    std::io::Read::read_to_end(&mut dec, &mut o)?;
                }
                let a = t0.elapsed().as_secs_f64();
                let t1 = std::time::Instant::now();
                for k in range.clone() {
                    game_files::zstd::decompress(game_files::zstd::Dialect::Zstd, &frames[k], sizes[k] as usize)?;
                }
                let b = t1.elapsed().as_secs_f64();
                let bytes: u64 = range.clone().map(|k| sizes[k]).sum();
                println!(
                    "{label}: {} files, {:.0} MB: ruzstd {a:.2} s ({:.0} MB/s), own {b:.2} s ({:.0} MB/s)",
                    range.len(),
                    bytes as f64 / 1e6,
                    bytes as f64 / 1e6 / a,
                    bytes as f64 / 1e6 / b
                );
            }
            for (n, k) in (frames.len() - 4..frames.len()).enumerate() {
                std::fs::write(out.join(format!("large{n}.zst")), &frames[k])?;
            }
            std::fs::write(out.join("rest.zst"), frames[..frames.len() - 4].concat())?;
            println!("{} frames, {:.2} GB stored for {:.2} GB", frames.len(), stored as f64 / 1e9, total as f64 / 1e9);
        }
        Some("check") => {
            let (ok, build, _) = game_files::checks::check(Some(&path), None)?;
            println!("supported: {ok} (build {build})");
        }
        Some(other) => return Err(format!("unknown command {other}").into()),
    }
    println!("{:.1} s", t0.elapsed().as_secs_f64());
    Ok(())
}
