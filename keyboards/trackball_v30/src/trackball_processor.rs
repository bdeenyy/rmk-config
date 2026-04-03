/// Trackball processor for Trackball Mini v3.0 (2 buttons only).
///
/// Mode switching handled entirely here:
/// - MouseBtn1 (RMK)  → normal click + cursor
/// - User12 (hold)    → Sniper mode (slow cursor)
/// - User12 (tap)     → MB2 click (right-click)
/// - MB1 + User12 hold → Scroll mode (trackball = wheel)
/// - MB1 + User12 tap  → MB3 click (middle button)
/// - MB1 + User12 double-tap → Adjust mode (BT controls)
///
/// In Adjust mode:
/// - MB1 tap → BT0 (select profile 0)  → handled as User0 by RMK
/// - User12 tap → BT Next              → handled as User3 by RMK
/// - Both buttons combo → exit Adjust mode (back to Base)
///
/// ZMK equivalent: adj_td tap-dance + combos on layers 0-3
///
/// ## Runtime settings via User keycodes:
/// - User8-11 = scroll/sniper divisor adjustment
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use rmk::embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use rmk::channel::{CONTROLLER_CHANNEL, KEY_EVENT_CHANNEL, KEYBOARD_REPORT_CHANNEL};
use rmk::event::{Event, KeyboardEvent};
use rmk::hid::Report;
use rmk::input_device::{InputProcessor, ProcessResult};
use rmk::keymap::KeyMap;
use usbd_hid::descriptor::MouseReport;

// Timing constants
const COMBO_WINDOW_MS: u32 = 100;
const COMBO_TAP_MS: u32 = 250;
const DOUBLE_TAP_MS: u32 = 400; // window for second combo tap to trigger Adjust

// Default divisors
const SCROLL_DIVISOR_DEFAULT: u32 = 5;
const SCROLL_DIVISOR_MIN: u32 = 1;
const SCROLL_DIVISOR_MAX: u32 = 32;
const SNIPER_DIVISOR_DEFAULT: u32 = 4;
const SNIPER_DIVISOR_MIN: u32 = 1;
const SNIPER_DIVISOR_MAX: u32 = 16;

const NORMAL_REPORT_INTERVAL_MS: u32 = 16;

/// Current mouse button bitmask for HID reports
static MOUSE_BUTTONS: AtomicU8 = AtomicU8::new(0);

/// Mode flags
static MODE_SCROLL: AtomicBool = AtomicBool::new(false);
static MODE_SNIPER: AtomicBool = AtomicBool::new(false);
static MODE_ADJUST: AtomicBool = AtomicBool::new(false);

/// Runtime-adjustable divisors
static SCROLL_DIVISOR: AtomicU32 = AtomicU32::new(SCROLL_DIVISOR_DEFAULT);
static SNIPER_DIVISOR: AtomicU32 = AtomicU32::new(SNIPER_DIVISOR_DEFAULT);

/// Scroll accumulators
static SCROLL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static SCROLL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);
static NORMAL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static NORMAL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);
static LAST_NORMAL_REPORT_MS: AtomicU32 = AtomicU32::new(0);

struct AtomicI32(core::sync::atomic::AtomicU32);
impl AtomicI32 {
    const fn new(v: i32) -> Self { Self(core::sync::atomic::AtomicU32::new(v as u32)) }
    fn load(&self, ord: Ordering) -> i32 { self.0.load(ord) as i32 }
    fn store(&self, v: i32, ord: Ordering) { self.0.store(v as u32, ord); }
    fn fetch_add(&self, v: i32, ord: Ordering) -> i32 { self.0.fetch_add(v as u32, ord) as i32 }
}

fn now_ms() -> u32 {
    (Instant::now().as_ticks() / (embassy_time::TICK_HZ / 1000)) as u32
}

pub struct TrackballProcessor<
    'a,
    const ROW: usize,
    const COL: usize,
    const NUM_LAYER: usize,
    const NUM_ENCODER: usize = 0,
> {
    keymap: &'a RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>>,
}

impl<'a, const ROW: usize, const COL: usize, const NUM_LAYER: usize, const NUM_ENCODER: usize>
    TrackballProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
{
    pub fn new(keymap: &'a RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>>) -> Self {
        Self { keymap }
    }
}

impl<'a, const ROW: usize, const COL: usize, const NUM_LAYER: usize, const NUM_ENCODER: usize>
    InputProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
    for TrackballProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
{
    async fn process(&mut self, event: Event) -> ProcessResult {
        let Event::Joystick(axes) = event else {
            return ProcessResult::Continue(event);
        };

        let mut dx: i16 = 0;
        let mut dy: i16 = 0;
        for axis in axes.iter() {
            match axis.axis {
                rmk::event::Axis::X => dx = axis.value,
                rmk::event::Axis::Y => dy = axis.value,
                _ => {}
            }
        }

        let scroll = MODE_SCROLL.load(Ordering::Relaxed);
        let sniper = MODE_SNIPER.load(Ordering::Relaxed);

        if scroll {
            let divisor = SCROLL_DIVISOR.load(Ordering::Relaxed) as i32;
            let acc_x = SCROLL_ACCUM_X.fetch_add(dx as i32, Ordering::Relaxed) + dx as i32;
            let acc_y = SCROLL_ACCUM_Y.fetch_add(dy as i32, Ordering::Relaxed) + dy as i32;
            let wheel = -(acc_y / divisor) as i8;
            let pan = (acc_x / divisor) as i8;
            if wheel != 0 || pan != 0 {
                SCROLL_ACCUM_X.store(acc_x % divisor, Ordering::Relaxed);
                SCROLL_ACCUM_Y.store(acc_y % divisor, Ordering::Relaxed);
                let report = MouseReport { buttons: 0, x: 0, y: 0, wheel, pan };
                KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(report)).await;
            }
            return ProcessResult::Stop;
        }

        if sniper {
            let divisor = SNIPER_DIVISOR.load(Ordering::Relaxed) as i16;
            let slow_dx = dx / divisor;
            let slow_dy = dy / divisor;
            if slow_dx != 0 || slow_dy != 0 {
                let report = MouseReport {
                    buttons: MOUSE_BUTTONS.load(Ordering::Relaxed),
                    x: slow_dx.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    y: slow_dy.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    wheel: 0, pan: 0,
                };
                KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(report)).await;
            }
            return ProcessResult::Stop;
        }

        // Normal mode
        NORMAL_ACCUM_X.fetch_add(dx as i32, Ordering::Relaxed);
        NORMAL_ACCUM_Y.fetch_add(dy as i32, Ordering::Relaxed);
        let now = now_ms();
        let last = LAST_NORMAL_REPORT_MS.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= NORMAL_REPORT_INTERVAL_MS {
            let acc_x = NORMAL_ACCUM_X.load(Ordering::Relaxed);
            let acc_y = NORMAL_ACCUM_Y.load(Ordering::Relaxed);
            if acc_x != 0 || acc_y != 0 {
                NORMAL_ACCUM_X.store(0, Ordering::Relaxed);
                NORMAL_ACCUM_Y.store(0, Ordering::Relaxed);
                LAST_NORMAL_REPORT_MS.store(now, Ordering::Relaxed);
                let report = MouseReport {
                    buttons: MOUSE_BUTTONS.load(Ordering::Relaxed),
                    x: acc_x.clamp(i8::MIN as i32, i8::MAX as i32) as i8,
                    y: acc_y.clamp(i8::MIN as i32, i8::MAX as i32) as i8,
                    wheel: 0, pan: 0,
                };
                KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(report)).await;
            }
        }
        ProcessResult::Stop
    }

    fn get_keymap(&self) -> &RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>> {
        self.keymap
    }
}

/// Send a synthetic key press+release for a virtual position.
/// This triggers the keymap action at (row, col) on the currently active layer.
/// Used for BT actions on virtual columns (col 2-3) in Adjust mode.
async fn send_virtual_key(row: u8, col: u8) {
    KEY_EVENT_CHANNEL.send(KeyboardEvent::key(row, col, true)).await;
    Timer::after(Duration::from_millis(50)).await;
    KEY_EVENT_CHANNEL.send(KeyboardEvent::key(row, col, false)).await;
}

async fn send_mb2_click() {
    let press = MouseReport { buttons: 0b00000010, x: 0, y: 0, wheel: 0, pan: 0 };
    let release = MouseReport { buttons: 0, x: 0, y: 0, wheel: 0, pan: 0 };
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(press)).await;
    Timer::after(Duration::from_millis(10)).await;
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(release)).await;
}

async fn send_mb3_click() {
    let press = MouseReport { buttons: 0b00000100, x: 0, y: 0, wheel: 0, pan: 0 };
    let release = MouseReport { buttons: 0, x: 0, y: 0, wheel: 0, pan: 0 };
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(press)).await;
    Timer::after(Duration::from_millis(10)).await;
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(release)).await;
}

/// Background task: button-driven mode switching + combo + double-tap for Adjust.
///
/// Combo (both buttons) behavior:
///   - 1st tap → MB3 click
///   - 1st hold → Scroll mode
///   - 2nd tap (within DOUBLE_TAP_MS of 1st) → enter Adjust mode
///
/// In Adjust mode:
///   - MB1 tap → User0 action (BT0) — already in keyboard.toml Adjust layer
///   - User12 tap → User3 action (BT Next) — already in keyboard.toml
///   - Both buttons combo → exit Adjust (back to normal)
///
/// Note: Adjust mode uses software flag, not RMK layer switch (since
/// set_default_layer is pub(crate)). BT actions are triggered via
/// KEY_EVENT_CHANNEL by synthesizing key events for the Adjust layer.
#[embassy_executor::task]
pub async fn trackball_tick_task() {
    let mut sub = defmt::unwrap!(CONTROLLER_CHANNEL.subscriber());

    let mut mb1_held = false;
    let mut mb2_held = false;
    let mut mb1_press_time: u32 = 0;
    let mut mb2_press_time: u32 = 0;
    let mut combo_active = false;
    let mut combo_start_time: u32 = 0;

    // Double-tap tracking for Adjust mode
    let mut last_combo_tap_time: u32 = 0;

    loop {
        match select(
            Timer::after(Duration::from_millis(500)),
            sub.next_message_pure(),
        )
        .await
        {
            Either::First(_) => {
                if !MODE_SCROLL.load(Ordering::Relaxed) {
                    SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                    SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                }
            }
            Either::Second(event) => {
                use rmk::event::ControllerEvent;
                match event {
                    ControllerEvent::Key(_key_event, action) => {
                        use rmk::types::action::{Action, KeyAction};
                        use rmk::types::keycode::KeyCode;

                        if let KeyAction::Single(Action::Key(kc)) = action {
                            let now = now_ms();
                            let in_adjust = MODE_ADJUST.load(Ordering::Relaxed);

                            match kc {
                                KeyCode::MouseBtn1 => {
                                    mb1_held = !mb1_held;

                                    if mb1_held {
                                        mb1_press_time = now;
                                        if !in_adjust {
                                            MOUSE_BUTTONS.store(
                                                MOUSE_BUTTONS.load(Ordering::Relaxed) | (1 << 0),
                                                Ordering::Relaxed,
                                            );
                                        }
                                        // Check combo
                                        if mb2_held && now.wrapping_sub(mb2_press_time) < COMBO_WINDOW_MS {
                                            if in_adjust {
                                                // In Adjust: combo → exit Adjust
                                                MODE_ADJUST.store(false, Ordering::Relaxed);
                                                defmt::info!("Adjust OFF → Base");
                                            } else {
                                                combo_active = true;
                                                combo_start_time = now;
                                                MODE_SCROLL.store(true, Ordering::Relaxed);
                                                MODE_SNIPER.store(false, Ordering::Relaxed);
                                                SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                                SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                                                defmt::info!("Combo: Scroll ON");
                                            }
                                        }
                                    } else {
                                        // MB1 released
                                        MOUSE_BUTTONS.store(
                                            MOUSE_BUTTONS.load(Ordering::Relaxed) & !(1 << 0),
                                            Ordering::Relaxed,
                                        );
                                        if combo_active {
                                            let held = now.wrapping_sub(combo_start_time);
                                            combo_active = false;
                                            MODE_SCROLL.store(false, Ordering::Relaxed);
                                            SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                            SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);

                                            if held < COMBO_TAP_MS {
                                                // Check double-tap → Adjust
                                                if now.wrapping_sub(last_combo_tap_time) < DOUBLE_TAP_MS {
                                                    MODE_ADJUST.store(true, Ordering::Relaxed);
                                                    defmt::info!("Double-tap: Adjust ON");
                                                    last_combo_tap_time = 0; // reset
                                                } else {
                                                    // Single combo tap → MB3
                                                    send_mb3_click().await;
                                                    last_combo_tap_time = now;
                                                }
                                            } else {
                                                last_combo_tap_time = 0; // held, not tap
                                            }

                                            if mb2_held && !MODE_ADJUST.load(Ordering::Relaxed) {
                                                MODE_SNIPER.store(true, Ordering::Relaxed);
                                            }
                                            defmt::info!("Combo: Scroll OFF");
                                        } else if in_adjust {
                                            // In Adjust: MB1 tap → BT0 (User0) via virtual key (0,2)
                                            let held = now.wrapping_sub(mb1_press_time);
                                            if held < COMBO_TAP_MS {
                                                defmt::info!("Adjust: BT0 (User0)");
                                                send_virtual_key(1, 0).await;
                                            }
                                        }
                                    }
                                }
                                KeyCode::User12 => {
                                    mb2_held = !mb2_held;

                                    if mb2_held {
                                        mb2_press_time = now;
                                        if mb1_held && now.wrapping_sub(mb1_press_time) < COMBO_WINDOW_MS {
                                            if in_adjust {
                                                MODE_ADJUST.store(false, Ordering::Relaxed);
                                                defmt::info!("Adjust OFF → Base");
                                            } else {
                                                combo_active = true;
                                                combo_start_time = now;
                                                MODE_SCROLL.store(true, Ordering::Relaxed);
                                                MODE_SNIPER.store(false, Ordering::Relaxed);
                                                SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                                SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                                                defmt::info!("Combo: Scroll ON");
                                            }
                                        } else if !in_adjust {
                                            MODE_SNIPER.store(true, Ordering::Relaxed);
                                            defmt::info!("Sniper ON");
                                        }
                                    } else {
                                        if combo_active {
                                            let held = now.wrapping_sub(combo_start_time);
                                            combo_active = false;
                                            MODE_SCROLL.store(false, Ordering::Relaxed);
                                            SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                            SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);

                                            if held < COMBO_TAP_MS {
                                                if now.wrapping_sub(last_combo_tap_time) < DOUBLE_TAP_MS {
                                                    MODE_ADJUST.store(true, Ordering::Relaxed);
                                                    defmt::info!("Double-tap: Adjust ON");
                                                    last_combo_tap_time = 0;
                                                } else {
                                                    send_mb3_click().await;
                                                    last_combo_tap_time = now;
                                                }
                                            } else {
                                                last_combo_tap_time = 0;
                                            }

                                            if mb2_held && !MODE_ADJUST.load(Ordering::Relaxed) {
                                                MODE_SNIPER.store(true, Ordering::Relaxed);
                                            }
                                            defmt::info!("Combo: Scroll OFF");
                                        } else if in_adjust {
                                            // Adjust: User12 tap → BT Next (User3) via virtual key (0,3)
                                            let held = now.wrapping_sub(mb2_press_time);
                                            if held < COMBO_TAP_MS {
                                                defmt::info!("Adjust: BT Next (User3)");
                                                send_virtual_key(1, 1).await;
                                            }
                                        } else {
                                            let held = now.wrapping_sub(mb2_press_time);
                                            MODE_SNIPER.store(false, Ordering::Relaxed);
                                            defmt::info!("Sniper OFF");
                                            if held < COMBO_TAP_MS {
                                                send_mb2_click().await;
                                            }
                                        }
                                    }
                                }
                                KeyCode::User0 | KeyCode::User3 | KeyCode::User5 | KeyCode::User6 => {
                                    // BT actions — pass through (handled by RMK natively)
                                }
                                KeyCode::User8 => {
                                    let v = (SCROLL_DIVISOR.load(Ordering::Relaxed) + 1).min(SCROLL_DIVISOR_MAX);
                                    SCROLL_DIVISOR.store(v, Ordering::Relaxed);
                                }
                                KeyCode::User9 => {
                                    let v = SCROLL_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SCROLL_DIVISOR_MIN);
                                    SCROLL_DIVISOR.store(v, Ordering::Relaxed);
                                }
                                KeyCode::User10 => {
                                    let v = (SNIPER_DIVISOR.load(Ordering::Relaxed) + 1).min(SNIPER_DIVISOR_MAX);
                                    SNIPER_DIVISOR.store(v, Ordering::Relaxed);
                                }
                                KeyCode::User11 => {
                                    let v = SNIPER_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SNIPER_DIVISOR_MIN);
                                    SNIPER_DIVISOR.store(v, Ordering::Relaxed);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
