//! The button-prompt faces: everything that differs between controllers, in one place.
//!
//! To add a face: an entry in [`FACES`], its `HOS_PROMPT_*` in `gpu/vk/vk_api.h` (in this order, after the Switch),
//! its name in `host/prompts.c`'s `faces[]`, and the Insert menu's names for it (`gpu/vk/vk_menu.cpp`).
//!
//! Each face has: the Kenney Input Prompts font its icons come from (`third_party/kenney_input`) and its glyph names'
//! prefix; each pad input's glyph (`/name`: the whole name; one left out keeps the game's icon; no Guide: the makers'
//! logos are their trademarks); the controller for the pause menu's controls page, drawn like the game's Pro
//! Controller ([`Drawing`]; a hand-made picture in the face's folder takes its place); and where each pad input is in
//! that picture ([`Parts`]), for the page's leader lines.

/// The `HOS_PAD_*` inputs (`gpu/vk/vk_api.h`), in order.
pub const PADS: [&str; 17] = [
    "south",
    "east",
    "west",
    "north",
    "back",
    "guide",
    "start",
    "lstick",
    "rstick",
    "lshoulder",
    "rshoulder",
    "up",
    "down",
    "left",
    "right",
    "ltrigger",
    "rtrigger",
];

/// A pad input's number.
pub fn pad(name: &str) -> usize {
    PADS.iter().position(|p| *p == name).expect("a pad input")
}

/// The Switch buttons, in `HOS_SW_*` order.
pub const SW: [&str; 16] =
    ["a", "b", "x", "y", "ls", "rs", "l", "r", "zl", "zr", "plus", "minus", "left", "up", "right", "down"];

/// A Switch button's number.
pub fn sw(name: &str) -> usize {
    SW.iter().position(|p| *p == name).expect("a Switch button")
}

/// The Switch buttons' code points in the game's font, in `HOS_SW_*` order.
pub const SWITCH: [u32; 16] = [
    0xE040, 0xE041, 0xE042, 0xE043, 0xE056, 0xE057, 0xE044, 0xE045, 0xE046, 0xE047, 0xE04B, 0xE04C, 0xE062, 0xE060,
    0xE063, 0xE061,
];

/// Longshot's default pad input for each Switch button (`host/nn_hid.c pad_default`: by position, A the right face
/// button).
pub const DEFAULT: [&str; 16] = [
    "east",
    "south",
    "north",
    "west",
    "lstick",
    "rstick",
    "lshoulder",
    "rshoulder",
    "ltrigger",
    "rtrigger",
    "start",
    "back",
    "left",
    "up",
    "right",
    "down",
];

/// The game's own icon for each pad input (in [`PADS`] order): what the Switch face shows.
pub const SWITCH_ICON: [u32; 17] = [
    0xE041, 0xE040, 0xE043, 0xE042, 0xE04C, 0xE04A, 0xE04B, 0xE056, 0xE057, 0xE044, 0xE045, 0xE060, 0xE061, 0xE062,
    0xE063, 0xE046, 0xE047,
];

/// The icons that do not follow the controls, by the Kenney glyph's name after the face's prefix: the whole d-pad (the
/// sticks keep the game's icons, right for any controller).
pub const STATIC: [(u32, &str); 3] =
    [(0xE049, "dpad"), (0xE064, "dpad_vertical_outline"), (0xE065, "dpad_horizontal_outline")];

/// One control of a drawn controller.
#[derive(Clone, Copy, Debug)]
pub enum Control {
    /// The Pro Controller's stick, scaled: centre (x, y) in the body's box (0 to 1), size a fraction of its width.
    Stick(f64, f64, f64),
    /// The Pro Controller's d-pad, the same way.
    Dpad(f64, f64, f64),
    /// A face button: a disc with the label of the pad input given.
    Face(f64, f64, f64, &'static str),
    /// A small button: a disc with its label (none: a blank button).
    Small(f64, f64, f64, Option<&'static str>),
    /// A touchpad: centre, (width, height) as fractions of the body's.
    Pad(f64, f64, (f64, f64)),
    /// A screen, the same way.
    Screen(f64, f64, (f64, f64)),
}

/// A face's drawn controller: the Kenney font and its controller glyph for the body (`None`: the Steam Deck's own
/// shape), and its controls.
#[derive(Clone, Copy, Debug)]
pub struct Drawing {
    /// The body's font and glyph.
    pub body: Option<(&'static str, &'static str)>,
    /// The controls.
    pub controls: &'static [Control],
}

/// Where each pad input is in a face's picture (picture pixels), for the page's leader lines: a stick's centre (its
/// line ends on it), a button's centre and radius (its line ends just off the edge it comes from), a shoulder's top
/// edge (radius 0); and boxes (x0, y0, x1, y1) the lines keep off (a screen, a touchpad).
#[derive(Clone, Debug)]
pub struct Parts {
    /// (pad input, (x, y, r)).
    pub points: Vec<(&'static str, (f64, f64, f64))>,
    /// Boxes to keep off.
    pub boxes: Vec<(f64, f64, f64, f64)>,
}

impl Parts {
    /// A pad input's place.
    pub fn get(&self, pad: &str) -> Option<(f64, f64, f64)> {
        self.points.iter().find(|p| p.0 == pad).map(|p| p.1)
    }

    /// A picture's parts: `axis` (x plus its mirror's x), the sticks ((x, y, r) left, right), the d-pad (x, y, an
    /// arm's distance, its r), the face buttons ((x, y) north, west, east, south, r), back ((x, y, r); start
    /// mirrored), home, the shoulders ((x, y) trigger, bumper; the right ones mirrored), and boxes.
    #[allow(clippy::too_many_arguments)]
    fn new(
        axis: f64,
        sticks: [(f64, f64, f64); 2],
        dpad: (f64, f64, f64, f64),
        face: ([(f64, f64); 4], f64),
        small: (f64, f64, f64),
        home: (f64, f64, f64),
        shoulders: [(f64, f64); 2],
        boxes: Vec<(f64, f64, f64, f64)>,
    ) -> Parts {
        let (dx, dy, arm, ar) = dpad;
        let ([n, w, e, s], fr) = face;
        let (bx, by, br) = small;
        let [(tx, ty), (sx, sy)] = shoulders;
        let points = vec![
            ("lstick", sticks[0]),
            ("rstick", sticks[1]),
            ("up", (dx, dy - arm, ar)),
            ("down", (dx, dy + arm, ar)),
            ("left", (dx - arm, dy, ar)),
            ("right", (dx + arm, dy, ar)),
            ("north", (n.0, n.1, fr)),
            ("west", (w.0, w.1, fr)),
            ("east", (e.0, e.1, fr)),
            ("south", (s.0, s.1, fr)),
            ("back", (bx, by, br)),
            ("start", (axis - bx, by, br)),
            ("guide", home),
            ("ltrigger", (tx, ty, 0.0)),
            ("lshoulder", (sx, sy, 0.0)),
            ("rtrigger", (axis - tx, ty, 0.0)),
            ("rshoulder", (axis - sx, sy, 0.0)),
        ];
        Parts { points, boxes }
    }
}

/// A controller face.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    /// Its name (`host/prompts.c`).
    pub name: &'static str,
    /// Its Kenney font, in `third_party/kenney_input`.
    pub font: &'static str,
    /// The glyph names' prefix.
    pub prefix: &'static str,
    /// Each pad input's glyph after the prefix (`/name`: the whole name).
    pub icons: &'static [(&'static str, &'static str)],
    /// Its controller for the controls page.
    pub drawing: Drawing,
    /// Its picture's parts.
    pub parts: fn() -> Parts,
}

impl Face {
    /// A pad input's Kenney glyph name, if the face has one.
    pub fn icon(&self, pad: &str) -> Option<String> {
        self.icons.iter().find(|i| i.0 == pad).map(|i| match i.1.strip_prefix('/') {
            Some(whole) => whole.to_string(),
            None => format!("{}{}", self.prefix, i.1),
        })
    }
}

/// The game's Pro Controller's parts (the Switch face's picture).
pub fn switch_parts() -> Parts {
    Parts::new(
        1159.0,
        [(348.0, 227.0, 48.0), (683.0, 348.0, 48.0)],
        (455.0, 348.0, 45.0, 17.0),
        ([(800.0, 168.0), (731.0, 227.0), (869.0, 227.0), (800.0, 286.0)], 32.0),
        (477.0, 161.0, 19.0),
        (631.0, 227.0, 21.0),
        [(375.0, 39.0), (290.0, 60.0)],
        vec![],
    )
}

/// The PlayStation faces' picture, drawn by hand (the same DualSense for both).
fn dualsense() -> Parts {
    Parts::new(
        1158.0,
        [(442.0, 334.0, 44.0), (716.0, 334.0, 44.0)],
        (312.0, 213.0, 45.0, 21.0),
        ([(848.0, 146.0), (786.0, 213.0), (910.0, 213.0), (848.0, 279.0)], 27.0),
        (379.0, 115.0, 11.0),
        (579.0, 327.0, 26.0),
        [(340.0, 49.0), (265.0, 68.0)],
        vec![(413.0, 57.0, 751.0, 240.0), (559.0, 376.0, 599.0, 388.0)],
    )
}

/// The Xbox face's picture, drawn by hand (an Xbox One pad); a bumper's end is under its trigger's, as the Menu line
/// leaves no other way to RB without a crossing.
fn xbox() -> Parts {
    Parts::new(
        1158.0,
        [(357.0, 202.0, 44.0), (695.0, 335.0, 44.0)],
        (462.0, 346.0, 41.0, 17.0),
        ([(805.0, 146.0), (746.0, 205.0), (865.0, 206.0), (805.0, 260.0)], 28.0),
        (516.0, 203.0, 18.0),
        (581.0, 116.0, 31.0),
        [(425.0, 39.0), (385.0, 69.0)],
        vec![],
    )
}

/// The Steam Deck face's picture, drawn by hand (L2 and R2 inward of View and Menu, so their lines don't cross).
fn steamdeck() -> Parts {
    Parts::new(
        1162.0,
        [(287.0, 241.0, 35.0), (874.0, 241.0, 35.0)],
        (206.0, 228.0, 20.0, 9.0),
        ([(955.0, 200.0), (930.0, 225.0), (981.0, 225.0), (955.0, 250.0)], 12.0),
        (255.0, 196.0, 8.0),
        (303.0, 409.0, 10.0),
        [(272.0, 162.0), (212.0, 162.0)],
        vec![
            (355.0, 197.0, 810.0, 482.0),
            (231.0, 288.0, 326.0, 383.0),
            (834.0, 288.0, 929.0, 383.0),
            (836.0, 399.0, 883.0, 418.0),
        ],
    )
}

/// The d-pad's arms, the same names in each of Kenney's fonts.
macro_rules! dpad_icons {
    ($($rest:tt)*) => {
        &[
            ("up", "dpad_up_outline"),
            ("down", "dpad_down_outline"),
            ("left", "dpad_left_outline"),
            ("right", "dpad_right_outline"),
            $($rest)*
        ]
    };
}

use Control::{Dpad, Face as Fb, Pad, Screen, Small, Stick};

/// The faces after the Switch's, in `HOS_PROMPT_*` order (the keyboard is last, `keys`).
pub const FACES: [Face; 4] = [
    Face {
        name: "xbox",
        font: "kenney_input_xbox_series.otf",
        prefix: "xbox_",
        icons: dpad_icons!(
            ("south", "button_a"),
            ("east", "button_b"),
            ("west", "button_x"),
            ("north", "button_y"),
            ("back", "button_view"),
            ("start", "button_menu"),
            ("lshoulder", "lb"),
            ("rshoulder", "rb"),
            ("ltrigger", "lt"),
            ("rtrigger", "rt"),
            ("lstick", "ls"),
            ("rstick", "rs"),
        ),
        drawing: Drawing {
            body: Some(("kenney_input_xbox_series.otf", "controller_xboxseries")),
            controls: &[
                Stick(0.215, 0.262, 0.158),
                Dpad(0.345, 0.530, 0.160),
                Stick(0.640, 0.495, 0.158),
                Fb(0.700, 0.275, 0.080, "west"),
                Fb(0.775, 0.375, 0.080, "south"),
                Fb(0.775, 0.175, 0.080, "north"),
                Fb(0.850, 0.275, 0.080, "east"),
                Small(0.498, 0.120, 0.090, Some("guide")),
                Small(0.420, 0.275, 0.052, Some("back")),
                Small(0.576, 0.275, 0.052, Some("start")),
                Small(0.498, 0.355, 0.044, None),
            ],
        },
        parts: xbox,
    },
    Face {
        name: "ps4",
        font: "kenney_input_playstation_series.otf",
        prefix: "playstation_",
        icons: dpad_icons!(
            ("south", "button_cross"),
            ("east", "button_circle"),
            ("west", "button_square"),
            ("north", "button_triangle"),
            ("back", "/playstation4_button_share"),
            ("start", "/playstation4_button_options"),
            ("lshoulder", "trigger_l1"),
            ("rshoulder", "trigger_r1"),
            ("ltrigger", "trigger_l2"),
            ("rtrigger", "trigger_r2"),
            ("lstick", "button_l3"),
            ("rstick", "button_r3"),
        ),
        drawing: Drawing {
            body: Some(("kenney_input_playstation_series.otf", "controller_playstation4")),
            controls: &[
                Dpad(0.195, 0.290, 0.180),
                Stick(0.340, 0.520, 0.150),
                Stick(0.660, 0.520, 0.150),
                Pad(0.499, 0.200, (0.320, 0.280)),
                Fb(0.742, 0.285, 0.072, "west"),
                Fb(0.814, 0.175, 0.072, "north"),
                Fb(0.814, 0.395, 0.072, "south"),
                Fb(0.886, 0.285, 0.072, "east"),
                Small(0.298, 0.130, 0.036, Some("back")),
                Small(0.702, 0.130, 0.036, Some("start")),
                Small(0.500, 0.510, 0.046, Some("guide")),
            ],
        },
        parts: dualsense,
    },
    Face {
        name: "ps5",
        font: "kenney_input_playstation_series.otf",
        prefix: "playstation_",
        icons: dpad_icons!(
            ("south", "button_cross"),
            ("east", "button_circle"),
            ("west", "button_square"),
            ("north", "button_triangle"),
            ("back", "/playstation5_button_create"),
            ("start", "/playstation5_button_options"),
            ("lshoulder", "trigger_l1"),
            ("rshoulder", "trigger_r1"),
            ("ltrigger", "trigger_l2"),
            ("rtrigger", "trigger_r2"),
            ("lstick", "button_l3"),
            ("rstick", "button_r3"),
        ),
        drawing: Drawing {
            body: Some(("kenney_input_playstation_series.otf", "controller_playstation5")),
            controls: &[
                Dpad(0.170, 0.265, 0.190),
                Stick(0.320, 0.490, 0.145),
                Stick(0.680, 0.490, 0.145),
                Pad(0.498, 0.175, (0.400, 0.300)),
                Fb(0.765, 0.265, 0.078, "west"),
                Fb(0.840, 0.150, 0.078, "north"),
                Fb(0.840, 0.380, 0.078, "south"),
                Fb(0.915, 0.265, 0.078, "east"),
                Small(0.248, 0.080, 0.036, Some("back")),
                Small(0.750, 0.080, 0.036, Some("start")),
                Small(0.496, 0.480, 0.046, Some("guide")),
                Small(0.496, 0.575, 0.030, None),
            ],
        },
        parts: dualsense,
    },
    Face {
        name: "steamdeck",
        font: "kenney_input_steam_deck.otf",
        prefix: "steamdeck_",
        icons: dpad_icons!(
            ("south", "button_a"),
            ("east", "button_b"),
            ("west", "button_x"),
            ("north", "button_y"),
            ("back", "button_view"),
            ("start", "button_options"),
            ("lshoulder", "button_l1"),
            ("rshoulder", "button_r1"),
            ("ltrigger", "button_l2"),
            ("rtrigger", "button_r2"),
        ),
        drawing: Drawing {
            // (the real Deck's proportions: Kenney's icon is a shorter one)
            body: None,
            controls: &[
                Screen(0.500, 0.465, (0.580, 0.860)),
                Dpad(0.072, 0.200, 0.078),
                Stick(0.158, 0.220, 0.085),
                Stick(0.842, 0.220, 0.085),
                Pad(0.140, 0.510, (0.112, 0.270)),
                Pad(0.860, 0.510, (0.112, 0.270)),
                Fb(0.935, 0.090, 0.036, "north"),
                Fb(0.935, 0.270, 0.036, "south"),
                Fb(0.900, 0.180, 0.036, "west"),
                Fb(0.970, 0.180, 0.036, "east"),
                Small(0.125, 0.075, 0.024, Some("back")),
                Small(0.875, 0.075, 0.024, Some("start")),
                Small(0.150, 0.755, 0.030, Some("guide")),
                Small(0.850, 0.755, 0.030, None),
            ],
        },
        parts: steamdeck,
    },
];

/// Every face, in `HOS_PROMPT_*` order: the Switch, [`FACES`], the keyboard.
pub fn all_faces() -> Vec<&'static str> {
    std::iter::once("switch").chain(FACES.iter().map(|f| f.name)).chain(std::iter::once("keyboard")).collect()
}

/// A face by name (not the Switch's or the keyboard's).
pub fn face(name: &str) -> Option<&'static Face> {
    FACES.iter().find(|f| f.name == name)
}

/// A face's picture's parts (the Switch: the game's Pro Controller); `None` for the keyboard.
pub fn parts(name: &str) -> Option<Parts> {
    if name == "switch" { Some(switch_parts()) } else { face(name).map(|f| (f.parts)()) }
}
