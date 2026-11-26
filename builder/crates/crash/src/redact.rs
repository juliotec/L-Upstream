//! The home folder and the user's name out of a line: gpu/vk/compat/longshot_redact.h's rule, byte for byte (the game
//! keeps the C one for its log and the Insert menu's Bug report). The home folder, as a whole, is written `~`
//! (`%USERPROFILE%` on Windows), and the user's name as a folder of any path `<user>` (/run/media/<user>,
//! D:\Users\<user>). The rest of the line stays as it is. A line redacted again comes out the same.

/// A path separator.
pub fn sep(c: u8) -> bool {
    c == b'/' || c == b'\\'
}

/// A character a file or folder name has: a name ends at any other.
fn name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-' || c >= 0x80
}

/// `what` at `s[at..]` (Windows: either separator for either, ASCII in any case).
fn matches_at(s: &[u8], at: usize, what: &[u8], windows: bool) -> bool {
    if what.is_empty() || at + what.len() > s.len() {
        return false;
    }
    s[at..at + what.len()].iter().zip(what).all(|(&a, &b)| {
        if sep(a) && sep(b) {
            return windows || a == b;
        }
        if windows { a.eq_ignore_ascii_case(&b) } else { a == b }
    })
}

/// `line` with `home` and `user` taken out (either may be empty: nothing of theirs is then taken out).
pub fn line_bytes(line: &[u8], home: &str, user: &str, windows: bool) -> Vec<u8> {
    let mut home = home.as_bytes();
    while home.len() > 1 && sep(home[home.len() - 1]) {
        home = &home[..home.len() - 1];
    }
    let with_home = home.len() > 1;
    let user = user.as_bytes();
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        // (the home folder as a whole: not inside a longer name, not the end of another path)
        if with_home
            && (i == 0 || (!name_char(line[i - 1]) && !sep(line[i - 1])))
            && matches_at(line, i, home, windows)
            && (i + home.len() >= line.len() || !name_char(line[i + home.len()]))
        {
            out.extend_from_slice(if windows { b"%USERPROFILE%" } else { b"~" });
            i += home.len();
            continue;
        }
        // (the user's name as a folder of any path)
        if !user.is_empty()
            && i > 0
            && sep(line[i - 1])
            && matches_at(line, i, user, windows)
            && (i + user.len() >= line.len() || !name_char(line[i + user.len()]))
        {
            out.extend_from_slice(b"<user>");
            i += user.len();
            continue;
        }
        out.push(line[i]);
        i += 1;
    }
    out
}

/// [`line_bytes`] of a text.
pub fn line(line: &str, home: &str, user: &str, windows: bool) -> String {
    String::from_utf8(line_bytes(line.as_bytes(), home, user, windows))
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Who is left out: this PC's home folder and user name (the Insert menu's Bug report's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Who {
    /// The home folder.
    pub home: String,
    /// The user's name.
    pub user: String,
    /// Windows' rules (`%USERPROFILE%`, either separator, any case).
    pub windows: bool,
}

impl Who {
    /// This PC's: `USERPROFILE` and `USERNAME` on Windows; `HOME` and `USER` elsewhere (the home's last folder when
    /// `USER` is not set).
    pub fn here() -> Who {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        if cfg!(windows) {
            Who { home: var("USERPROFILE"), user: var("USERNAME"), windows: true }
        } else {
            let home = var("HOME");
            let mut user = var("USER");
            if user.is_empty() {
                user = home.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
            }
            Who { home, user, windows: false }
        }
    }

    /// `s` with this one's home and name out.
    pub fn redact(&self, s: &str) -> String {
        line(s, &self.home, &self.user, self.windows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (input, home, user, windows, what longshot_redact.h's longshot_redact_line wrote for it: each row's answer was
    /// taken from the C, compiled and run on the same input)
    const C_SAID: &[(&str, &str, &str, bool, &str)] = &include!("redact_cases.in");

    #[test]
    fn it_matches_the_c_rule_on_the_same_inputs() {
        for (input, home, user, windows, want) in C_SAID {
            assert_eq!(line(input, home, user, *windows), *want, "{input:?} {home:?} {user:?} {windows}");
        }
    }

    #[test]
    fn a_line_redacted_again_comes_out_the_same() {
        for (input, home, user, windows, _) in C_SAID {
            let once = line(input, home, user, *windows);
            assert_eq!(line(&once, home, user, *windows), once);
        }
    }
}
