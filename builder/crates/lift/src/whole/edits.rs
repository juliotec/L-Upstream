//! The lift's edits: every change Longshot makes to the game's functions (the hook sites of game/'s tables, placed
//! by the `sites` crate; the supported build's patches; the host's entry hooks of the SDK's functions), planned by the
//! `edits` crate per function. The function builder writes each function's edits in as it builds it
//! ([`crate::Program::new`]), and each function's key holds its edits' key ([`Edits::key`]), so one site's or one
//! patch's change lifts again only the units of the functions it lands in.

use crate::whole::host::Host;
use build_core::{Error, Result};
use cfg::plan::Plan;

/// The edits of a lift, by function.
#[derive(Clone, Debug, Default)]
pub struct Edits {
    /// Each changed function's edits.
    pub functions: edits::Edits,
    /// The placed sites (their log lines among them).
    pub placement: Option<sites::Placement>,
    /// What was left out, and why (an entry hook whose symbol is no lifted function, the patches of another build).
    pub notes: Vec<String>,
}

impl Edits {
    /// The edits of this lift: the placement's sites, the host's entry hooks, the patches for the image's build.
    pub fn new(image: &image::Image, plan: &Plan, placement: Option<sites::Placement>, host: &Host) -> Result<Edits> {
        let hooks: Vec<edits::entry_hooks::EntryHook> = host
            .entry_hooks
            .iter()
            .map(|(symbol, &least)| edits::entry_hooks::EntryHook { symbol: symbol.clone(), least })
            .collect();
        let inputs = edits::Inputs { image, plan, placement: placement.as_ref(), entry_hooks: &hooks };
        let planned = edits::plan_edits(&inputs).map_err(|e| Error::format("the game's edits", e.to_string()))?;
        Ok(Edits { functions: planned.edits, placement, notes: planned.notes })
    }

    /// The key of the function at `entry`'s edits, for its key (empty: none).
    pub fn key(&self, entry: u64) -> String {
        self.functions.get(&entry).map_or_else(String::new, |f| f.key().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// A line per changed function: its entry, its edits' key, and what changes it.
    pub fn list_text(&self) -> String {
        let mut out = String::new();
        for (entry, f) in &self.functions {
            out.push_str(&format!("{entry:08x} {} {}\n", self.key(*entry), f.why.join("; ")));
        }
        out
    }
}
