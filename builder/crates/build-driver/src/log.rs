//! The run log: the build's event stream as it goes to stdout, also written to the work folder's `build.log` (and a
//! file the caller names), whole lines at a time, without the player's home folder and name (the game's rule:
//! crate::redact; players attach the log to build problems; stdout, which a launcher reads, stays as it is).

use crate::redact::Who;
use std::io::Write;
use std::path::Path;

/// `line` as a log keeps it: without the user's home folder and name ([`Who::redact`]), in a JSON line's escaped form
/// too (a Windows home folder's backslashes doubled).
pub fn for_log(who: &Who, line: &str) -> String {
    let s = who.redact(line);
    if who.windows && who.home.contains('\\') {
        let json = Who { home: who.home.replace('\\', "\\\\"), ..who.clone() };
        return json.redact(&s);
    }
    s
}

/// stdout, and the log files.
pub struct Tee {
    out: Box<dyn Write + Send>,
    files: Vec<std::fs::File>,
    line: Vec<u8>,
    who: Who,
}

impl Tee {
    /// `out` and the open `file`, written on (a panic's line added to a log).
    pub fn append(out: Box<dyn Write + Send>, file: std::fs::File) -> Tee {
        Tee { out, files: vec![file], line: Vec::new(), who: Who::here() }
    }

    /// stdout and `files` (each made again; one that cannot be opened is left out, said on stderr).
    pub fn new(out: Box<dyn Write + Send>, files: &[&Path]) -> Tee {
        let mut opened = Vec::new();
        for f in files {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::File::create(f) {
                Ok(h) => opened.push(h),
                Err(e) => eprintln!("the build log {} was not opened: {e}", f.display()),
            }
        }
        Tee { out, files: opened, line: Vec::new(), who: Who::here() }
    }

    fn redact(&self, line: &str) -> String {
        for_log(&self.who, line)
    }

    fn write_line(&mut self, line: &[u8]) {
        let text = self.redact(&String::from_utf8_lossy(line));
        for f in &mut self.files {
            let _ = f.write_all(text.as_bytes());
        }
    }
}

impl Drop for Tee {
    fn drop(&mut self) {
        // (what was left without its line break: written at the end)
        if !self.line.is_empty() {
            let rest = std::mem::take(&mut self.line);
            self.write_line(&rest);
        }
    }
}

impl Write for Tee {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.out.write_all(b)?;
        self.line.extend_from_slice(b);
        while let Some(i) = self.line.iter().position(|&c| c == b'\n') {
            let line: Vec<u8> = self.line.drain(..=i).collect();
            self.write_line(&line);
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        for f in &mut self.files {
            let _ = f.flush();
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_lines_without_the_home_folder() {
        let d = crate::scratch("log");
        let f = d.join("build.log");
        let mut t = Tee::new(Box::new(std::io::sink()), &[&f]);
        t.who = Who { home: "/home/player".into(), user: "player".into(), folder: String::new(), windows: false };
        write!(t, "reading /home/player/Games/x.nup").unwrap();
        writeln!(t, " now, from /media/player/usb").unwrap();
        write!(t, "half").unwrap();
        t.flush().unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "reading ~/Games/x.nup now, from /media/<user>/usb\n");
        drop(t);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "reading ~/Games/x.nup now, from /media/<user>/usb\nhalf");
        let _ = std::fs::remove_dir_all(&d);
    }
}
