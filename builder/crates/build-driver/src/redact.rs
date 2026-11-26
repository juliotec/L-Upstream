//! The home folder and the user's name out of a log line, by the game's own rule (gpu/vk/compat/longshot_redact.h,
//! host/log_redact.c: the same output): the home folder, as a whole, written `~` (`%USERPROFILE%` on Windows), and the
//! user's name as a folder of any path `<user>`. The rest of the line, the rest of each path too, stays.
//! tests/test_log_redact.py compares this with the C rule (the `redact` example).

fn sep(c: u8) -> bool {
    c == b'/' || c == b'\\'
}

/// A byte of a name: ASCII letters and digits, `.`, `_`, `-`, and every byte of a letter outside ASCII.
fn name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-') || c >= 0x80
}

/// Whether `what` is at `at` in `s`: a separator matches either separator on Windows, and ASCII letters any case there.
fn matches_at(s: &[u8], at: usize, what: &[u8], windows: bool) -> bool {
    if what.is_empty() || at + what.len() > s.len() {
        return false;
    }
    s[at..at + what.len()].iter().zip(what).all(|(&a, &b)| {
        if sep(a) && sep(b) {
            windows || a == b
        } else if windows {
            a.eq_ignore_ascii_case(&b)
        } else {
            a == b
        }
    })
}

/// `line` with `home` and `user` taken out (either may be empty).
pub fn redact_line(line: &str, home: &str, user: &str, windows: bool) -> String {
    let s = line.as_bytes();
    let mut home = home.as_bytes();
    while home.len() > 1 && sep(home[home.len() - 1]) {
        home = &home[..home.len() - 1];
    }
    let with_home = home.len() > 1;
    let user = user.as_bytes();
    let ends = |at: usize| at >= s.len() || !name_byte(s[at]);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if with_home
            && (i == 0 || (!name_byte(s[i - 1]) && !sep(s[i - 1])))
            && matches_at(s, i, home, windows)
            && ends(i + home.len())
        {
            out.extend_from_slice(if windows { b"%USERPROFILE%" } else { b"~" });
            i += home.len();
        } else if i > 0 && sep(s[i - 1]) && matches_at(s, i, user, windows) && ends(i + user.len()) {
            out.extend_from_slice(b"<user>");
            i += user.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    // (only whole names of ASCII bytes, or whole UTF-8 letters, are replaced: still UTF-8)
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Who the PC's user is, for the rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Who {
    /// The home folder.
    pub home: String,
    /// The user's name.
    pub user: String,
    /// The home folder's own name, when it is not the user's (a renamed account on Windows).
    pub folder: String,
    /// Windows' rule (either separator, ASCII letters any case).
    pub windows: bool,
}

impl Who {
    /// This PC's: `USERPROFILE` and `USERNAME` on Windows, `HOME` and `USER` elsewhere, from `env`.
    pub fn from_env(env: impl Fn(&str) -> Option<String>, windows: bool) -> Who {
        let (h, u) = if windows { ("USERPROFILE", "USERNAME") } else { ("HOME", "USER") };
        let (home, user) = (env(h).unwrap_or_default(), env(u).unwrap_or_default());
        let leaf = home.trim_end_matches(['/', '\\']);
        let leaf = &leaf[leaf.rfind(['/', '\\']).map_or(0, |i| i + 1)..];
        let folder = if leaf.len() >= 2 && leaf != user { leaf.to_string() } else { String::new() };
        Who { home, user, folder, windows }
    }

    /// This process's.
    pub fn here() -> Who {
        Who::from_env(|k| std::env::var(k).ok().filter(|v| !v.is_empty()), cfg!(windows))
    }

    /// `line` without the home folder and the user's name (and the home folder's own name).
    pub fn redact(&self, line: &str) -> String {
        let s = redact_line(line, &self.home, &self.user, self.windows);
        if self.folder.is_empty() { s } else { redact_line(&s, "", &self.folder, self.windows) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_folder_as_a_whole_and_the_name_as_a_folder() {
        let r = |l: &str| redact_line(l, "/home/ana", "ana", false);
        assert_eq!(r("reading /home/ana/Games/x.nup"), "reading ~/Games/x.nup");
        assert_eq!(r("/home/ana"), "~");
        assert_eq!(r("/home/anabel/x"), "/home/anabel/x", "another name that starts the same");
        assert_eq!(
            r("/media/ana/disk and ana herself"),
            "/media/<user>/disk and ana herself",
            "a word of the line stays"
        );
        assert_eq!(r("x/home/ana/y"), "x/home/<user>/y", "inside another path: only the name");
        assert_eq!(redact_line(&r("/home/ana/a"), "/home/ana", "ana", false), "~/a", "again: the same");
    }

    #[test]
    fn windows_any_separator_and_case() {
        let r = |l: &str| redact_line(l, "C:\\Users\\Ana B", "Ana B", true);
        assert_eq!(r("c:/users/ana b/Games"), "%USERPROFILE%/Games");
        assert_eq!(r("D:\\Users\\ANA B\\x"), "D:\\Users\\<user>\\x");
        assert_eq!(r("C:\\Users\\Ana Bé\\x"), "C:\\Users\\Ana Bé\\x", "a longer name with a letter outside ASCII");
    }

    #[test]
    fn who_names_the_home_folder_when_it_is_not_the_user() {
        let env = |k: &str| match k {
            "USERPROFILE" => Some("C:\\Users\\anab".to_string()),
            "USERNAME" => Some("Ana".to_string()),
            _ => None,
        };
        let w = Who::from_env(env, true);
        assert_eq!(w.folder, "anab");
        assert_eq!(w.redact("E:\\Users\\anab\\x and C:\\Users\\anab\\y"), "E:\\Users\\<user>\\x and %USERPROFILE%\\y");
    }
}
