//! The channels and the version order (docs/dev/reference/releases.md, "Versions"; the Longshot Launcher's
//! Core/Versions.cs, ported): a stable is `vX.Y.Z` from `v3.0.0` on, a nightly `nightly-<commit count>-<commit>`.
//! A nightly and a stable are never ordered against each other; a name that is neither is on no channel.

/// A release channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// `vX.Y.Z`.
    Stable,
    /// `nightly-<count>-<commit>`, from `main`.
    Nightly,
}

impl Channel {
    /// Its name: `stable` or `nightly`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }

    /// The channel of its name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Channel> {
        match name {
            "stable" => Some(Channel::Stable),
            "nightly" => Some(Channel::Nightly),
            _ => None,
        }
    }

    /// The channel `tag` is on: a nightly's name nightly, a release's (`vX.Y.Z` from [`FIRST_MAJOR`]) stable, any
    /// other none.
    #[must_use]
    pub fn of(tag: &str) -> Option<Channel> {
        if nightly_key(tag).is_some() {
            Some(Channel::Nightly)
        } else if release_key(tag).is_some() {
            Some(Channel::Stable)
        } else {
            None
        }
    }
}

/// Longshot's first major version: a lower `vX.Y.Z` is Hookshot's, no version of Longshot.
pub const FIRST_MAJOR: u64 = 3;

/// A name a version folder may have and the forge's API is asked by: `[0-9A-Za-z._+-]{1,128}`, not `.` or `..`.
#[must_use]
pub fn is_tag_name(tag: &str) -> bool {
    (1..=128).contains(&tag.len())
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        && tag != "."
        && tag != ".."
}

/// A nightly's place in the nightly order: its count, then `-dirty` after its own commit's.
fn nightly_key(tag: &str) -> Option<[u64; 2]> {
    let rest = tag.strip_prefix("nightly-")?;
    let (count, rest) = rest.split_once('-')?;
    let (commit, dirty) = match rest.strip_suffix("-dirty") {
        Some(c) => (c, 1),
        None => (rest, 0),
    };
    let digits =
        |s: &str, n: std::ops::RangeInclusive<usize>| n.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    let hex = commit.len() == 8 && commit.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (digits(count, 1..=9) && hex).then(|| [count.parse().unwrap_or(0), dirty])
}

/// A release's place in the version order, `vX.Y.Z` from [`FIRST_MAJOR`] on.
fn release_key(tag: &str) -> Option<[u64; 3]> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let mut key = [0u64; 3];
    for k in &mut key {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *k = p.parse().ok()?;
    }
    (parts.next().is_none() && key[0] >= FIRST_MAJOR).then_some(key)
}

/// `a` newer than `b`: both releases in version order, or both nightlies in the nightly order. A nightly and a
/// release are not ordered (false both ways), as when either is no version.
#[must_use]
pub fn newer(a: &str, b: &str) -> bool {
    match (nightly_key(a), nightly_key(b), release_key(a), release_key(b)) {
        (Some(x), Some(y), ..) => x > y,
        (None, None, Some(x), Some(y)) => x > y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channels_and_the_order() {
        assert_eq!(Channel::of("nightly-2029-c544828f"), Some(Channel::Nightly));
        assert_eq!(Channel::of("nightly-2029-c544828f-dirty"), Some(Channel::Nightly));
        assert_eq!(Channel::of("v3.1.0"), Some(Channel::Stable));
        assert_eq!(Channel::of("v2.9.9"), None, "Hookshot's numbers");
        assert_eq!(Channel::of("pr-12-1a2b3c4d"), None);
        assert_eq!(Channel::of("nightly-12-XYZ"), None);
        assert!(newer("v3.1.1", "v3.1.0") && newer("v3.10.0", "v3.9.9") && !newer("v3.1.0", "v3.1.0"));
        assert!(newer("nightly-100-aaaaaaaa", "nightly-99-bbbbbbbb"));
        assert!(newer("nightly-99-bbbbbbbb-dirty", "nightly-99-bbbbbbbb"));
        assert!(!newer("nightly-100-aaaaaaaa", "v3.0.0") && !newer("v3.0.0", "nightly-100-aaaaaaaa"));
        assert!(is_tag_name("v3.0.0") && !is_tag_name("..") && !is_tag_name("a/b") && !is_tag_name(""));
    }
}
