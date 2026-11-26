//! The Insert menu's Field of view, as a percentage of the game's current vertical angle (it was host/oot_fov.c). The
//! hook site `camera.lens` calls it after the angle load, before the game builds its projection (including the cached
//! half-angle tangent used by CPU culling): x19 the camera, s0 the angle.
//!
//! Only camera zero's gameplay view is adjusted. Event cameras keep their authored angle. The source camera is never
//! written: zooms still vary with the game, each update starts from its own angle, and 100% restores it on the next
//! update.

use crate::options;
use guest::{Ctx, GuestCtx, GuestPtr};

/// A camera the site passes (sead's camera holder: its views at +0x18).
pub enum Camera {}
/// A camera's views (the second word, +8, its gameplay view).
pub enum Views {}
/// A view (its vtable first: `sead::LookAtCamera`'s for the gameplay view).
pub enum View {}
/// CameraMgr: its camera count at +32, its cameras at +40.
pub enum CameraMgr {}
/// One of CameraMgr's cameras: its view at +120.
pub enum MgrCamera {}
/// EventMgr: an event running when the word at +104 is not 0.
pub enum EventMgr {}

/// `sead::LookAtCamera`'s vtable: the gameplay view's.
const LOOK_AT_CAMERA_VTABLE: u64 = 0x84c1_8a88;
/// CameraMgr's singleton slot.
const CAMERA_MGR_SLOT: GuestPtr<GuestPtr<GuestPtr<CameraMgr>>> = GuestPtr::new(0x84ca_6bc0);
/// EventMgr's singleton slot.
const EVENT_MGR_SLOT: GuestPtr<GuestPtr<GuestPtr<EventMgr>>> = GuestPtr::new(0x84c9_e090);
const CAMERA_VIEWS: u64 = 0x18;
const VIEWS_GAMEPLAY: u64 = 8;
const MGR_COUNT: u64 = 32;
const MGR_CAMERAS: u64 = 40;
const MGR_CAMERA_VIEW: u64 = 120;
const EVENT_RUNNING: u64 = 104;

/// The percentages the row has: 100 changes nothing, others are no row's.
const PERCENT: std::ops::RangeInclusive<i32> = 50..=150;
/// The vertical lens's bounds, in radians: 5 and 140 degrees, even during an extreme zoom (the C's 0.087266463f and
/// 2.443460953f, the same floats written to f32's own digits).
const LENS_MIN: f32 = 0.087_266_46;
const LENS_MAX: f32 = 2.443_461;
/// π (the C's 3.141592654f, the same float): an angle at or above it is no lens.
const HALF_TURN: f32 = std::f32::consts::PI;

/// The site's callback (game/sites.def names it for `camera.lens`): x19 the camera, s0 the angle, which it scales.
// (no_mangle: the symbol the site's slot is set to, as the C's was; the only attribute of the kind in this crate)
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn hos_oot_fov(c: Option<&mut Ctx>, camera: u64) {
    if let Some(c) = c {
        fov(&mut GuestCtx::new(c), GuestPtr::new(camera));
    }
}

/// The angle in s0 scaled by the row's percentage, when `camera` is the gameplay view of camera zero and no event runs.
pub fn fov(g: &mut GuestCtx<'_>, camera: GuestPtr<Camera>) {
    let percent = guest::host::option(options::FOV, 100);
    if percent == 100 || !PERCENT.contains(&percent) || camera.is_null() {
        return;
    }
    let Some(view) = gameplay_view(g, camera) else { return };
    if !is_main_camera_view(g, view) || event_running(g) {
        return;
    }
    let angle = g.vreg_f32(0);
    if !angle.is_finite() || angle <= 0.0 || angle >= HALF_TURN {
        return;
    }
    // (clamp is fminf(max, fmaxf(min, x)) for a finite x, as here: the C's)
    g.set_vreg_f32(0, (angle * (percent as f32 / 100.0)).clamp(LENS_MIN, LENS_MAX));
}

/// The camera's gameplay view, when it is a `sead::LookAtCamera`.
fn gameplay_view(g: &mut GuestCtx<'_>, camera: GuestPtr<Camera>) -> Option<GuestPtr<View>> {
    let views = g.read_ptr(camera.field::<GuestPtr<Views>>(CAMERA_VIEWS))?;
    if views.is_null() {
        return None;
    }
    let view = g.read_ptr(views.field::<GuestPtr<View>>(VIEWS_GAMEPLAY))?;
    if view.is_null() || g.read(view.cast::<u64>())? != LOOK_AT_CAMERA_VTABLE {
        return None;
    }
    Some(view)
}

/// Whether `view` is CameraMgr's first camera's: it supplies the gameplay view (oot_tools.c's free view uses the same
/// ownership check). Portraits, reflections and UI stay alone.
fn is_main_camera_view(g: &mut GuestCtx<'_>, view: GuestPtr<View>) -> bool {
    let first = (|| {
        let holder = g.read_ptr(CAMERA_MGR_SLOT)?;
        let manager = if holder.is_null() { return None } else { g.read_ptr(holder)? };
        if manager.is_null() || g.read(manager.field::<u32>(MGR_COUNT))? == 0 {
            return None;
        }
        let cameras = g.read_ptr(manager.field::<GuestPtr<GuestPtr<MgrCamera>>>(MGR_CAMERAS))?;
        if cameras.is_null() {
            return None;
        }
        let main = g.read_ptr(cameras)?;
        if main.is_null() { None } else { g.read_ptr(main.field::<GuestPtr<View>>(MGR_CAMERA_VIEW)) }
    })();
    first == Some(view)
}

/// Whether an event runs (EventMgr's word at +104): its camera keeps its authored angle.
fn event_running(g: &mut GuestCtx<'_>) -> bool {
    (|| {
        let holder = g.read_ptr(EVENT_MGR_SLOT)?;
        let events = if holder.is_null() { return Some(false) } else { g.read_ptr(holder)? };
        if events.is_null() {
            return Some(false);
        }
        Some(g.read(events.field::<u64>(EVENT_RUNNING))? != 0)
    })()
    // (a slot that cannot be read: as an event, the angle left alone)
    .unwrap_or(true)
}
