//! The finish's assets, over the build-assets crate ([`build_assets::Finish`]), reading the game's files the way the
//! game reads them: through the romfs index the read stage staged beside them (the player's .nup), or the romfs
//! folder linked in the game folder (an unpacked dump).

use crate::stages::{AssetPlaces, Assets};
use build_assets::romfs::{Folder, Romfs};
use build_core::report::Reporter;
use build_core::{Error, Result};
use game_files::romfs::{INDEX_FILE, Index};

/// The romfs of a game built from a .nup: its index.
struct Indexed {
    index: Index,
    /// What every file's fingerprint starts with: the index's own header (it names the .nup by its size and its
    /// head's hash), so a file is known by the .nup it is in and its place there.
    identity: String,
}

impl Romfs for Indexed {
    fn files(&self) -> Result<Vec<String>> {
        Ok(self.index.entries().iter().map(|e| e.name.clone()).collect())
    }

    fn read(&self, rel: &str) -> Result<Vec<u8>> {
        self.index.read(rel)
    }

    fn fingerprint(&self, rel: &str) -> Result<String> {
        let e = self.index.find(rel).ok_or_else(|| Error::format("romfs", format!("no file {rel}")))?;
        Ok(format!("{} {} {} {} {}", self.identity, e.offset, e.size, e.stored, e.codec))
    }

    fn has(&self, rel: &str) -> bool {
        self.index.find(rel).is_some()
    }
}

/// What the assets are, by their place in the game folder (a file, or a folder of them): what a finish puts there from
/// the assets' folder ([`stages::AssetPlaces::into`]), and takes away when it is made no more.
pub const MADE: [&str; 5] =
    ["texts", "prompts", build_assets::census::FILE, build_assets::pipelines::SHIPPED, build_assets::icon::WINDOW_ICON];

/// The game's romfs: the staged index, else the game folder's linked folder.
fn open(at: &AssetPlaces<'_>) -> Result<Box<dyn Romfs>> {
    let idx = at.into.join(INDEX_FILE);
    if idx.is_file() {
        let head = std::fs::read(&idx).map_err(|e| Error::io(&idx, e))?;
        let identity = build_core::key::sha1_hex(&head[..head.len().min(0x40)]);
        return Ok(Box::new(Indexed { index: Index::open(&idx)?, identity }));
    }
    Ok(Box::new(Folder::new(&at.out.join("romfs"))?))
}

fn with<T>(at: &AssetPlaces<'_>, f: impl FnOnce(&build_assets::Finish<'_>) -> Result<T>) -> Result<T> {
    let romfs = open(at)?;
    let finish = build_assets::Finish::new(romfs.as_ref(), at.into, at.work, at.root, at.jobs);
    f(&finish)
}

/// The builder's assets ([`build_assets`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct FinishAssets;

impl Assets for FinishAssets {
    fn texts(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        with(at, |f| f.texts(r))
    }

    fn prompts(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        with(at, |f| f.prompts(r))
    }

    fn shader_list(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        with(at, |f| f.shader_list(r))
    }

    fn pipelines(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()> {
        // (no game file read: the tools' lists alone)
        let none = build_assets::romfs::Folder::empty();
        build_assets::Finish::new(&none, at.into, at.work, at.root, at.jobs).pipelines(r)
    }

    fn window_icon(&self, at: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        let (_, icon) = crate::read::title_of(at.into, at.out);
        let none = build_assets::romfs::Folder::empty();
        build_assets::Finish::new(&none, at.into, at.work, at.root, at.jobs).window_icon(icon.as_deref())
    }
}
