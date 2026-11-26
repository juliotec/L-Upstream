//! The game's hook sites (docs/dev/reference/hooks.md), as the lifter reads and places them: the port of nx2/hooks.py's
//! reader, resolver and placement.
//!
//! - [`load`]: the definitions (`game/sites/sites.toml`) joined with the build's locations
//!   (`game/sites/<build id>/locations.toml`), every rule of the format checked before anything is placed;
//! - [`resolve()`]: each site's signature found exactly once in its module's code, and its guards there;
//! - [`locate()`]: each site found only with the lift's functions (another site's function, an anchor, a fingerprint);
//! - [`place()`]: each site checked against the lift and placed, as data: a [`Placed`] per site, the form's point
//!   and what its hook does, for the mid IR to turn into an edit.
//!
//! [`apply`] does all of it for an image and a plan, and checks the host's list (`game/sites.def`) for every
//! site's slot, every rule's value, and its `HOS_SITE_OUTCOME` lines against the sites that change the outcome on a
//! worker thread. Nothing here matches lifted text.

mod code;
mod host;
mod place;
mod resolve;
mod signature;
mod table;

pub use code::{Function, InsnKind, Lift, Space};
pub use host::{HostList, HostValue};
pub use place::{Hook, Placed, Placement, Point, place, summary};
pub use resolve::{Found, Location, locate, resolve, string_addresses};
pub use signature::{MAX_WORDS, MaskedWord, Signature};
pub use table::{
    Anchor, AnchorKind, Arg, ArgType, CodeSite, End, Fingerprint, Form, Guard, Kind, Part, RegClass, Returns, Rule,
    RuleUse, Shape, Site, TableError, Thread, check_site, function_address, ident, is_name, join, parse_definitions,
    parse_locations, parse_table,
};

use cfg::plan::Plan;
use image::Image;
use std::path::{Path, PathBuf};

/// Why the sites cannot be placed: the lift stops, naming every site at fault (a hook is never placed by guess).
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// A table that does not follow the format.
    #[error("hooks: {0}")]
    Table(#[from] TableError),
    /// A table's file could not be read.
    #[error("hooks: {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// What the host's list lacks, or a site the lifter cannot place yet.
    #[error("hooks: {0}")]
    Host(String),
    /// Sites whose signatures do not resolve exactly once in the module's code (a line each).
    #[error(
        "hooks: {} site(s) of the table do not resolve exactly once, so the build stops (a hook is never placed by \
         guess):\n{}\nThe signature is the code the hook expects; this game's code differs there. The ExeFS patch mods \
         are written into the code first: one that stops a site on its own is refused before the lift \
         (game-files's patch check, its kind \"refused\"), so here it is several together, or none: turn the code mods off \
         and build again to tell. Otherwise the table needs the site found again (docs/dev/reference/hooks.md)",
        .0.len(),
        .0.join("\n")
    )]
    Signatures(Vec<String>),
    /// Sites not found exactly once with the lift's functions (a line each).
    #[error(
        "hooks: {} site(s) of the table do not resolve exactly once in the lift, so the build stops (a hook is never \
         placed by guess):\n{}\nThe table needs the site found again (docs/dev/reference/hooks.md)",
        .0.len(),
        .0.join("\n")
    )]
    InTheLift(Vec<String>),
    /// Found sites that do not fit the lift (a line each).
    #[error("hooks: {} site(s) do not fit the lift:\n{}", .0.len(), .0.join("\n"))]
    DoNotFit(Vec<String>),
}

/// The site tables' folder (`game`): the definitions (`sites/sites.toml`), each build's locations
/// (`sites/<build id>/locations.toml`), the host's list (`sites.def`, which the game's C includes).
#[derive(Clone, Debug)]
pub struct Tables {
    dir: PathBuf,
}

impl Tables {
    /// The tables in `dir` (the repository's `game`).
    pub fn new(dir: impl Into<PathBuf>) -> Tables {
        Tables { dir: dir.into() }
    }

    /// The definitions: `sites/sites.toml`.
    pub fn definitions(&self) -> PathBuf {
        self.dir.join("sites").join("sites.toml")
    }

    /// A build's locations: `sites/<build id>/locations.toml`.
    pub fn locations(&self, build_id: &str) -> PathBuf {
        self.dir.join("sites").join(build_id).join("locations.toml")
    }

    /// The host's list: `sites.def`.
    pub fn host_list(&self) -> PathBuf {
        self.dir.join("sites.def")
    }

    /// `path` as messages name it: from the repository's root (`game/sites/sites.toml`).
    fn shown(&self, path: &Path) -> String {
        let root = self.dir.parent().unwrap_or(&self.dir);
        let rel = path.strip_prefix(root).unwrap_or(path);
        rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
    }

    fn read(&self, path: &Path) -> Result<String, ResolveError> {
        std::fs::read_to_string(path).map_err(|source| ResolveError::Io { path: path.into(), source })
    }

    /// The host's list, or `None` when there is no `sites.def`.
    pub fn host(&self) -> Result<Option<HostList>, ResolveError> {
        let path = self.host_list();
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(HostList::parse(&self.read(&path)?)))
    }
}

/// The build's sites, or `None` when the tables have no locations for it (an unsupported build: nothing is placed).
/// The id may come padded to 64 digits (an NSO header's). `engine`: the host's engine sites, for the rule on who may
/// change the outcome on a worker thread (`None`: no host list, the rule left out).
pub fn load(tables: &Tables, build_id: &str, host: Option<&HostList>) -> Result<Option<Vec<Site>>, ResolveError> {
    let id: String = build_id.to_ascii_lowercase().chars().take(40).collect();
    let locs_path = tables.locations(&id);
    if id.is_empty() || !locs_path.is_file() {
        return Ok(None);
    }
    let defs_path = tables.definitions();
    let (defs_shown, locs_shown) = (tables.shown(&defs_path), tables.shown(&locs_path));
    if !defs_path.is_file() {
        return Err(TableError {
            place: defs_shown,
            site: None,
            what: "the sites' definitions are missing (a build's locations.toml says only where each is)".into(),
        }
        .into());
    }
    let defs = parse_definitions(&tables.read(&defs_path)?, &defs_shown)?;
    let locs = parse_locations(&tables.read(&locs_path)?, &locs_shown, Some(&id))?;
    Ok(Some(join(&defs, &locs, &defs_shown, &locs_shown, host.map(|h| &h.engine))?))
}

/// The build's tables read, resolved and placed, for an image and the lift's functions. `None` when the tables have no
/// locations for the image's build. Each site needs its host line (a flag and a slot, or a rule's value of the rule's
/// type), or the game would not link.
pub fn apply(tables: &Tables, image: &Image, plan: &Plan) -> Result<Option<(Vec<Site>, Placement)>, ResolveError> {
    let build_id = image.module("main").map(|m| m.build_id.clone()).unwrap_or_default();
    let host = tables.host()?;
    let Some(sites) = load(tables, &build_id, host.as_ref())? else { return Ok(None) };
    let host = host.unwrap_or_default();
    let missing: Vec<&str> = sites
        .iter()
        .filter(|s| s.code().is_none_or(|c| c.rule.is_none()) && host.sites.get(&s.ident()) != Some(&s.name))
        .map(|s| s.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(ResolveError::Host(format!(
            "game/sites.def has no HOS_SITE line for {}: the game would not link",
            missing.join(", ")
        )));
    }
    let wrong: Vec<String> = sites
        .iter()
        .filter_map(|s| s.code()?.rule.as_ref().map(|r| (s, r)))
        .filter(|(_, r)| host.values.get(&r.value).is_none_or(|v| v.ctype != r.rule.host_type()))
        .map(|(s, r)| format!("{} ({})", s.name, r.value))
        .collect();
    if !wrong.is_empty() {
        return Err(ResolveError::Host(format!(
            "game/sites.def has no HOS_VALUE line of the rule's type for {}: the game would not link",
            wrong.join(", ")
        )));
    }
    // (the host refuses run-time callbacks on the sites its HOS_SITE_OUTCOME lines name: one missing would take one)
    let decides: std::collections::BTreeMap<String, String> =
        sites.iter().filter(|s| s.decides_on_worker()).map(|s| (s.ident(), s.name.clone())).collect();
    if host.outcome != decides {
        let not_in = |a: &std::collections::BTreeMap<String, String>,
                      b: &std::collections::BTreeMap<String, String>| {
            let mut n: Vec<&str> = a.iter().filter(|(i, n)| b.get(*i) != Some(n)).map(|(_, n)| n.as_str()).collect();
            n.sort_unstable();
            if n.is_empty() { "none".to_string() } else { n.join(", ") }
        };
        return Err(ResolveError::Host(format!(
            "game/sites.def's HOS_SITE_OUTCOME lines are not the table's sites that change the game's outcome on a \
             worker thread: missing {}; not such a site {}",
            not_in(&decides, &host.outcome),
            not_in(&host.outcome, &decides)
        )));
    }
    let later: Vec<&str> = sites.iter().filter(|s| s.code().is_none()).map(|s| s.name.as_str()).collect();
    if !later.is_empty() {
        return Err(ResolveError::Host(format!(
            "{}: vslot sites are part of the format but not implemented yet: take them out of the table for now",
            later.join(", ")
        )));
    }
    let lift = Lift::new(image, plan);
    let mut found = Found::new();
    let mut modules: Vec<&str> = sites.iter().map(|s| s.module.as_str()).collect();
    modules.sort_unstable();
    modules.dedup();
    for module in modules {
        let group: Vec<Site> = sites.iter().filter(|s| s.module == module).cloned().collect();
        let space = Space::of(image, module)
            .ok_or_else(|| ResolveError::Host(format!("the image has no module {module} with code")))?;
        found.extend(resolve(&group, &space)?);
        locate(&group, &lift, &mut found, &space)?;
    }
    let placement = place(&sites, &lift, &found, &host)?;
    Ok(Some((sites, placement)))
}

#[cfg(test)]
mod tests;
