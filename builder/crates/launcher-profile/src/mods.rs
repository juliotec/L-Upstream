//! `profile.json`'s `"mods"` list as the game's channel may change it (`set_mods`): the order of its entries and each one's
//! `on`, and nothing else. Never an entry's `granted` (its permissions), another of its keys, or what trust has
//! accepted: the entries that are there are kept whole but for `on`, whatever the request carries. A mod that is not in
//! the profile can't be added here (adding one is an install).

use crate::Profile;
use crate::file::{json_string, members};
use serde_json::value::RawValue;

/// Why a change was refused: a stable `code` (`bad_args`, `not_found`) and the sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsError {
    /// Stable id.
    pub code: &'static str,
    /// Why.
    pub text: String,
}

fn refused(code: &'static str, text: impl Into<String>) -> ModsError {
    ModsError { code, text: text.into() }
}

/// `incoming` (a JSON array of objects, each a string `id` and a boolean `on`; every other key ignored) applied to the
/// profile's `"mods"`: its entries in `incoming`'s order, each its existing entry with `on` set. An existing entry
/// `incoming` leaves out stays, after the others, as it was.
///
/// # Errors
/// `bad_args` for anything that is not that shape (or an id twice), `not_found` for an id the profile does not have.
pub fn set_mods(profile: &mut Profile, incoming: &serde_json::Value) -> Result<(), ModsError> {
    let list = incoming.as_array().ok_or_else(|| refused("bad_args", "mods is a list"))?;
    let mut wanted: Vec<(&str, bool)> = Vec::new();
    for entry in list {
        let id = entry.get("id").and_then(|v| v.as_str());
        let on = entry.get("on").and_then(|v| v.as_bool());
        let (Some(id), Some(on)) = (id, on) else {
            return Err(refused("bad_args", "each mod is an object with a text id and a true or false on"));
        };
        if wanted.iter().any(|(w, _)| *w == id) {
            return Err(refused("bad_args", format!("{id} is in the list twice")));
        }
        wanted.push((id, on));
    }
    // (the entries now: each one's text, its id when it has one)
    let have: Vec<Box<RawValue>> = match profile.raw("mods") {
        Some(raw) => {
            serde_json::from_str(raw.get()).map_err(|_| refused("bad_args", "the profile's mods are no list"))?
        }
        None => Vec::new(),
    };
    let id_of = |e: &RawValue| -> Option<String> {
        let m = members(e.get()).ok()?;
        let (_, v) = m.iter().find(|(k, _)| k == "id")?;
        serde_json::from_str::<String>(v.get()).ok()
    };
    let ids: Vec<Option<String>> = have.iter().map(|e| id_of(e)).collect();
    let mut out: Vec<String> = Vec::new();
    let mut used = vec![false; have.len()];
    for (id, on) in &wanted {
        let at = ids
            .iter()
            .position(|h| h.as_deref() == Some(id))
            .ok_or_else(|| refused("not_found", format!("{id} is not a mod of this profile")))?;
        used[at] = true;
        let mut m = members(have[at].get()).map_err(|_| refused("bad_args", format!("{id}'s entry is no object")))?;
        let flag = RawValue::from_string(on.to_string()).map_err(|e| refused("bad_args", e.to_string()))?;
        match m.iter_mut().find(|(k, _)| k == "on") {
            Some((_, v)) => *v = flag,
            None => m.push(("on".into(), flag)),
        }
        out.push(object_text(&m));
    }
    for (at, entry) in have.iter().enumerate() {
        if !used[at] {
            out.push(entry.get().to_string());
        }
    }
    let text = format!("[{}]", out.join(", "));
    profile.put("mods", RawValue::from_string(text).map_err(|e| refused("bad_args", e.to_string()))?);
    Ok(())
}

fn object_text(m: &[(String, Box<RawValue>)]) -> String {
    let parts: Vec<String> = m.iter().map(|(k, v)| format!("{}: {}", json_string(k), v.get())).collect();
    format!("{{{}}}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ana() -> Profile {
        Profile::parse(
            r#"{"v":1,"mods":[{"id":"a","on":true,"granted":["network"],"x":{"k":1}},{"id":"b","on":false},{"id":"c","on":true}]}"#,
        )
        .unwrap()
    }

    fn mods_of(p: &Profile) -> serde_json::Value {
        serde_json::from_str(p.raw("mods").unwrap().get()).unwrap()
    }

    #[test]
    fn the_order_and_on_change_and_nothing_else() {
        let mut p = ana();
        // (a forged request: other keys, a granted of its own)
        set_mods(
            &mut p,
            &json!([{"id":"b","on":true,"granted":["everything"]}, {"id":"a","on":false,"granted":[],"x":0,"trust":true}]),
        )
        .unwrap();
        assert_eq!(
            mods_of(&p),
            json!([
                {"id":"b","on":true},
                {"id":"a","on":false,"granted":["network"],"x":{"k":1}},
                {"id":"c","on":true}
            ]),
            "b first and on, a second and off with its own granted; c, left out, stays last as it was"
        );
        // (the keys of an entry stay in their order)
        assert!(
            p.raw("mods").unwrap().get().contains(r#"{"id": "a", "on": false, "granted": ["network"], "x": {"k":1}}"#)
        );
    }

    #[test]
    fn an_unknown_mod_and_a_bad_list_are_refused_and_change_nothing() {
        let mut p = ana();
        let before = p.clone();
        for (bad, code) in [
            (json!([{"id":"zzz","on":true}]), "not_found"),
            (json!({"id":"a"}), "bad_args"),
            (json!([{"id":"a"}]), "bad_args"),
            (json!([{"id":1,"on":true}]), "bad_args"),
            (json!([{"id":"a","on":"yes"}]), "bad_args"),
            (json!([{"id":"a","on":true},{"id":"a","on":false}]), "bad_args"),
        ] {
            assert_eq!(set_mods(&mut p, &bad).unwrap_err().code, code, "{bad}");
            assert_eq!(p, before);
        }
    }
}
