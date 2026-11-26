//! The finish stage's files: what the game folder holds beside the compiled game, made from the player's romfs and
//! the repository's assets.
//!
//! - [`texts`]: the text that names the console, reworded for a PC in every language (`texts/`);
//! - [`prompts`]: the button prompts of other controllers: the icon font, the textures, the layouts' places
//!   (`prompts/`);
//! - [`census`]: every shader program of the game, for the first start's precompile (`longshot_shaders.bin`);
//! - [`pipelines`]: the pipeline lists a release ships (`longshot_pipeline_states_shipped.bin`,
//!   `cache/longshot_pipelines.bin`);
//! - [`icon`]: the window's icon (`longshot.png`) and the name Windows shows for the exe (a `.res`);
//! - [`share`]: the player's pipeline list zipped for the "Submit your pipelines" issue (`longshot-builder share`).
//!
//! Each output is cached by exactly what it is made from ([`cache`]): one changed text archive remakes that language,
//! one face's hand-drawn picture that face's textures. [`Finish`] is what the driver calls; the modules are the
//! pieces, each with its own tests.

pub mod cache;
pub mod census;
pub mod draw;
pub mod font;
pub mod icon;
pub mod pipelines;
pub mod prompts;
pub mod raster;
pub mod romfs;
pub mod sarc;
pub mod share;
pub mod texts;
pub mod texture;
pub mod zs;

use build_core::key::Key;
use build_core::report::Reporter;
use build_core::{Result, fsx};
use cache::Cache;
use romfs::Romfs;
use std::path::{Path, PathBuf};

/// The recipe of the shader list ([`cache`]).
const CENSUS_RECIPE: &str = "census 1";

/// What the finish stage works with.
pub struct Finish<'a> {
    /// The game's files.
    pub romfs: &'a dyn Romfs,
    /// The game folder.
    pub game: PathBuf,
    /// The tools' root: `third_party/kenney_input*`, `assets/`, the seed pipeline list.
    pub root: PathBuf,
    /// Workers a step may run.
    pub jobs: usize,
    cache: Cache,
}

impl<'a> Finish<'a> {
    /// The finish of the game folder `game` from `romfs`, its cache in `work`'s `assets/` folder.
    pub fn new(romfs: &'a dyn Romfs, game: &Path, work: &Path, root: &Path, jobs: usize) -> Finish<'a> {
        Finish {
            romfs,
            game: game.to_path_buf(),
            root: root.to_path_buf(),
            jobs: jobs.max(1),
            cache: Cache::open(&work.join("assets")),
        }
    }

    /// The reworded texts (`texts/`).
    pub fn texts(&self, r: &Reporter) -> Result<()> {
        let made = texts::make(self.romfs, &self.game.join("texts"), &self.cache, r)?;
        r.log(format!("texts: {} languages reworded for a PC", made.len()));
        Ok(())
    }

    /// The button prompts (`prompts/`).
    pub fn prompts(&self, r: &Reporter) -> Result<()> {
        let sources = prompts::Sources::in_checkout(&self.root);
        let options = prompts::Options { jobs: self.jobs, ..Default::default() };
        let made = prompts::make(self.romfs, &sources, &self.game.join("prompts"), &self.cache, &options, r)?;
        r.log(format!("prompts: {} parts made, {} kept", made.made.len(), made.kept.len()));
        Ok(())
    }

    /// The shader list (`longshot_shaders.bin`), made again only when the files it reads changed.
    pub fn shader_list(&self, r: &Reporter) -> Result<()> {
        let mut key = Key::new();
        key.text("recipe", CENSUS_RECIPE);
        self.cache.key_romfs(&mut key, self.romfs, zs::DICTIONARY_PACK)?;
        for rel in census::archives(self.romfs)? {
            self.cache.key_romfs(&mut key, self.romfs, &rel)?;
        }
        let out = self.game.join(census::FILE);
        if self.cache.fresh("census", &key) {
            r.log("the shader list is the one made from the same files: kept");
            return Ok(());
        }
        self.cache.forget("census")?;
        let c = census::census(self.romfs, self.jobs, &|_| {})?;
        for s in &c.skipped {
            r.log(format!("shader list: {s}"));
        }
        fsx::write_atomic(&out, &c.bytes())?;
        self.cache.record("census", &key, std::slice::from_ref(&out))?;
        r.log(format!("the shader list: {} programs from {} stages", c.programs.len(), c.stages.len()));
        Ok(())
    }

    /// The merged pipeline state list shipped with the tools (`longshot_pipeline_states_shipped.bin`). The seed list
    /// goes in the game's own cache, not with the assets: [`seed_pipelines`].
    pub fn pipelines(&self, r: &Reporter) -> Result<()> {
        let n = pipelines::ship(&self.root.join("assets/pipeline_states"), &self.game, &mut |l| r.log(l))?;
        if n > 0 {
            r.log(format!("the shipped pipeline state list: {n} pipelines"));
        }
        Ok(())
    }

    /// The window's icon (`longshot.png`): the game's own picture (`icon`, its JPEG), else Longshot's.
    pub fn window_icon(&self, icon: Option<&[u8]>) -> Result<()> {
        let fallback = std::fs::read(self.root.join("assets/longshot.png")).ok();
        if let Some(png) = icon::window_icon(icon, fallback.as_deref()) {
            fsx::write_if_changed(&self.game.join(icon::WINDOW_ICON), &png)?;
        }
        Ok(())
    }
}

/// The seed pipeline list (`<game>/cache/longshot_pipelines.bin`, the game's own cache, which the game adds to)
/// placed from the tools' `longshot_pipelines_seed.bin` when it is another: once the game folder has the new build.
pub fn seed_pipelines(root: &Path, game: &Path, r: &Reporter) -> Result<()> {
    seed_pipelines_in(root, &game.join("cache"), Some(game), r)
}

/// [`seed_pipelines`] into the shader cache folder `cache` (a game folder's `cache/`, or the shared one the game is
/// told of: gpu/vk/vk_paths.h cache_path); `game`: the game folder whose loose list (a build before `cache/`) moves in
/// first.
pub fn seed_pipelines_in(root: &Path, cache: &Path, game: Option<&Path>, r: &Reporter) -> Result<()> {
    if pipelines::seed_into(&root.join("longshot_pipelines_seed.bin"), cache, game)? {
        r.log("the seed pipeline list: placed, built before the first title");
    }
    Ok(())
}

/// The name Windows shows for the exe, as a `.res` at `path`: the game's `title`; without one the file is removed
/// (the exe is named Longshot). Whether it was written.
pub fn title_res(path: &Path, title: Option<&str>) -> Result<bool> {
    match title {
        Some(t) => fsx::write_if_changed(path, &icon::title_res(t)),
        None => fsx::remove_file(path).map(|()| false),
    }
}
