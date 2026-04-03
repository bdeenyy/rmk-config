#![no_main]
#![no_std]

use defmt::unwrap;
use embassy_nrf::twim::{self, Twim};
use embassy_time::Duration;
use rmk::{
    channel::{CONTROLLER_CHANNEL, ControllerSub},
    controller::Controller,
    event::ControllerEvent,
    macros::rmk_central,
};

const OLED_ADDR: u8 = 0x3C;
const OLED_WIDTH: usize = 128;
const OLED_PAGES: usize = 4;
const OLED_BUF_SIZE: usize = OLED_WIDTH * OLED_PAGES;
const OLED_I2C_TIMEOUT: Duration = Duration::from_millis(2);

#[derive(Default)]
struct ScreenState {
    layer: u8,
    ble_profile: u8,
    connection_type: u8,
    battery_percent: u8,
    charging_state: bool,
    peripheral_connected: bool,
    sleeping: bool,
}

pub struct ScreenController<'d> {
    i2c: Twim<'d>,
    sub: ControllerSub,
    state: ScreenState,
    available: bool,
    framebuffer: [u8; OLED_BUF_SIZE],
}

impl<'d> ScreenController<'d> {
    pub fn new(i2c: Twim<'d>) -> Self {
        Self {
            i2c,
            sub: unwrap!(CONTROLLER_CHANNEL.subscriber()),
            state: ScreenState {
                connection_type: 1,
                ..ScreenState::default()
            },
            available: true,
            framebuffer: [0; OLED_BUF_SIZE],
        }
    }

    pub fn init_display(&mut self) {
        self.available = self
            .write_commands(&[
                0xAE, 0xD5, 0x80, 0xA8, 0x1F, 0xD3, 0x00, 0x40, 0x8D, 0x14, 0x20, 0x00, 0xA1,
                0xC8, 0xDA, 0x02, 0x81, 0x8F, 0xD9, 0xF1, 0xDB, 0x40, 0xA4, 0xA6, 0xAF,
            ])
            .is_ok();
    }

    fn write_commands(&mut self, commands: &[u8]) -> Result<(), twim::Error> {
        let mut packet = [0u8; 32];
        packet[0] = 0x00;
        let mut offset = 0;
        while offset < commands.len() {
            let chunk_len = core::cmp::min(commands.len() - offset, packet.len() - 1);
            packet[1..1 + chunk_len].copy_from_slice(&commands[offset..offset + chunk_len]);
            self.i2c
                .blocking_write_timeout(OLED_ADDR, &packet[..1 + chunk_len], OLED_I2C_TIMEOUT)?;
            offset += chunk_len;
        }
        Ok(())
    }

    fn flush_buffer(&mut self) {
        if !self.available {
            return;
        }

        for page in 0..OLED_PAGES {
            if self.write_commands(&[0xB0 + page as u8, 0x00, 0x10]).is_err() {
                self.available = false;
                return;
            }
            let base = page * OLED_WIDTH;
            let mut packet = [0u8; 17];
            packet[0] = 0x40;
            for chunk in 0..(OLED_WIDTH / 16) {
                let start = base + chunk * 16;
                let end = start + 16;
                packet[1..].copy_from_slice(&self.framebuffer[start..end]);
                if self
                    .i2c
                    .blocking_write_timeout(OLED_ADDR, &packet, OLED_I2C_TIMEOUT)
                    .is_err()
                {
                    self.available = false;
                    return;
                }
            }
        }
    }

    fn flush_state_to_display(&mut self) {
        if !self.available {
            return;
        }

        self.framebuffer.fill(0);

        if self.state.sleeping {
            self.draw_text(0, 0, "SLEEP");
            self.draw_text(1, 0, "WAKE KEY");
            self.draw_text(2, 0, "TO RESUME");
            self.draw_text(3, 0, "LILY58");
            self.flush_buffer();
            return;
        }

        self.draw_text(0, 0, "LILY58 OLED");
        self.draw_text(1, 0, "LAYER ");
        self.draw_u8(1, 6, self.state.layer);

        match self.state.connection_type {
            0 => self.draw_text(2, 0, "USB"),
            _ => {
                self.draw_text(2, 0, "BLE P");
                self.draw_u8(2, 6, self.state.ble_profile.saturating_add(1));
            }
        }

        self.draw_text(3, 0, "BAT ");
        if self.state.charging_state {
            self.draw_text(3, 4, "CHG");
        } else {
            self.draw_u8(3, 4, self.state.battery_percent);
        }

        self.draw_text(3, 9, "R ");
        self.draw_text(
            3,
            11,
            if self.state.peripheral_connected { "OK" } else { "--" },
        );

        self.flush_buffer();
    }

    fn draw_text(&mut self, page: usize, col: usize, text: &str) {
        for (idx, ch) in text.bytes().enumerate() {
            self.draw_char(page, col + idx, ch);
        }
    }

    fn draw_u8(&mut self, page: usize, col: usize, value: u8) {
        if value >= 100 {
            self.draw_char(page, col, b'1');
            self.draw_char(page, col + 1, b'0');
            self.draw_char(page, col + 2, b'0');
        } else if value >= 10 {
            self.draw_char(page, col, b'0' + (value / 10));
            self.draw_char(page, col + 1, b'0' + (value % 10));
        } else {
            self.draw_char(page, col, b'0' + value);
        }
    }

    fn draw_char(&mut self, page: usize, col: usize, ch: u8) {
        let glyph = glyph(ch);
        let start = page * OLED_WIDTH + col * 6;
        if start + 5 >= self.framebuffer.len() {
            return;
        }
        self.framebuffer[start..start + 5].copy_from_slice(&glyph);
        self.framebuffer[start + 5] = 0;
    }
}

impl Controller for ScreenController<'static> {
    type Event = ControllerEvent;

    async fn process_event(&mut self, event: Self::Event) {
        match event {
            ControllerEvent::Layer(layer) => {
                if layer == self.state.layer {
                    return;
                }
                self.state.layer = layer;
            }
            ControllerEvent::Battery(level) => {
                if level == self.state.battery_percent {
                    return;
                }
                self.state.battery_percent = level;
            }
            ControllerEvent::BleProfile(profile) => {
                if profile == self.state.ble_profile {
                    return;
                }
                self.state.ble_profile = profile;
            }
            ControllerEvent::ChargingState(charging) => {
                if charging == self.state.charging_state {
                    return;
                }
                self.state.charging_state = charging;
            }
            ControllerEvent::ConnectionType(kind) => {
                if kind == self.state.connection_type {
                    return;
                }
                self.state.connection_type = kind;
            }
            ControllerEvent::SplitPeripheral(_, connected) => {
                if connected == self.state.peripheral_connected {
                    return;
                }
                self.state.peripheral_connected = connected;
            }
            ControllerEvent::Sleep(sleeping) => {
                if sleeping == self.state.sleeping {
                    return;
                }
                self.state.sleeping = sleeping;
            }
            _ => {
                return;
            }
        }

        self.flush_state_to_display();
    }

    async fn next_message(&mut self) -> Self::Event {
        self.sub.next_message_pure().await
    }
}

fn glyph(ch: u8) -> [u8; 5] {
    match ch {
        b' ' => [0x00, 0x00, 0x00, 0x00, 0x00],
        b'-' => [0x08, 0x08, 0x08, 0x08, 0x08],
        b'0' => [0x3E, 0x51, 0x49, 0x45, 0x3E],
        b'1' => [0x00, 0x42, 0x7F, 0x40, 0x00],
        b'2' => [0x62, 0x51, 0x49, 0x49, 0x46],
        b'3' => [0x22, 0x49, 0x49, 0x49, 0x36],
        b'4' => [0x18, 0x14, 0x12, 0x7F, 0x10],
        b'5' => [0x2F, 0x49, 0x49, 0x49, 0x31],
        b'6' => [0x3E, 0x49, 0x49, 0x49, 0x32],
        b'7' => [0x01, 0x71, 0x09, 0x05, 0x03],
        b'8' => [0x36, 0x49, 0x49, 0x49, 0x36],
        b'9' => [0x26, 0x49, 0x49, 0x49, 0x3E],
        b'A' => [0x7E, 0x11, 0x11, 0x11, 0x7E],
        b'B' => [0x7F, 0x49, 0x49, 0x49, 0x36],
        b'C' => [0x3E, 0x41, 0x41, 0x41, 0x22],
        b'D' => [0x7F, 0x41, 0x41, 0x22, 0x1C],
        b'E' => [0x7F, 0x49, 0x49, 0x49, 0x41],
        b'G' => [0x3E, 0x41, 0x49, 0x49, 0x7A],
        b'H' => [0x7F, 0x08, 0x08, 0x08, 0x7F],
        b'I' => [0x00, 0x41, 0x7F, 0x41, 0x00],
        b'K' => [0x7F, 0x08, 0x14, 0x22, 0x41],
        b'L' => [0x7F, 0x40, 0x40, 0x40, 0x40],
        b'O' => [0x3E, 0x41, 0x41, 0x41, 0x3E],
        b'P' => [0x7F, 0x09, 0x09, 0x09, 0x06],
        b'R' => [0x7F, 0x09, 0x19, 0x29, 0x46],
        b'S' => [0x26, 0x49, 0x49, 0x49, 0x32],
        b'T' => [0x01, 0x01, 0x7F, 0x01, 0x01],
        b'U' => [0x3F, 0x40, 0x40, 0x40, 0x3F],
        b'W' => [0x3F, 0x40, 0x38, 0x40, 0x3F],
        b'Y' => [0x07, 0x08, 0x70, 0x08, 0x07],
        _ => [0x00, 0x00, 0x00, 0x00, 0x00],
    }
}

#[rmk_central]
mod keyboard_central {
    add_interrupt!(TWISPI0 => ::embassy_nrf::twim::InterruptHandler<::embassy_nrf::peripherals::TWISPI0>;);

    #[controller(event)]
    fn screen_controller() -> crate::ScreenController<'static> {
        static TWIM_TX_BUF: ::static_cell::StaticCell<[u8; 32]> = ::static_cell::StaticCell::new();
        let tx_buf = &mut TWIM_TX_BUF.init([0; 32])[..];

        let mut config = ::embassy_nrf::twim::Config::default();
        config.frequency = ::embassy_nrf::twim::Frequency::K400;

        let i2c = ::embassy_nrf::twim::Twim::new(p.TWISPI0, Irqs, p.P0_08, p.P0_06, config, tx_buf);
        let mut controller = ScreenController::new(i2c);
        controller.init_display();
        controller.flush_state_to_display();
        controller
    }
}
