//! Home gesture: swiping up from the bottom edge of the screen shows the desk.
//!
//! The gesture is recognised by where a press starts, not by what is under the
//! finger, so it works over any application. A press that begins in a band
//! along the bottom edge is a candidate. Once the finger has moved up past a
//! small slop the shell claims it: the application's press is cancelled (it
//! receives no click, scroll or long-press) and the pill follows the finger.
//! A tap or a sideways move in the band is left to the application.
//!
//! LVGL sends only `PRESSED`, `RELEASED` and `INDEV_RESET` to input device
//! callbacks, so a timer that runs only while a band press is active follows
//! the finger between them. A fast flick (press, one move, release) is judged
//! from the release point and never depends on that timer.

use core::ptr::null_mut;

use xila::graphics::{self, lvgl};

use crate::{desk::Desk, error::Result};

/// Opacity of the pill (70 %), a little above the previous 50 % so it reads as
/// a cue on any application.
const PILL_OPACITY: u8 = 178;

/// Sampling period of the timer that follows the finger, in milliseconds.
const FOLLOW_PERIOD: u32 = 16;

/// Gesture sizes in pixels, derived from the display density so they keep the
/// same physical size on a small panel and in a large desktop window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Metrics {
    /// Height of the touch band along the bottom edge, full screen width.
    band: i32,
    /// Upward movement after which the shell claims the press.
    slop: i32,
    /// Upward displacement from the press to the release that opens the desk.
    travel: i32,
    /// Distance between the pill and the bottom edge when idle.
    rest: i32,
    /// Largest upward offset of the pill while following the finger.
    lift: i32,
    /// Height of the pill.
    pill: i32,
}

impl Metrics {
    fn new(dpx: impl Fn(i32) -> i32) -> Self {
        Self {
            band: dpx(24),
            slop: dpx(8),
            travel: dpx(20),
            rest: dpx(5),
            lift: dpx(48),
            pill: dpx(7),
        }
    }

    fn current() -> Self {
        // SAFETY: called with the graphics lock held, with a default display.
        Self::new(|n| unsafe { lvgl::lv_dpx(n) })
    }
}

/// How far a movement from the press point has progressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Swipe {
    /// Not an upward swipe (yet).
    None,
    /// Upward past the slop, short of the release distance.
    Started,
    /// Upward far enough to open the desk when released here.
    Complete,
}

fn classify(dx: i32, dy: i32, metrics: &Metrics) -> Swipe {
    let up = -dy;
    // Mostly vertical, so a diagonal or sideways drag in the band stays with
    // the application.
    if up < metrics.slop || up < dx.abs() {
        Swipe::None
    } else if up >= metrics.travel {
        Swipe::Complete
    } else {
        Swipe::Started
    }
}

fn in_band(y: i32, height: i32, metrics: &Metrics) -> bool {
    y >= height - metrics.band
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Press started in the band; the application still owns it.
    Tracking,
    /// The shell took the press away from the application.
    Claimed,
}

/// What a release means for the press that just ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Release {
    /// Not a home swipe: the application handles the release normally.
    Pass,
    /// A home swipe that stopped short, or was dragged back: swallow the press.
    Cancel,
    Home,
}

/// Pill offset requested by a claimed gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Claim {
    /// True on the call that claimed the press: cancel the application's press.
    first: bool,
    /// Distance the pill follows the finger up, within `Metrics::lift`.
    lift: i32,
}

#[derive(Debug, Default)]
struct Gesture {
    phase: Phase,
    start: (i32, i32),
    metrics: Metrics,
}

impl Gesture {
    fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    fn is_tracking(&self) -> bool {
        self.phase == Phase::Tracking
    }

    /// Starts tracking when the press is inside the band.
    fn press(&mut self, x: i32, y: i32, height: i32, metrics: Metrics) -> bool {
        self.phase = Phase::Idle;
        if !in_band(y, height, &metrics) {
            return false;
        }
        self.phase = Phase::Tracking;
        self.start = (x, y);
        self.metrics = metrics;
        true
    }

    fn motion(&mut self, x: i32, y: i32) -> Option<Claim> {
        let (dx, dy) = (x - self.start.0, y - self.start.1);
        let first = match self.phase {
            Phase::Idle => return None,
            Phase::Tracking if classify(dx, dy, &self.metrics) == Swipe::None => return None,
            Phase::Tracking => true,
            Phase::Claimed => false,
        };
        self.phase = Phase::Claimed;
        Some(Claim {
            first,
            lift: (-dy).clamp(0, self.metrics.lift),
        })
    }

    fn release(&mut self, x: i32, y: i32) -> Release {
        let phase = core::mem::take(&mut self.phase);
        let swipe = classify(x - self.start.0, y - self.start.1, &self.metrics);
        match (phase, swipe) {
            (Phase::Idle, _) | (Phase::Tracking, Swipe::None) => Release::Pass,
            (_, Swipe::Complete) => Release::Home,
            _ => Release::Cancel,
        }
    }

    fn cancel(&mut self) {
        self.phase = Phase::Idle;
    }
}

/// Gesture state shared with the LVGL callbacks. Only accessed with the
/// graphics lock held, from callbacks and from `Home`.
struct State {
    desk: *mut lvgl::lv_obj_t,
    pill: *mut lvgl::lv_obj_t,
    timer: *mut lvgl::lv_timer_t,
    /// Input device that owns the gesture in progress.
    indev: *mut lvgl::lv_indev_t,
    gesture: Gesture,
}

impl State {
    unsafe fn point(indev: *mut lvgl::lv_indev_t) -> (i32, i32) {
        let mut point = lvgl::lv_point_t::default();
        unsafe { lvgl::lv_indev_get_point(indev, &mut point) };
        (point.x, point.y)
    }

    /// Puts the pill back and stops following the finger.
    unsafe fn settle(&mut self) {
        unsafe {
            lvgl::lv_timer_pause(self.timer);
            lvgl::lv_obj_set_y(self.pill, -Metrics::current().rest);
        }
    }

    /// Cancels the application's press and ignores the rest of this one.
    unsafe fn take_over(indev: *mut lvgl::lv_indev_t) {
        unsafe {
            lvgl::lv_indev_reset(indev, null_mut());
            lvgl::lv_indev_wait_release(indev);
        }
    }

    unsafe fn open_desk(&self) {
        unsafe {
            lvgl::lv_obj_remove_flag(self.desk, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            lvgl::lv_obj_move_foreground(self.desk);
            lvgl::lv_obj_send_event(self.desk, Desk::HOME_EVENT.into_lvgl_code(), null_mut());
        }
    }

    unsafe fn pressed(&mut self, indev: *mut lvgl::lv_indev_t) {
        if !self.gesture.is_idle() && indev != self.indev {
            return;
        }
        let was_active = !self.gesture.is_idle();
        let (x, y) = unsafe { Self::point(indev) };
        let height =
            unsafe { lvgl::lv_display_get_vertical_resolution(lvgl::lv_indev_get_display(indev)) };
        if self.gesture.press(x, y, height, Metrics::current()) {
            self.indev = indev;
            unsafe { lvgl::lv_timer_resume(self.timer) };
        } else if was_active {
            unsafe { self.settle() };
        }
    }

    /// The finger lifted while the application still owned the press.
    unsafe fn released(&mut self, indev: *mut lvgl::lv_indev_t) {
        if self.gesture.is_idle() || indev != self.indev {
            return;
        }
        let (x, y) = unsafe { Self::point(indev) };
        let release = self.gesture.release(x, y);
        if release != Release::Pass {
            // Drops the application's click along with its press.
            unsafe { lvgl::lv_indev_reset(indev, null_mut()) };
        }
        if release == Release::Home {
            unsafe { self.open_desk() };
        }
        unsafe { self.settle() };
    }

    /// Someone else reset the input device: forget an unclaimed candidate.
    unsafe fn reset(&mut self, indev: *mut lvgl::lv_indev_t) {
        if self.gesture.is_tracking() && indev == self.indev {
            self.gesture.cancel();
            unsafe { self.settle() };
        }
    }

    unsafe fn follow(&mut self) {
        if self.gesture.is_idle() {
            unsafe { lvgl::lv_timer_pause(self.timer) };
            return;
        }
        let indev = self.indev;
        let (x, y) = unsafe { Self::point(indev) };
        if unsafe { lvgl::lv_indev_get_state(indev) }
            == lvgl::lv_indev_state_t_LV_INDEV_STATE_RELEASED
        {
            // A claimed press never reaches the `RELEASED` callback.
            if self.gesture.release(x, y) == Release::Home {
                unsafe { self.open_desk() };
            }
            unsafe { self.settle() };
        } else if let Some(claim) = self.gesture.motion(x, y) {
            unsafe {
                if claim.first {
                    Self::take_over(indev);
                }
                lvgl::lv_obj_set_y(self.pill, -(self.gesture.metrics.rest + claim.lift));
            }
        }
    }
}

pub struct Home {
    graphics_manager: &'static graphics::Manager,
    pill: *mut lvgl::lv_obj_t,
    state: *mut State,
}

impl Drop for Home {
    fn drop(&mut self) {
        let (pill, state) = (self.pill, self.state);
        let _ = self.graphics_manager.lock_function_sync(|| {
            // SAFETY: the graphics lock is held, `Home` owns `pill` on
            // `lv_layer_top()`, the timer and `state`. Removing the callbacks
            // and deleting the timer and pill synchronously guarantees none of
            // them can fire after `Desk` drops or after `state` is freed.
            unsafe {
                for_each_indev(|indev| {
                    lvgl::lv_indev_remove_event_cb_with_user_data(
                        indev,
                        Some(handle_indev),
                        state.cast(),
                    );
                });
                lvgl::lv_timer_delete((*state).timer);
                lvgl::lv_obj_delete(pill);
                drop(alloc::boxed::Box::from_raw(state));
            }
            Ok(())
        });
    }
}

unsafe fn for_each_indev(mut function: impl FnMut(*mut lvgl::lv_indev_t)) {
    unsafe {
        let mut indev = lvgl::lv_indev_get_next(null_mut());
        while !indev.is_null() {
            function(indev);
            indev = lvgl::lv_indev_get_next(indev);
        }
    }
}

impl Home {
    #[cfg(test)]
    pub(crate) fn pill_object(&self) -> *mut lvgl::lv_obj_t {
        self.pill
    }

    pub async fn new(
        graphics_manager: &'static graphics::Manager,
        desk: *mut lvgl::lv_obj_t,
    ) -> Result<Self> {
        let (pill, state) = graphics::lock!(graphics_manager, {
            unsafe {
                let metrics = Metrics::current();
                let pill = lvgl::lv_obj_create(lvgl::lv_layer_top());

                if pill.is_null() {
                    return Err(crate::error::Error::FailedToCreateObject);
                }

                // Only a visual cue: the gesture is recognised from the input
                // device, so the pill never takes a press from the application.
                lvgl::lv_obj_remove_flag(
                    pill,
                    lvgl::lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE
                        | lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE,
                );
                lvgl::lv_obj_set_size(pill, lvgl::lv_pct(28), metrics.pill);
                lvgl::lv_theme_shadcn_set_variant(
                    pill,
                    lvgl::lv_theme_shadcn_variant_t_LV_THEME_SHADCN_VARIANT_DEFAULT,
                );
                lvgl::lv_obj_set_style_bg_opa(pill, PILL_OPACITY, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_align(pill, lvgl::lv_align_t_LV_ALIGN_BOTTOM_MID);
                lvgl::lv_obj_set_y(pill, -metrics.rest);

                let timer = lvgl::lv_timer_create(Some(handle_timer), FOLLOW_PERIOD, null_mut());
                if timer.is_null() {
                    lvgl::lv_obj_delete(pill);
                    return Err(crate::error::Error::FailedToCreateObject);
                }
                lvgl::lv_timer_pause(timer);

                let state = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(State {
                    desk,
                    pill,
                    timer,
                    indev: null_mut(),
                    gesture: Gesture::default(),
                }));
                lvgl::lv_timer_set_user_data(timer, state.cast());

                for_each_indev(|indev| {
                    if lvgl::lv_indev_get_type(indev) != lvgl::lv_indev_type_t_LV_INDEV_TYPE_POINTER
                    {
                        return;
                    }
                    for code in [
                        lvgl::lv_event_code_t_LV_EVENT_PRESSED,
                        lvgl::lv_event_code_t_LV_EVENT_RELEASED,
                        lvgl::lv_event_code_t_LV_EVENT_INDEV_RESET,
                    ] {
                        lvgl::lv_indev_add_event_cb(indev, Some(handle_indev), code, state.cast());
                    }
                });

                (pill, state)
            }
        });

        Ok(Self {
            graphics_manager,
            pill,
            state,
        })
    }
}

unsafe extern "C" fn handle_indev(event: *mut lvgl::lv_event_t) {
    unsafe {
        let state = &mut *(lvgl::lv_event_get_user_data(event) as *mut State);
        let indev = lvgl::lv_event_get_target(event) as *mut lvgl::lv_indev_t;

        match lvgl::lv_event_get_code(event) {
            lvgl::lv_event_code_t_LV_EVENT_PRESSED => state.pressed(indev),
            lvgl::lv_event_code_t_LV_EVENT_RELEASED => state.released(indev),
            lvgl::lv_event_code_t_LV_EVENT_INDEV_RESET => state.reset(indev),
            _ => {}
        }
    }
}

unsafe extern "C" fn handle_timer(timer: *mut lvgl::lv_timer_t) {
    unsafe {
        let state = &mut *(lvgl::lv_timer_get_user_data(timer) as *mut State);
        state.follow();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 130 dpi, LVGL's default density.
    fn dpx(n: i32) -> i32 {
        (130 * n + 80) / 160
    }

    fn metrics() -> Metrics {
        Metrics::new(dpx)
    }

    const HEIGHT: i32 = 480;

    fn pressed(x: i32, y: i32) -> Gesture {
        let mut gesture = Gesture::default();
        assert!(gesture.press(x, y, HEIGHT, metrics()));
        gesture
    }

    #[test]
    fn metrics_scale_with_density() {
        assert_eq!(metrics().band, 20);
        assert_eq!(metrics().slop, 7);
        assert_eq!(metrics().travel, 16);
        let dense = Metrics::new(|n| n * 2);
        assert_eq!(dense.band, 48);
        assert_eq!(dense.travel, 40);
    }

    #[test]
    fn band_covers_the_full_width_at_the_bottom_edge() {
        let metrics = metrics();
        assert!(in_band(HEIGHT - 1, HEIGHT, &metrics));
        assert!(in_band(HEIGHT - metrics.band, HEIGHT, &metrics));
        assert!(!in_band(HEIGHT - metrics.band - 1, HEIGHT, &metrics));
        assert!(!in_band(0, HEIGHT, &metrics));
        let mut gesture = Gesture::default();
        for x in [0, 160, 319] {
            assert!(gesture.press(x, HEIGHT - 1, HEIGHT, metrics));
        }
    }

    #[test]
    fn press_above_the_band_is_never_a_candidate() {
        let mut gesture = Gesture::default();
        assert!(!gesture.press(160, HEIGHT - 100, HEIGHT, metrics()));
        assert!(gesture.is_idle());
        assert_eq!(gesture.motion(160, 0), None);
        assert_eq!(gesture.release(160, 0), Release::Pass);
    }

    #[test]
    fn classify_needs_a_mostly_vertical_upward_move() {
        let metrics = metrics();
        assert_eq!(classify(0, 0, &metrics), Swipe::None);
        assert_eq!(classify(0, -(metrics.slop - 1), &metrics), Swipe::None);
        assert_eq!(classify(0, -metrics.slop, &metrics), Swipe::Started);
        assert_eq!(classify(0, -(metrics.travel - 1), &metrics), Swipe::Started);
        assert_eq!(classify(0, -metrics.travel, &metrics), Swipe::Complete);
        assert_eq!(classify(0, 60, &metrics), Swipe::None);
        assert_eq!(classify(80, 0, &metrics), Swipe::None);
        assert_eq!(classify(-80, -20, &metrics), Swipe::None);
        assert_eq!(classify(10, -30, &metrics), Swipe::Complete);
    }

    #[test]
    fn tap_in_the_band_is_left_to_the_application() {
        let mut gesture = pressed(100, HEIGHT - 5);
        assert_eq!(gesture.motion(101, HEIGHT - 6), None);
        assert!(gesture.is_tracking());
        assert_eq!(gesture.release(101, HEIGHT - 6), Release::Pass);
        assert!(gesture.is_idle());
    }

    #[test]
    fn sideways_move_in_the_band_is_left_to_the_application() {
        let mut gesture = pressed(20, HEIGHT - 5);
        assert_eq!(gesture.motion(150, HEIGHT - 8), None);
        assert_eq!(gesture.release(250, HEIGHT - 8), Release::Pass);
    }

    #[test]
    fn a_fast_flick_is_judged_from_the_release_point() {
        // Press, one move of 30 px, release: no timer tick in between.
        let mut gesture = pressed(160, HEIGHT - 8);
        assert_eq!(gesture.release(160, HEIGHT - 38), Release::Home);
        assert!(gesture.is_idle());
    }

    #[test]
    fn slow_swipe_claims_then_opens_the_desk() {
        let metrics = metrics();
        let mut gesture = pressed(0, HEIGHT - 1);
        assert_eq!(gesture.motion(0, HEIGHT - 4), None);
        assert_eq!(
            gesture.motion(0, HEIGHT - 1 - metrics.slop),
            Some(Claim {
                first: true,
                lift: metrics.slop
            })
        );
        assert_eq!(
            gesture.motion(0, HEIGHT - 1 - metrics.slop - 3),
            Some(Claim {
                first: false,
                lift: metrics.slop + 3
            })
        );
        assert_eq!(gesture.release(0, HEIGHT - 1 - 40), Release::Home);
    }

    #[test]
    fn claimed_pill_is_clamped() {
        let metrics = metrics();
        let mut gesture = pressed(160, HEIGHT - 1);
        let claim = gesture.motion(160, HEIGHT - 1 - 400).unwrap();
        assert_eq!(claim.lift, metrics.lift);
    }

    #[test]
    fn short_or_dragged_back_swipe_cancels_without_opening() {
        let metrics = metrics();
        let mut gesture = pressed(160, HEIGHT - 1);
        assert!(gesture.motion(160, HEIGHT - 1 - metrics.slop).is_some());
        assert_eq!(
            gesture.release(160, HEIGHT - 1 - metrics.slop),
            Release::Cancel
        );

        let mut gesture = pressed(160, HEIGHT - 1);
        assert!(gesture.motion(160, HEIGHT - 1 - 40).is_some());
        // Dragged back down: the application already lost the press.
        assert_eq!(gesture.release(160, HEIGHT - 1), Release::Cancel);
    }

    #[test]
    fn short_move_released_before_a_tick_is_swallowed_not_opened() {
        let metrics = metrics();
        let mut gesture = pressed(160, HEIGHT - 1);
        assert_eq!(
            gesture.release(160, HEIGHT - 1 - metrics.slop),
            Release::Cancel
        );
    }

    #[test]
    fn a_new_press_starts_over() {
        let mut gesture = pressed(160, HEIGHT - 1);
        assert!(gesture.motion(160, HEIGHT - 60).is_some());
        assert!(!gesture.press(160, 100, HEIGHT, metrics()));
        assert!(gesture.is_idle());
        assert!(gesture.press(10, HEIGHT - 2, HEIGHT, metrics()));
        assert!(gesture.is_tracking());
        gesture.cancel();
        assert!(gesture.is_idle());
    }
}
