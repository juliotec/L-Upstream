//! What the build says when the game's shader preparation ends badly: each exit status the game gives its own reason
//! (gpu/vk/vk_main.cpp: 3 no usable Vulkan, 4 the watchdog's stuck driver), said in the build's warning.

use super::*;
use std::sync::{Arc, Mutex};

/// A Write the test reads back: what the Reporter said.
#[derive(Clone, Default)]
struct Said(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Said {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A game folder and a stand-in game that prints a line and ends with `exit`.
fn game_that_exits(name: &str, exit: i32) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let d = crate::scratch(name);
    let folder = d.join("game");
    std::fs::create_dir_all(&folder).unwrap();
    let exe = d.join("longshot");
    std::fs::write(&exe, format!("#!/bin/sh\necho 'preparing'\nexit {exit}\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    (exe, folder)
}

#[test]
fn each_exit_status_of_the_preparation_says_why() {
    for (exit, why) in [(3, "no usable Vulkan here"), (4, "the GPU driver stopped responding")] {
        let (exe, folder) = game_that_exits(&format!("shaders-outcome-{exit}"), exit);
        assert_eq!(
            supervised(&exe, &folder, &folder.join("cache"), CompileShaders::Most, 1, &|_| {}, &|_| {}, &|| false),
            Err(why.to_string()),
            "exit {exit}"
        );
    }
}

#[test]
fn the_build_warns_that_the_gpu_driver_stopped_responding_at_exit_status_4() {
    let (exe, folder) = game_that_exits("shaders-outcome-warn", 4);
    let said = Said::default();
    let r = Reporter::to(build_core::report::Format::Text, Box::new(said.clone()));
    assert!(!prepare(&exe, &folder, CompileShaders::Most, 1, &r));
    let text = String::from_utf8(said.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("The shaders were not prepared now (the GPU driver stopped responding)"),
        "the warning: {text}"
    );
}

#[test]
fn each_run_s_game_is_given_to_the_memory_gate_s_ticket() {
    let (exe, folder) = game_that_exits("shaders-outcome-pid", 0);
    let pids = Mutex::new(Vec::new());
    let ok = supervised(
        &exe,
        &folder,
        &folder.join("cache"),
        CompileShaders::Most,
        1,
        &|_| {},
        &|pid| pids.lock().unwrap().push(pid),
        &|| false,
    );
    assert_eq!(ok, Ok(()));
    let pids = pids.into_inner().unwrap();
    assert_eq!(pids.len(), 1, "one run, its game's pid");
    assert!(pids[0] != 0 && pids[0] != std::process::id());
}
