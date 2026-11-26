//! What a build is asked to do: the game files it reads, the game folder it writes, and the player's choices that
//! the game folder keeps (its language, its upscalers, what the shader preparation builds).

use build_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The player's copy of the game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameSource {
    /// A .nup package: the game reads its romfs from it in place (an index in the game folder), so it must stay where
    /// it is.
    Nup(PathBuf),
    /// An unpacked dump (a developer's): `romfs/` and `exefs/` folders.
    Folder {
        /// The romfs folder.
        romfs: PathBuf,
        /// The exefs folder.
        exefs: PathBuf,
    },
}

impl GameSource {
    /// `--game`'s value: a .nup file, or a folder with `romfs` and `exefs` in it; said plainly when it is neither,
    /// before any folder is made (a launcher's path, not a trace half way in).
    pub fn from_path(path: &Path) -> Result<GameSource> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if path.is_file() {
            return Ok(GameSource::Nup(path));
        }
        if path.is_dir() {
            let (romfs, exefs) = (path.join("romfs"), path.join("exefs"));
            if romfs.is_dir() && exefs.is_dir() {
                return Ok(GameSource::Folder { romfs, exefs });
            }
            return Err(Error::player(
                "game_folder_incomplete",
                format!("The folder {} needs both a romfs and an exefs folder in it.", path.display()),
            ));
        }
        Err(Error::player("game_not_found", format!("Your copy of the game was not found: {}", path.display())))
    }

    /// The .nup, when it is one.
    pub fn nup(&self) -> Option<&Path> {
        match self {
            GameSource::Nup(p) => Some(p),
            GameSource::Folder { .. } => None,
        }
    }
}

/// An upscaler built in on request, as it needs a vendor's files (XeSS needs none at build time: every Windows build
/// has it, and it runs once Intel's library is beside the game).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Upscaler {
    /// NVIDIA DLSS: built against NVIDIA's NGX SDK (`LONGSHOT_NGX_SDK`), under NVIDIA's license.
    Dlss,
    /// AMD FSR 4: AMD's MIT headers are in third_party/fidelityfx; AMD's runtime for Vulkan is put beside a Windows
    /// game (crate::runtimes), a Linux one loads the player's own.
    Fsr4,
    /// Intel XeSS: every Windows build has it (gpu/vk/vk_xess.cpp); asked for, Intel's library is put beside the game
    /// (crate::runtimes). Nothing on Linux, which Intel publishes no library for.
    Xess,
}

impl Upscaler {
    /// Its name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Upscaler::Dlss => "dlss",
            Upscaler::Fsr4 => "fsr4",
            Upscaler::Xess => "xess",
        }
    }

    /// `--upscaler`'s value: names separated by commas (`dlss,fsr4`), or `none`.
    pub fn parse_list(text: &str) -> Result<Vec<Upscaler>> {
        let mut out = Vec::new();
        for name in text.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            match name.to_ascii_lowercase().as_str() {
                "none" => {}
                "dlss" => out.push(Upscaler::Dlss),
                "fsr" | "fsr4" => out.push(Upscaler::Fsr4),
                "xess" => out.push(Upscaler::Xess),
                _ => {
                    return Err(Error::player(
                        "bad_upscaler",
                        format!("--upscaler: {name:?} is not one of dlss, fsr4, xess or none."),
                    ));
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// What the shader preparation builds (`compile_shaders=` in the game folder's `longshot_options.ini`, which the
/// game's own preparations at a start follow: gpu/vk/vk_draw.cpp compile_all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompileShaders {
    /// Every shader program and the pipelines the lists name, before the title.
    #[default]
    Most,
    /// The predicted pipelines too: about 40 % more time, for pipelines play almost never draws.
    All,
}

impl CompileShaders {
    /// Its name (`--compile-shaders`).
    pub fn name(self) -> &'static str {
        match self {
            CompileShaders::Most => "most",
            CompileShaders::All => "all",
        }
    }

    /// The number the game reads.
    pub fn number(self) -> u32 {
        match self {
            CompileShaders::Most => 0,
            CompileShaders::All => 1,
        }
    }

    /// From its name.
    pub fn parse(text: &str) -> Result<CompileShaders> {
        match text.trim() {
            "most" => Ok(CompileShaders::Most),
            "all" => Ok(CompileShaders::All),
            other => {
                Err(Error::player("bad_compile_shaders", format!("--compile-shaders: {other:?} is not most or all.")))
            }
        }
    }
}

/// The game's languages: the code language.txt holds, and its name (host/nn_applet.c reads the same codes).
pub const LANGUAGES: [(&str, &str); 16] = [
    ("en-US", "English (US)"),
    ("en-GB", "English (UK)"),
    ("fr", "French (France)"),
    ("fr-CA", "French (Canada)"),
    ("de", "German"),
    ("it", "Italian"),
    ("es", "Spanish (Spain)"),
    ("es-419", "Spanish (Latin America)"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("ru", "Russian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-Hans", "Chinese (Simplified)"),
    ("zh-Hant", "Chinese (Traditional)"),
];

/// A language code as the game knows it, matched without regard to case; an error naming the codes otherwise.
pub fn language_code(text: &str) -> Result<&'static str> {
    LANGUAGES.iter().map(|l| l.0).find(|c| c.eq_ignore_ascii_case(text.trim())).ok_or_else(|| {
        Error::player(
            "bad_language",
            format!(
                "--language: {text:?} is not one of the game's languages ({}).",
                LANGUAGES.iter().map(|l| l.0).collect::<Vec<_>>().join(", ")
            ),
        )
    })
}

/// The game language closest to a system locale (`de-DE`, `pt_BR.UTF-8`): the exact one, then the same family;
/// `None` when the game has none of it.
pub fn closest_language(locale: &str) -> Option<&'static str> {
    let name = locale.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let family = name.split('-').next().unwrap_or("").to_ascii_lowercase();
    if let Some((code, _)) = LANGUAGES.iter().find(|(c, _)| c.eq_ignore_ascii_case(&name)) {
        return Some(code);
    }
    match family.as_str() {
        "zh" => Some(if ["TW", "HK", "MO", "Hant"].iter().any(|t| name.contains(t)) { "zh-Hant" } else { "zh-Hans" }),
        "es" if !name.eq_ignore_ascii_case("es-es") => Some("es-419"),
        "pt" => Some("pt-BR"),
        _ => LANGUAGES.iter().map(|l| l.0).find(|c| *c == family),
    }
}

/// The system's language as gettext reads it (`LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE`'s first over them unless
/// the locale is C), as the game's closest one. (Windows: a launcher passes `--language`; the variables are rarely set.)
pub fn system_language(get: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let mut name = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| get(v).filter(|x| !x.is_empty()))?;
    let first = get("LANGUAGE").unwrap_or_default().split(':').next().unwrap_or("").to_string();
    if !first.is_empty() && !matches!(name.split(['.', '@']).next(), Some("C" | "POSIX")) {
        name = first;
    }
    closest_language(&name)
}

/// Which guest profile the lift uses (docs/dev/reference/builder.md, "The guest profile").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GuestProfile {
    /// The one the tree ships (`assets/profiles/guest.profile.xz`), if it ships one: what players build with.
    #[default]
    Shipped,
    /// This file instead (a developer's).
    File(PathBuf),
    /// None: an instrumented build, whose units count their blocks and branches for a new profile
    /// (`HOS_GUEST_PROFILE=<raw>` in a run, then `longshot-builder dev profile --map gen/profile_map.txt`).
    Generate,
}

impl GuestProfile {
    /// `--guest-profile`'s value: `generate`, or a profile's file.
    pub fn parse(v: &str) -> GuestProfile {
        if v == "generate" { GuestProfile::Generate } else { GuestProfile::File(PathBuf::from(v)) }
    }
}

/// A build.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// The player's copy.
    pub game: GameSource,
    /// The game folder.
    pub out: PathBuf,
    /// The work folder (`None`: the per-user cache's, one per game folder: [`crate::layout::default_work`]).
    pub work: Option<PathBuf>,
    /// The stores' folder (`None`: `LONGSHOT_STORE`, else the per-user cache's).
    pub store: Option<PathBuf>,
    /// The builder's source tree (`None`: found beside the program, or `LONGSHOT_ROOT`).
    pub root: Option<PathBuf>,
    /// Workers (`None`: `LONGSHOT_JOBS`, else one per CPU thread but one).
    pub jobs: Option<usize>,
    /// The game's language (`None`: the folder's, else the system's, else en-US).
    pub language: Option<String>,
    /// The upscalers built in.
    pub upscalers: Vec<Upscaler>,
    /// The vendors whose licenses the player accepted (`--accept-licenses`): their files may be downloaded
    /// (crate::runtimes).
    pub licenses: Vec<crate::runtimes::Vendor>,
    /// What the shader preparation builds (`None`: the folder's choice, else most).
    pub compile_shaders: Option<CompileShaders>,
    /// Prepare the shaders after the build (the built game, `--prepare-shaders`).
    pub prepare_shaders: bool,
    /// The profile whose mods the build takes (crate::play::options: play's, `build --profile`'s): its manifest mods,
    /// the Mod API's layer's to read, and the mods in the emulators' layout in its `mods/`, said (`mod_needs_import`).
    /// Not kept in the record: an update names it again.
    pub profile: Option<PathBuf>,
    /// A second log file, besides the work folder's.
    pub log: Option<PathBuf>,
    /// A rebuild of a folder built before (`longshot-builder update`): its language is kept, its times recorded as an
    /// update.
    pub update: bool,
    /// The guest profile (`--guest-profile`; a build's alone: an update lifts with the shipped one again).
    pub guest_profile: GuestProfile,
    /// The stamp the record keeps for play's fast check (crate::play; `None`: a build for no profile, which the next
    /// play updates again).
    pub stamp: Option<crate::folder::Stamp>,
}

impl BuildOptions {
    /// A build of `game` into `out`, everything else at its default.
    pub fn new(game: GameSource, out: impl Into<PathBuf>) -> BuildOptions {
        BuildOptions {
            game,
            out: out.into(),
            work: None,
            store: None,
            root: None,
            jobs: None,
            language: None,
            upscalers: Vec::new(),
            licenses: Vec::new(),
            compile_shaders: None,
            prepare_shaders: true,
            profile: None,
            log: None,
            update: false,
            guest_profile: GuestProfile::Shipped,
            stamp: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closest_language() {
        assert_eq!(closest_language("de_DE.UTF-8"), Some("de"));
        assert_eq!(closest_language("fr-CA"), Some("fr-CA"));
        assert_eq!(closest_language("zh_TW"), Some("zh-Hant"));
        assert_eq!(closest_language("zh_CN"), Some("zh-Hans"));
        assert_eq!(closest_language("es_MX"), Some("es-419"));
        assert_eq!(closest_language("es_ES"), Some("es"));
        assert_eq!(closest_language("pt_PT"), Some("pt-BR"));
        assert_eq!(closest_language("sv_SE"), None);
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(system_language(env(&[("LANG", "ja_JP.UTF-8")])), Some("ja"));
        assert_eq!(system_language(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "nl:en")])), Some("nl"));
        assert_eq!(system_language(env(&[("LANG", "C"), ("LANGUAGE", "nl")])), None);
        assert_eq!(language_code("EN-us").unwrap(), "en-US");
        assert!(language_code("xx").is_err());
    }

    #[test]
    fn upscalers_and_levels() {
        assert_eq!(Upscaler::parse_list("fsr4, dlss,dlss").unwrap(), [Upscaler::Dlss, Upscaler::Fsr4]);
        assert_eq!(Upscaler::parse_list("none").unwrap(), []);
        assert_eq!(Upscaler::parse_list("xess").unwrap(), [Upscaler::Xess]);
        assert!(Upscaler::parse_list("dlss3").is_err());
        assert_eq!(CompileShaders::parse("all").unwrap().number(), 1);
    }
}
