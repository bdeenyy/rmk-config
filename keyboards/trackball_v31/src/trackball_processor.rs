/// Trackball processor for Trackball Mini v3.1 / v3.0.
///
/// Mode switching handled entirely here (NO LT/TH in keyboard.toml):
/// - MouseBtn1 (RMK)  → normal click + cursor
/// - User12 (hold)    → Sniper mode (slow cursor)
/// - User12 (tap)     → MB2 click (right-click) — sent from our code
/// - MB1 + User12     → Scroll mode (trackball = wheel)
/// - MB1 + User12 tap → MB3 click (middle button)
///
/// keyboard.toml uses: MouseBtn1, User12 (no MouseBtn2 to avoid RMK auto-sending it)
///
/// ## Runtime settings via User keycodes:
/// - User8  = Scroll divisor +1
/// - User9  = Scroll divisor -1
/// - User10 = Sniper divisor +1
/// - User11 = Sniper divisor -1
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use rmk::embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use rmk::channel::{CONTROLLER_CHANNEL, KEYBOARD_REPORT_CHANNEL};
use rmk::event::Event;
use rmk::hid::Report;
use rmk::input_device::{InputProcessor, ProcessResult};
use rmk::keymap::KeyMap;
use usbd_hid::descriptor::MouseReport;

// Timing constants
const COMBO_WINDOW_MS: u32 = 100;
const COMBO_TAP_MS: u32 = 250;

// Default scroll divisor (higher = slower).
const SCROLL_DIVISOR_DEFAULT: u32 = 5;
const SCROLL_DIVISOR_MIN: u32 = 1;
const SCROLL_DIVISOR_MAX: u32 = 32;

// Default sniper divisor.
const SNIPER_DIVISOR_DEFAULT: u32 = 4;
const SNIPER_DIVISOR_MIN: u32 = 1;
const SNIPER_DIVISOR_MAX: u32 = 16;

// Normal mode report interval: 16ms ≈ 62Hz
const NORMAL_REPORT_INTERVAL_MS: u32 = 16;

/// Current mouse button bitmask for HID reports (MB1 via RMK, MB2/MB3 via our code)
static MOUSE_BUTTONS: AtomicU8 = AtomicU8::new(0);

/// Mode flags (set by tick task, read by processor)
static MODE_SCROLL: AtomicBool = AtomicBool::new(false);
static MODE_SNIPER: AtomicBool = AtomicBool::new(false);

/// Runtime-adjustable divisors
static SCROLL_DIVISOR: AtomicU32 = AtomicU32::new(SCROLL_DIVISOR_DEFAULT);
static SNIPER_DIVISOR: AtomicU32 = AtomicU32::new(SNIPER_DIVISOR_DEFAULT);

/// Scroll accumulators
static SCROLL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static SCROLL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);

/// Normal mode accumulators + timestamp
static NORMAL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static NORMAL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);
static LAST_NORMAL_REPORT_MS: AtomicU32 = AtomicU32::new(0);

// AtomicI32 via AtomicU32 bit-cast
struct AtomicI32(core::sync::atomic::AtomicU32);
impl AtomicI32 {
    const fn new(v: i32) -> Self {
        Self(core::sync::atomic::AtomicU32::new(v as u32))
    }
    fn load(&self, ord: Ordering) -> i32 {
        self.0.load(ord) as i32
    }
    fn store(&self, v: i32, ord: Ordering) {
        self.0.store(v as u32, ord);
    }
    fn fetch_add(&self, v: i32, ord: Ordering) -> i32 {
        self.0.fetch_add(v as u32, ord) as i32
    }
}

fn now_ms() -> u32 {
    (Instant::now().as_ticks() / (embassy_time::TICK_HZ / 1000)) as u32
}

/// Handle User keycodes for runtime divisor adjustment.
pub fn handle_user_keycode(keycode_idx: u8) {
    match keycode_idx {
        8 => {
            let v = (SCROLL_DIVISOR.load(Ordering::Relaxed) + 1).min(SCROLL_DIVISOR_MAX);
            SCROLL_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Scroll divisor: {}", v);
        }
        9 => {
            let v = SCROLL_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SCROLL_DIVISOR_MIN);
            SCROLL_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Scroll divisor: {}", v);
        }
        10 => {
            let v = (SNIPER_DIVISOR.load(Ordering::Relaxed) + 1).min(SNIPER_DIVISOR_MAX);
            SNIPER_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Sniper divisor: {}", v);
        }
        11 => {
            let v = SNIPER_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SNIPER_DIVISOR_MIN);
            SNIPER_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Sniper divisor: {}", v);
        }
        _ => {}
    }
}

/// Main trackball InputProcessor.
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
                    wheel: 0,
                    pan: 0,
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
                    wheel: 0,
                    pan: 0,
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

/// Send an MB2 press+release cycle via HID.
async fn send_mb2_click() {
    defmt::info!("Sending MB2 click");
    let press = MouseReport { buttons: 0b00000010, x: 0, y: 0, wheel: 0, pan: 0 };
    let release = MouseReport { buttons: 0, x: 0, y: 0, wheel: 0, pan: 0 };
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(press)).await;
    Timer::after(Duration::from_millis(10)).await;
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(release)).await;
}

/// Send an MB3 press+release cycle via HID.
async fn send_mb3_click() {
    defmt::info!("Sending MB3 click");
    let press = MouseReport { buttons: 0b00000100, x: 0, y: 0, wheel: 0, pan: 0 };
    let release = MouseReport { buttons: 0, x: 0, y: 0, wheel: 0, pan: 0 };
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(press)).await;
    Timer::after(Duration::from_millis(10)).await;
    KEYBOARD_REPORT_CHANNEL.send(Report::MouseReport(release)).await;
}

/// Background task: button-driven mode switching + combo detection.
///
/// Button mapping (User12 is the "right button" keycode):
/// - User12 hold alone  → Sniper mode (slow cursor, NO right-click sent)
/// - User12 tap alone   → MB2 click (right-click)
/// - MB1 + User12 hold  → Scroll mode
/// - MB1 + User12 tap   → MB3 click (middle-button)
/// - MB1 alone          → normal click (handled by RMK)
#[embassy_executor::task]
pub async fn trackball_tick_task() {
    let mut sub = defmt::unwrap!(CONTROLLER_CHANNEL.subscriber());

    // Button state tracking
    let mut mb1_held = false;
    let mut mb2_held = false;
    let mut mb1_press_time: u32 = 0;
    let mut mb2_press_time: u32 = 0;
    let mut combo_active = false;
    let mut combo_start_time: u32 = 0;

    loop {
        match select(
            Timer::after(Duration::from_millis(500)),
            sub.next_message_pure(),
        )
        .await
        {
            Either::First(_) => {
                // Periodic cleanup
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

                            match kc {
                                KeyCode::MouseBtn1 => {
                                    // Toggle: RMK sends this on both press and release
                                    mb1_held = !mb1_held;

                                    if mb1_held {
                                        // MB1 pressed
                                        mb1_press_time = now;
                                        MOUSE_BUTTONS.store(
                                            MOUSE_BUTTONS.load(Ordering::Relaxed) | (1 << 0),
                                            Ordering::Relaxed,
                                        );
                                        // Check combo: MB2 already held?
                                        if mb2_held && now.wrapping_sub(mb2_press_time) < COMBO_WINDOW_MS {
                                            combo_active = true;
                                            combo_start_time = now;
                                            MODE_SCROLL.store(true, Ordering::Relaxed);
                                            MODE_SNIPER.store(false, Ordering::Relaxed);
                                            SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                            SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                                            defmt::info!("Combo: Scroll ON");
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
                                                // Short combo → MB3
                                                send_mb3_click().await;
                                            }
                                            // Back to sniper if MB2 still held
                                            if mb2_held {
                                                MODE_SNIPER.store(true, Ordering::Relaxed);
                                            }
                                            defmt::info!("Combo: Scroll OFF");
                                        }
                                    }
                                }
                                KeyCode::User12 => {
                                    // "Right button" — handled entirely by us (no RMK HID)
                                    mb2_held = !mb2_held;

                                    if mb2_held {
                                        // User12 pressed
                                        mb2_press_time = now;
                                        // Check combo: MB1 already held?
                                        if mb1_held && now.wrapping_sub(mb1_press_time) < COMBO_WINDOW_MS {
                                            combo_active = true;
                                            combo_start_time = now;
                                            MODE_SCROLL.store(true, Ordering::Relaxed);
                                            MODE_SNIPER.store(false, Ordering::Relaxed);
                                            SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                            SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                                            defmt::info!("Combo: Scroll ON");
                                        } else {
                                            // Solo User12 → Sniper mode
                                            MODE_SNIPER.store(true, Ordering::Relaxed);
                                            defmt::info!("Sniper ON");
                                        }
                                    } else {
                                        // User12 released
                                        if combo_active {
                                            let held = now.wrapping_sub(combo_start_time);
                                            combo_active = false;
                                            MODE_SCROLL.store(false, Ordering::Relaxed);
                                            SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                                            SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);

                                            if held < COMBO_TAP_MS {
                                                send_mb3_click().await;
                                            }
                                            // Back to sniper if MB1 still held? No — MB1 is for scrolling
                                            if mb2_held {
                                                MODE_SNIPER.store(true, Ordering::Relaxed);
                                            }
                                            defmt::info!("Combo: Scroll OFF");
                                        } else {
                                            // Solo User12 released
                                            let held = now.wrapping_sub(mb2_press_time);
                                            MODE_SNIPER.store(false, Ordering::Relaxed);
                                            defmt::info!("Sniper OFF");

                                            // If tap (short press) → send MB2 click
                                            if held < COMBO_TAP_MS {
                                                send_mb2_click().await;
                                            }
                                            // If hold → no MB2 sent, was just sniper
                                        }
                                    }
                                }
                                KeyCode::User8 => {
                                    let v = (SCROLL_DIVISOR.load(Ordering::Relaxed) + 1).min(SCROLL_DIVISOR_MAX);
                                    SCROLL_DIVISOR.store(v, Ordering::Relaxed);
                                    defmt::info!("Scroll divisor: {}", v);
                                }
                                KeyCode::User9 => {
                                    let v = SCROLL_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SCROLL_DIVISOR_MIN);
                                    SCROLL_DIVISOR.store(v, Ordering::Relaxed);
                                    defmt::info!("Scroll divisor: {}", v);
                                }
                                KeyCode::User10 => {
                                    let v = (SNIPER_DIVISOR.load(Ordering::Relaxed) + 1).min(SNIPER_DIVISOR_MAX);
                                    SNIPER_DIVISOR.store(v, Ordering::Relaxed);
                                    defmt::info!("Sniper divisor: {}", v);
                                }
                                KeyCode::User11 => {
                                    let v = SNIPER_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SNIPER_DIVISOR_MIN);
                                    SNIPER_DIVISOR.store(v, Ordering::Relaxed);
                                    defmt::info!("Sniper divisor: {}", v);
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
