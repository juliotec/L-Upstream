//! The stages the driver calls, each behind one seam, so a stage still being written is one change to bring in.
//!
//! - [`GameFiles`]: the read stage: the dump's checks, the exefs, the mods' ExeFS patches, the image, the romfs placed
//!   for the game, the disk ([`crate::read::ReadStage`], over the game-files and image crates).
//! - [`Assets`]: the finish's game-derived files (the texts, the button prompts, the shader list, the pipeline lists,
//!   the window's icon: [`crate::assets::FinishAssets`], over the build-assets crate). Each is optional in the game
//!   (it shows its own without), so a failure is a warning.
//! - [`lift`] and [`compile`]: the lift (`lift::whole::run`) and the compile (`build_compile::run`), each called from
//!   one function, as the [`Lift`] and [`Compile`] the driver is given ([`Stages::real`]; a test gives its own).

use crate::options::GameSource;
use build_core::report::Reporter;
use build_core::{Error, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A game check's answer (`longshot-builder check`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// It is the build Longshot supports.
    pub ok: bool,
    /// main's build id, short.
    pub build: String,
    /// The sentence for the player.
    pub text: String,
}

/// What the read stage found of the game's code.
#[derive(Clone, Debug, Default)]
pub struct GameRead {
    /// main's build id.
    pub build_id: String,
    /// The game's own name, from its .nup (kept in the game folder for the window and the exe's name).
    pub title: Option<String>,
}

/// The mods' ExeFS patches this build takes, in order.
#[derive(Clone, Debug, Default)]
pub struct Patches {
    /// The patch files, as the image stage reads them back (game-files' `patches::to_json`).
    pub files: Vec<u8>,
    /// The list the game folder keeps (`exefs_patches.txt`: game/mods.c says which mods wait for a build).
    pub list: String,
    /// How many files apply (code or data); 0: the image is the game's own.
    pub applied: usize,
}

/// How the romfs was placed for the game.
#[derive(Clone, Debug, Default)]
pub struct RomfsPlaced {
    /// Its files.
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
    /// Damaged files replaced by stand-ins (an incomplete dump): their names.
    pub stood_in: Vec<String>,
    /// What the swap does with a `romfs` folder in the game folder.
    pub romfs_dir: RomfsDir,
    /// The builder's own files of the game folder (by name) this build has none of: removed at the swap (a damaged
    /// list, a dump's build's index).
    pub remove: Vec<String>,
}

/// A `romfs` folder in the game folder of a build from a .nup, which the game would read before the .nup's index
/// (host/hos_main.c: a stale dump's folder wins).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RomfsDir {
    /// None there.
    #[default]
    None,
    /// The builder's (a build from a dump linked or copied it in): removed at the swap.
    Remove,
    /// Not the builder's: the player's own, left as it is and said.
    Theirs,
}

/// What the disk under a folder can tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    /// The bytes free for this user.
    pub free: Option<u64>,
    /// On a hard drive (`None`: cannot be told).
    pub spinning: Option<bool>,
}

/// The read stage.
pub trait GameFiles: Sync {
    /// Whether the game is the supported build, from its main module's header alone (nothing unpacked).
    fn check(&self, game: &GameSource) -> Result<Check>;
    /// The disk under `folder` (or the nearest folder above it that exists).
    fn disk(&self, folder: &Path) -> Disk;
    /// The game folder `out` takes new files (made if missing), said in the player's words where it does not: a folder
    /// under Program Files does only for administrators, and the build would otherwise stop minutes later.
    fn check_writable(&self, out: &Path) -> Result<()>;
    /// The game's code modules into `exefs` (made again from nothing), checked: every module one, the supported build.
    /// The game's own name and icon kept in `staging` (the finish's folder, laid out as the game folder).
    fn read_exefs(&self, game: &GameSource, exefs: &Path, staging: &Path, r: &Reporter) -> Result<GameRead>;
    /// The mods' ExeFS patches for this build: `profile`'s manifest mods' (the Mod API's layer gives them), each file's
    /// kind, those that would stop a hook site (`tables`) refused; a mod in the emulators' layout in its `mods/` said
    /// (`mod_needs_import`), never read.
    fn patches(&self, exefs: &Path, profile: Option<&Path>, tables: &Path, r: &Reporter) -> Result<Patches>;
    /// The image written into `image` (`image.bin`, `image.json`, each only when it changed): the modules laid out,
    /// patched, relocated and bound; `hle`, the symbols the host implements.
    fn image(&self, exefs: &Path, patches: &Patches, hle: &BTreeSet<String>, image: &Path, r: &Reporter) -> Result<()>;
    /// The romfs placed for the game in game folder `out`: the .nup's index, its note and the damaged files' list
    /// written in `staging` (the finish's folder: they go in at the swap), or an unpacked dump's folder linked into
    /// `out` (26 GB is not staged). What the swap removes is said in the answer. Runs beside the lift: it logs, and
    /// sends no progress.
    fn romfs(&self, game: &GameSource, out: &Path, staging: &Path, r: &Reporter) -> Result<RomfsPlaced>;
}

/// Where the finish's assets go, and what they may use.
#[derive(Clone, Copy, Debug)]
pub struct AssetPlaces<'a> {
    /// The game folder: its romfs folder (a dump's, linked) and, before the first build that stages it, its title.
    pub out: &'a Path,
    /// Where the assets are written (the work folder's `finish/`), laid out as the game folder: the finish puts them
    /// in with the build's other files, all at once, so a build that fails before leaves the old build's whole.
    pub into: &'a Path,
    /// The work folder: the assets' cache (`assets/`).
    pub work: &'a Path,
    /// The tools' root: Kenney's fonts, `assets/`, the seed pipeline list.
    pub root: &'a Path,
    /// Workers a step may run.
    pub jobs: usize,
}

/// The finish's game-derived files. Each writes into the game folder, reading the game's files through the romfs as
/// the game reads them (its index, or its linked folder), and keeps what it made while its inputs stay the same.
pub trait Assets: Sync {
    /// The texts that name the console, reworded for a PC in every language (`out/texts`, game/texts.c).
    fn texts(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()>;
    /// The button prompts of other controllers, the game's icon font redrawn (`out/prompts`, host/prompts.c).
    fn prompts(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()>;
    /// Every shader of the game, for the shader preparation (`out/longshot_shaders.bin`, gpu/vk/vk_shader.cpp).
    fn shader_list(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()>;
    /// The pipeline lists shipped with the tools (`out/cache/longshot_pipelines.bin`,
    /// `out/longshot_pipeline_states_shipped.bin`): hashes and states, no game code.
    fn pipelines(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()>;
    /// The window's icon (`out/longshot.png`, Linux): the game's own (kept by the read stage), else Longshot's.
    fn window_icon(&self, at: &AssetPlaces<'_>, r: &Reporter) -> Result<()>;
}

fn pending(what: &str, missing: &str) -> Error {
    Error::player("stage_not_ready", format!("This builder cannot {what} yet: {missing} is not in it."))
}

/// Assets that are never made (a test's stand-in): each says so, as a warning of the build.
#[derive(Clone, Copy, Debug, Default)]
pub struct PendingAssets;

impl Assets for PendingAssets {
    fn texts(&self, _: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        Err(pending("reword the game's texts", "the assets stage"))
    }
    fn prompts(&self, _: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        Err(pending("draw the button prompts", "the assets stage"))
    }
    fn shader_list(&self, _: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        Err(pending("list the game's shaders", "the assets stage"))
    }
    fn pipelines(&self, _: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        Err(pending("ship the pipeline lists", "the assets stage"))
    }
    fn window_icon(&self, _: &AssetPlaces<'_>, _: &Reporter) -> Result<()> {
        Err(pending("make the window's icon", "the assets stage"))
    }
}

/// What a lift is asked: the driver's view of `lift::whole::LiftOptions`.
#[derive(Clone, Debug)]
pub struct LiftAsk {
    /// The image's folder.
    pub image: PathBuf,
    /// gen/.
    pub gen_dir: PathBuf,
    /// The tree.
    pub root: PathBuf,
    /// `main.npdm` (the boot constants).
    pub npdm: PathBuf,
    /// What the lift does with the guest profile.
    pub profiling: lift::whole::Profiling,
    /// Workers.
    pub jobs: usize,
    /// The lifts' store.
    pub store: Option<PathBuf>,
}

/// What a lift did, as the driver needs it.
#[derive(Clone, Debug, Default)]
pub struct Lifted {
    /// Functions built with their edits (hook sites, entry hooks, text patches).
    pub edited: usize,
    /// The lift's key.
    pub key: String,
    /// Units in all, and lifted now.
    pub units: (usize, usize),
    /// The lift was skipped or taken from the store.
    pub reused: bool,
    /// Where the compile gets the units' texts the lift did not write (none: it wrote them all).
    pub texts: Option<Arc<LiftTexts>>,
}

impl Lifted {
    /// The compile's source of the units' texts.
    pub fn ir_source(&self) -> Option<build_compile::IrSource> {
        self.texts.clone().map(|t| build_compile::IrSource(t as Arc<dyn build_compile::IrText>))
    }

    /// The units whose texts were made noted in gen/'s `units.txt` (after the compile, whatever came of it).
    pub fn note_texts(&self, r: &Reporter) {
        if let Some(t) = &self.texts
            && let Err(e) = t.0.finish()
        {
            r.log(format!("lift: the units' list not updated ({e}): the next build makes those units' texts again"));
        }
    }
}

/// The lift's units' texts, made for the compile ([`lift::whole::Texts`]).
pub struct LiftTexts(pub lift::whole::Texts);

impl std::fmt::Debug for LiftTexts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiftTexts")
    }
}

impl build_compile::IrText for LiftTexts {
    fn text(&self, name: &str) -> Result<String> {
        self.0.text(name)
    }
}

/// The lift stage.
pub trait Lift: Sync {
    /// The lift of `ask`.
    fn lift(&self, ask: &LiftAsk, r: &Reporter) -> Result<Lifted>;
}

/// The compile stage.
pub trait Compile: Sync {
    /// The compile of `o`.
    fn compile(&self, o: &build_compile::Options, r: &Reporter) -> Result<build_compile::Outcome>;
}

/// The guest profile the tree ships, if it does (assets/profiles/; addresses and counts, no game code).
pub const GUEST_PROFILE: &str = "assets/profiles/guest.profile.xz";

/// What the lift does with the guest profile `g` asks for: the tree's (when it ships one) or a file's counts used (its
/// hottest functions, 95 % of the counted runs and at most 4,000, in hot units), or counters written in.
pub fn profiling(root: &Path, g: &crate::GuestProfile) -> lift::whole::Profiling {
    let used = |path: PathBuf| lift::whole::Profiling::Use { path, share: 0.95, max: 4000 };
    match g {
        crate::GuestProfile::Shipped => {
            Some(root.join(GUEST_PROFILE)).filter(|f| f.is_file()).map_or(lift::whole::Profiling::Off, used)
        }
        crate::GuestProfile::File(f) => used(f.clone()),
        crate::GuestProfile::Generate => lift::whole::Profiling::Generate,
    }
}

/// The lift's options for `ask`.
pub fn lift_options(ask: &LiftAsk) -> lift::whole::LiftOptions {
    let mut o = lift::whole::LiftOptions::new(&ask.image, &ask.gen_dir);
    o.host = Some(ask.root.join("host"));
    o.tables = Some(ask.root.join("game"));
    o.npdm = Some(ask.npdm.clone());
    o.jobs = ask.jobs.max(1);
    o.store = ask.store.clone();
    o.profiling = ask.profiling.clone();
    // (the units' texts, 5.7 GB on the game, are made as the compile needs them and not kept; LONGSHOT_KEEP_IR=1 keeps
    // them in gen/ to read or compare)
    o.keep_text = crate::env("LONGSHOT_KEEP_IR").is_some_and(|v| v == "1");
    o
}

/// The texts of the lift `ask` asks for, made when the compile needs one (`session`: the lift's, when it ran now).
pub fn lift_texts(ask: &LiftAsk, session: Option<Arc<lift::whole::Session>>) -> Option<Arc<LiftTexts>> {
    let o = lift_options(ask);
    (!o.keeps_text()).then(|| Arc::new(LiftTexts(lift::whole::Texts::new(o, session))))
}

/// The lift (`lift::whole::run`): the one place the driver calls it.
pub fn lift(ask: &LiftAsk, r: &Reporter) -> Result<Lifted> {
    let (out, session) = lift::whole::run_with(&lift_options(ask), r)?;
    let reused = !matches!(out.how, lift::whole::How::Lifted);
    Ok(Lifted {
        units: (out.units.len(), out.units_lifted),
        key: out.key,
        reused,
        edited: out.edited,
        texts: lift_texts(ask, session),
    })
}

/// The compile (`build_compile::run`): the one place the driver calls it.
pub fn compile(o: &build_compile::Options, r: &Reporter) -> Result<build_compile::Outcome> {
    build_compile::run(o, r)
}

/// The builder's lift ([`lift`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct LiftStage;
impl Lift for LiftStage {
    fn lift(&self, ask: &LiftAsk, r: &Reporter) -> Result<Lifted> {
        lift(ask, r)
    }
}

/// The builder's compile ([`compile`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileStage;
impl Compile for CompileStage {
    fn compile(&self, o: &build_compile::Options, r: &Reporter) -> Result<build_compile::Outcome> {
        compile(o, r)
    }
}

/// The stages a build runs.
pub struct Stages<'a> {
    /// The read stage.
    pub game: &'a dyn GameFiles,
    /// The lift.
    pub lift: &'a dyn Lift,
    /// The compile.
    pub compile: &'a dyn Compile,
    /// The finish's assets.
    pub assets: &'a dyn Assets,
}

impl Stages<'static> {
    /// The builder's own stages.
    pub fn real() -> Stages<'static> {
        Stages {
            game: &crate::read::ReadStage,
            lift: &LiftStage,
            compile: &CompileStage,
            assets: &crate::assets::FinishAssets,
        }
    }
}
