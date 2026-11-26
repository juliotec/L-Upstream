//! The keyboard face of the button prompts: which icon each key and the sticks show, and the code points they take
//! (`font::keyboard_glyphs` draws them, from Kenney's Input Prompts Keyboard & Mouse font, CC0,
//! `third_party/kenney_input_keyboard`, and the keys whose names are words and the mouse's buttons drawn there).
//!
//! The font has, beside the faces' icons, the icon of each key at [`KEY_BASE`] + its key code. On the keyboard face
//! `host/prompts.c` points each Switch button's code point at the icon of the first key bound to it; a text rewritten
//! while the game runs gets codes of its own from [`SLOT_BASE`] ([`SLOTS`] codes no font of the game has).

/// The keys' icons: `KEY_BASE` + a key code (`host/prompts.c`: `PAD_BASE + FACE_CODES x HOS_PROMPT_N`).
pub const KEY_BASE: u32 = 0xF7C0;
/// The codes a rewritten text gives out (U+EEC0..U+EFFF: in none of the game's fonts).
pub const SLOT_BASE: u32 = 0xEEC0;
/// How many.
pub const SLOTS: u32 = 320;
/// Kenney's keyboard font, in `third_party/kenney_input_keyboard`.
pub const FONT: &str = "kenney_input_keyboard_&_mouse.otf";
/// The game's bold font, for the word keys.
pub const LABEL_FONT: &str = "Font2/Scalable/ClarimoUDPE-DemiBold.Nin_Ounce_NVN2.bfotf.zs";

/// A Windows virtual-key code's Kenney glyph name after `keyboard_` (`/name`: the whole name); `None`: Kenney has no
/// key for it.
pub fn key(code: u32) -> Option<String> {
    let fixed = match code {
        0x01 => "/mouse_left",
        0x02 => "/mouse_right",
        0x04 => "/mouse_scroll",
        0x05 => "/mouse_side_back",
        0x06 => "/mouse_side_forward",
        0x08 => "backspace_icon",
        0x09 => "tab_icon",
        0x0D => "return",
        0x10 | 0xA0 | 0xA1 => "shift_icon",
        0x11 | 0xA2 | 0xA3 => "ctrl",
        0x12 | 0xA4 | 0xA5 => "alt",
        0x13 => "pause",
        0x14 => "capslock_icon",
        0x1B => "escape",
        0x20 => "space_icon",
        0x21 => "page_up",
        0x22 => "page_down",
        0x23 => "end",
        0x24 => "home",
        0x25 => "arrow_left",
        0x26 => "arrow_up",
        0x27 => "arrow_right",
        0x28 => "arrow_down",
        0x2C => "printscreen",
        0x2D => "insert",
        0x2E => "delete",
        0x5B | 0x5C => "win",
        0x6A => "asterisk",
        0x6B => "numpad_plus",
        0x6D | 0xBD => "minus",
        0x6E | 0xBE => "period",
        0x6F | 0xBF => "slash_forward",
        0x90 => "numlock",
        0x91 => "scroll_lock",
        0xBA => "semicolon",
        0xBB => "equals",
        0xBC => "comma",
        0xC0 => "tilde",
        0xDB => "bracket_open",
        0xDC => "slash_back",
        0xDD => "bracket_close",
        0xDE => "apostrophe",
        0x30..=0x39 => return Some((code - 0x30).to_string()),
        0x60..=0x69 => return Some((code - 0x60).to_string()), // (the numpad's digits: Kenney has none of their own)
        0x41..=0x5A => return Some(char::from_u32(0x61 + code - 0x41).map(String::from).unwrap_or_default()),
        0x70..=0x7B => return Some(format!("f{}", code - 0x70 + 1)),
        _ => return None,
    };
    Some(fixed.to_string())
}

/// A key code's Kenney glyph's whole name ("ANY" for a key Kenney has none for).
pub fn kenney_name(code: u32) -> String {
    let n = key(code).unwrap_or_else(|| "any".into());
    match n.strip_prefix('/') {
        Some(whole) => whole.into(),
        None => format!("keyboard_{n}"),
    }
}

/// The keys drawn as words (Kenney's words are too small at the icons' size), in the game's own bold font.
pub fn label(code: u32) -> Option<String> {
    Some(
        match code {
            0x05 => "M4",
            0x06 => "M5",
            0x11 | 0xA2 | 0xA3 => "Ctrl",
            0x12 | 0xA4 | 0xA5 => "Alt",
            0x13 => "Pause",
            0x1B => "Esc",
            0x21 => "PgUp",
            0x22 => "PgDn",
            0x23 => "End",
            0x24 => "Home",
            0x2C => "PrtSc",
            0x2D => "Ins",
            0x2E => "Del",
            0x90 => "NumLk",
            0x91 => "ScrLk",
            0x60..=0x69 => return Some(format!("Num{}", code - 0x60)),
            _ => return None,
        }
        .to_string(),
    )
}

/// The mouse's buttons, drawn: a key with a mouse cut out of it and that button left white.
pub fn mouse(code: u32) -> Option<&'static str> {
    match code {
        0x01 => Some("left"),
        0x02 => Some("right"),
        0x04 => Some("middle"),
        _ => None,
    }
}

/// A stick of the game's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stick {
    /// The left stick: the keys that move (WASD).
    Move,
    /// The right: the mouse that turns the camera.
    Cam,
}

/// The game's stick icons (code point, stick, the ways shown: up down left right), in `host/prompts.c stick_code`
/// order.
pub const STICKS: [(u32, Stick, &str); 15] = [
    (0xE050, Stick::Move, "d"),
    (0xE052, Stick::Move, "u"),
    (0xE054, Stick::Move, "lr"),
    (0xE058, Stick::Move, "r"),
    (0xE05A, Stick::Move, "l"),
    (0xE05C, Stick::Move, "ud"),
    (0xE05E, Stick::Move, "uldr"),
    (0xE051, Stick::Cam, "d"),
    (0xE053, Stick::Cam, "u"),
    (0xE055, Stick::Cam, "lr"),
    (0xE059, Stick::Cam, "r"),
    (0xE05B, Stick::Cam, "l"),
    (0xE05D, Stick::Cam, "ud"),
    (0xE05F, Stick::Cam, "uldr"),
    (0xE068, Stick::Cam, "uldr"),
];
/// The sticks' icons on the keyboard face.
pub const STICK_KEYS: u32 = 0xF8C0;
/// The sticks' own icons, for the other faces.
pub const STICK_GAME: u32 = 0xF8D0;

/// The key of a way of the left stick (WASD).
pub fn move_key(way: char) -> u32 {
    match way {
        'u' => 0x57,
        'd' => 0x53,
        'l' => 0x41,
        _ => 0x44,
    }
}
