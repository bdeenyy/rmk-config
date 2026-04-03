/// Auto-mouse layer + Scroll/Sniper mode for velvet_ui.
///
/// - Layer 4 (Mouse):  activates automatically when trackball moves
/// - Layer 5 (Scroll): trackball → scroll wheel
/// - Layer 6 (Sniper): trackball speed reduced
///
/// ## Runtime settings via User keycodes (in Adjust layer):
/// - User0 = CPI +100          User1 = CPI -100
/// - User2 = Scroll divisor +1  User3 = Scroll divisor -1  (higher = slower scroll)
/// - User4 = Sniper divisor +1  User5 = Sniper divisor -1  (higher = slower sniper)
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use rmk::embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use rmk::channel::{CONTROLLER_CHANNEL, KEY_EVENT_CHANNEL, KEYBOARD_REPORT_CHANNEL};
use rmk::event::{ControllerEvent, Event, KeyboardEvent};
use rmk::hid::Report;
use usbd_hid::descriptor::MouseReport;
use rmk::input_device::{InputProcessor, ProcessResult};
use rmk::keymap::KeyMap;

// Layer numbers
const LAYER_MOUSE: u8 = 4;
const LAYER_SCROLL: u8 = 5;
const LAYER_SNIPER: u8 = 6;

// Virtual key at row=8, col=0 → MO(4) in Base layer
pub const AUTO_MOUSE_VIRTUAL_ROW: u8 = 8;
pub const AUTO_MOUSE_VIRTUAL_COL: u8 = 0;

// Auto-mouse timeout: 500ms
const TIMEOUT_DECISECONDS: u32 = 5;

// Default scroll divisor (higher = slower). QMK default = 16 for slow scroll.
// Value 5 is a middle ground — adjust with User2/User3 keys.
const SCROLL_DIVISOR_DEFAULT: u32 = 5;
const SCROLL_DIVISOR_MIN: u32 = 1;
const SCROLL_DIVISOR_MAX: u32 = 32;

// Default sniper divisor
const SNIPER_DIVISOR_DEFAULT: u32 = 4;
const SNIPER_DIVISOR_MIN: u32 = 1;
const SNIPER_DIVISOR_MAX: u32 = 16;

/// Shared state
static LAST_MOTION_T: AtomicU32 = AtomicU32::new(0);
static LAYER_ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTIVE_LAYER: AtomicU8 = AtomicU8::new(0);
/// Current mouse button state (bitmask: bit0=MB1, bit1=MB2, bit2=MB3...)
/// Updated by auto_mouse_tick_task via ControllerEvent::Key
static MOUSE_BUTTONS: AtomicU8 = AtomicU8::new(0);

/// Runtime-adjustable settings (RAM only, reset to defaults on power-off)
static SCROLL_DIVISOR: AtomicU32 = AtomicU32::new(SCROLL_DIVISOR_DEFAULT);
static SNIPER_DIVISOR: AtomicU32 = AtomicU32::new(SNIPER_DIVISOR_DEFAULT);

/// Scroll accumulators
static SCROLL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static SCROLL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);

/// Normal mode accumulators — throttled to BLE connection interval (~8ms = 125Hz)
static NORMAL_ACCUM_X: AtomicI32 = AtomicI32::new(0);
static NORMAL_ACCUM_Y: AtomicI32 = AtomicI32::new(0);
/// Last normal-mode report timestamp in ms (for throttling)
static LAST_NORMAL_REPORT_MS: AtomicU32 = AtomicU32::new(0);
/// Normal mode report interval: 16ms ≈ 62Hz, gives headroom above BLE 7.5ms interval
const NORMAL_REPORT_INTERVAL_MS: u32 = 16;

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

fn now_deciseconds() -> u32 {
    (Instant::now().as_ticks() / (embassy_time::TICK_HZ / 10)) as u32
}

fn now_ms() -> u32 {
    (Instant::now().as_ticks() / (embassy_time::TICK_HZ / 1000)) as u32
}

/// Handle User keycodes for runtime trackball settings.
/// User0-User7 are reserved for BLE profile switching (handled natively by RMK).
/// User8-User11 are for trackball sensitivity.
pub fn handle_user_keycode(keycode_idx: u8, pressed: bool) {
    if !pressed {
        return;
    }
    match keycode_idx {
        // User8: Scroll slower (+divisor)
        8 => {
            let v = (SCROLL_DIVISOR.load(Ordering::Relaxed) + 1).min(SCROLL_DIVISOR_MAX);
            SCROLL_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Scroll divisor: {}", v);
        }
        // User9: Scroll faster (-divisor)
        9 => {
            let v = SCROLL_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SCROLL_DIVISOR_MIN);
            SCROLL_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Scroll divisor: {}", v);
        }
        // User10: Sniper slower (+divisor)
        10 => {
            let v = (SNIPER_DIVISOR.load(Ordering::Relaxed) + 1).min(SNIPER_DIVISOR_MAX);
            SNIPER_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Sniper divisor: {}", v);
        }
        // User11: Sniper faster (-divisor)
        11 => {
            let v = SNIPER_DIVISOR.load(Ordering::Relaxed).saturating_sub(1).max(SNIPER_DIVISOR_MIN);
            SNIPER_DIVISOR.store(v, Ordering::Relaxed);
            defmt::info!("Sniper divisor: {}", v);
        }
        _ => {}
    }
}

/// InputProcessor inserted before trackball0_processor.
pub struct AutoMouseProcessor<
    'a,
    const ROW: usize,
    const COL: usize,
    const NUM_LAYER: usize,
    const NUM_ENCODER: usize = 0,
> {
    keymap: &'a RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>>,
}

impl<'a, const ROW: usize, const COL: usize, const NUM_LAYER: usize, const NUM_ENCODER: usize>
    AutoMouseProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
{
    pub fn new(keymap: &'a RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>>) -> Self {
        Self { keymap }
    }
}

impl<'a, const ROW: usize, const COL: usize, const NUM_LAYER: usize, const NUM_ENCODER: usize>
    InputProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
    for AutoMouseProcessor<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>
{
    async fn process(&mut self, event: Event) -> ProcessResult {
        let Event::Joystick(axes) = event else {
            return ProcessResult::Continue(event);
        };

        // Update auto-mouse timeout
        LAST_MOTION_T.store(now_deciseconds(), Ordering::Relaxed);
        if !LAYER_ACTIVE.load(Ordering::Relaxed) {
            KEY_EVENT_CHANNEL
                .try_send(KeyboardEvent::key(AUTO_MOUSE_VIRTUAL_ROW, AUTO_MOUSE_VIRTUAL_COL, true))
                .ok();
            LAYER_ACTIVE.store(true, Ordering::Relaxed);
        }

        // Extract dx/dy
        let mut dx: i16 = 0;
        let mut dy: i16 = 0;
        for axis in axes.iter() {
            match axis.axis {
                rmk::event::Axis::X => dx = axis.value,
                rmk::event::Axis::Y => dy = axis.value,
                _ => {}
            }
        }

        let layer = ACTIVE_LAYER.load(Ordering::Relaxed);

        match layer {
            LAYER_SCROLL => {
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
                ProcessResult::Stop
            }
            LAYER_SNIPER => {
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
                ProcessResult::Stop
            }
            _ => {
                // Normal mode: accumulate dx/dy and send throttled report (62Hz)
                // Include current button state so drag works correctly.
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
        }
    }

    fn get_keymap(&self) -> &RefCell<KeyMap<'a, ROW, COL, NUM_LAYER, NUM_ENCODER>> {
        self.keymap
    }
}

/// Background task: auto-mouse timeout + layer tracking + User keycode handling.
#[embassy_executor::task]
pub async fn auto_mouse_tick_task() {
    let mut sub = defmt::unwrap!(CONTROLLER_CHANNEL.subscriber());

    loop {
        match select(
            Timer::after(Duration::from_millis(100)),
            sub.next_message_pure(),
        )
        .await
        {
            Either::First(_) => {
                if LAYER_ACTIVE.load(Ordering::Relaxed) {
                    let elapsed = now_deciseconds()
                        .saturating_sub(LAST_MOTION_T.load(Ordering::Relaxed));
                    if elapsed >= TIMEOUT_DECISECONDS {
                        KEY_EVENT_CHANNEL
                            .try_send(KeyboardEvent::key(
                                AUTO_MOUSE_VIRTUAL_ROW,
                                AUTO_MOUSE_VIRTUAL_COL,
                                false,
                            ))
                            .ok();
                        LAYER_ACTIVE.store(false, Ordering::Relaxed);
                        SCROLL_ACCUM_X.store(0, Ordering::Relaxed);
                        SCROLL_ACCUM_Y.store(0, Ordering::Relaxed);
                        NORMAL_ACCUM_X.store(0, Ordering::Relaxed);
                        NORMAL_ACCUM_Y.store(0, Ordering::Relaxed);
                    }
                }
            }
            Either::Second(event) => {
                match event {
                    ControllerEvent::Layer(layer) => {
                        ACTIVE_LAYER.store(layer, Ordering::Relaxed);
                    }
                    ControllerEvent::Key(key_event, action) => {
                        use rmk::types::action::{Action, KeyAction};
                        use rmk::types::keycode::KeyCode;
                        if let KeyAction::Single(Action::Key(kc)) = action {
                            // Track mouse button state for drag support
                            let btn_bit: Option<u8> = match kc {
                                KeyCode::MouseBtn1 => Some(1 << 0),
                                KeyCode::MouseBtn2 => Some(1 << 1),
                                KeyCode::MouseBtn3 => Some(1 << 2),
                                KeyCode::MouseBtn4 => Some(1 << 3),
                                KeyCode::MouseBtn5 => Some(1 << 4),
                                _ => None,
                            };
                            if let Some(bit) = btn_bit {
                                // Toggle button bit on each event (press sets, release clears).
                                // ControllerEvent::Key fires for both press and release,
                                // so XOR correctly tracks state: 0→1 on press, 1→0 on release.
                                let cur = MOUSE_BUTTONS.load(Ordering::Relaxed);
                                MOUSE_BUTTONS.store(cur ^ bit, Ordering::Relaxed);
                            }

                            // Handle User keycodes for trackball settings
                            let id = match kc {
                                KeyCode::User8 => Some(8u8),
                                KeyCode::User9 => Some(9),
                                KeyCode::User10 => Some(10),
                                KeyCode::User11 => Some(11),
                                _ => None,
                            };
                            if let Some(id) = id {
                                handle_user_keycode(id, true);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
