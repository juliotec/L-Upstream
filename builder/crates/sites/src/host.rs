//! The host's list of sites, `game/sites.def`: a flag byte and a callback slot for each site (`HOS_SITE`, and
//! `HOS_SITE_ENGINE` for a site the engine's own feature uses, on from the start), the values the inline rules read
//! (`HOS_VALUE`), and the sites that change the game's outcome on a worker thread (`HOS_SITE_OUTCOME`, which the host
//! refuses a callback set at run time). A site with no line here would not link, so the lift stops first.

use std::collections::{BTreeMap, BTreeSet};

/// A value a data rule reads: its C identifier, C type and the variable that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostValue {
    /// The value's C identifier.
    pub ident: String,
    /// Its C type (`uint32_t`, `uint64_t`).
    pub ctype: String,
    /// The C variable the rule reads or writes.
    pub symbol: String,
}

/// What `sites.def` lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostList {
    /// Every site with a flag and a slot: its C identifier and its name.
    pub sites: BTreeMap<String, String>,
    /// The sites the engine's own features use (`HOS_SITE_ENGINE`), by name.
    pub engine: BTreeSet<String>,
    /// The rules' values, by name.
    pub values: BTreeMap<String, HostValue>,
    /// The sites that change the game's outcome on a worker thread (`HOS_SITE_OUTCOME`): their C identifier and name.
    pub outcome: BTreeMap<String, String>,
}

/// The arguments of a line that is a call of `name(...)` at the line's start, split at its commas and trimmed.
fn call<'a>(line: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let rest = line.strip_prefix(name)?.strip_prefix('(')?;
    let inner = &rest[..rest.find(')')?];
    Some(inner.split(',').map(str::trim).collect())
}

/// `"name"` as `name`.
fn quoted(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.is_empty() && !inner.contains('"')).then_some(inner)
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A site's line: `HOS_SITE(ident, "name")` or `HOS_SITE_ENGINE(ident, "name", ...)`: its identifier, its name, and
/// whether the engine's.
fn site_line(line: &str) -> Option<(&str, &str, bool)> {
    let (a, engine) = match call(line, "HOS_SITE_ENGINE") {
        Some(a) => (a, true),
        None => (call(line, "HOS_SITE")?, false),
    };
    let [ident, name, ..] = a[..] else { return None };
    let name = quoted(name).filter(|_| is_word(ident))?;
    Some((ident, name, engine))
}

impl HostList {
    /// Every site's line in `text`, in order (an identifier named twice is twice here): its identifier and its name.
    pub fn site_lines(text: &str) -> Vec<(String, String)> {
        text.lines().filter_map(site_line).map(|(i, n, _)| (i.into(), n.into())).collect()
    }

    /// The list in `text` (the lines that do not start with one of its macros are left out, as its comments).
    pub fn parse(text: &str) -> HostList {
        let mut out = HostList::default();
        for line in text.lines() {
            if let Some((ident, name, engine)) = site_line(line) {
                out.sites.insert(ident.into(), name.into());
                if engine {
                    out.engine.insert(name.into());
                }
            } else if let Some(a) = call(line, "HOS_SITE_OUTCOME")
                && let [ident, name] = &a[..]
                && let Some(name) = quoted(name).filter(|_| is_word(ident))
            {
                out.outcome.insert((*ident).into(), name.into());
            } else if let Some(a) = call(line, "HOS_VALUE")
                && let [ident, name, ctype, symbol] = &a[..]
                && let Some(name) = quoted(name).filter(|_| is_word(ident) && is_word(ctype) && is_word(symbol))
            {
                out.values.insert(
                    name.into(),
                    HostValue { ident: (*ident).into(), ctype: (*ctype).into(), symbol: (*symbol).into() },
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_kind_of_line() {
        let list = HostList::parse(
            "/* HOS_SITE(a_b, \"a.b\") in a comment */\n\
             HOS_SITE(test_step, \"test.step\")\n\
             HOS_SITE_ENGINE(test_engine, \"test.engine\", fn)\n\
             HOS_VALUE(frame_seconds, \"frame.seconds\", uint32_t, hos_frame_seconds_bits)\n\
             HOS_SITE_OUTCOME(test_engine, \"test.engine\")\n",
        );
        assert_eq!(list.sites.len(), 2);
        assert_eq!(list.outcome, [("test_engine".to_string(), "test.engine".to_string())].into());
        assert_eq!(list.sites["test_step"], "test.step");
        assert_eq!(list.engine, ["test.engine".to_string()].into());
        let v = &list.values["frame.seconds"];
        assert_eq!(
            (v.ident.as_str(), v.ctype.as_str(), v.symbol.as_str()),
            ("frame_seconds", "uint32_t", "hos_frame_seconds_bits")
        );
    }
}
