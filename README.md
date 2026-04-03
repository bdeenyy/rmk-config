# Ergohaven RMK Firmware

RMK BLE split firmware for Ergohaven keyboards and trackballs (nRF52840).

This repo also contains a Lily58 Pro target adapted for nice!nano / nRF52840 with Vial support in `keyboards/lily58pro`.

## Supported Devices

### Keyboards (BLE split)

| Keyboard    | Layout         | Encoders | Trackball |
|-------------|----------------|----------|-----------|
| K:03        | 5×6 + 5 thumb  | 3+3      | —         |
| Imperial44  | 4×6 + 3 thumb  | 1+1      | —         |
| OP36        | 3×5 + 3 thumb  | —        | —         |
| Velvet      | 4×6 + 5 thumb  | —        | —         |
| Velvet UI   | 4×6 + 5 thumb  | —        | PMW3610   |
| Lily58 Pro  | 4×6 + 4 thumb + inner keys | — | — |

### Trackballs (standalone BLE)

| Device              | Buttons | Modes                          |
|---------------------|---------|--------------------------------|
| Trackball Royale     | 6       | Normal, Scroll, Sniper, Adjust |
| Trackball Mini v3.1 | 4       | Normal, Scroll, Sniper, Adjust |
| Trackball Mini v3.0 | 2       | Normal, Scroll, Sniper, Adjust |

### Tools

| Tool           | Description                              |
|----------------|------------------------------------------|
| settings_reset | Erases keymap and BLE bonds, resets to bootloader |

## Building

```sh
cd keyboards/lily58pro
cargo build --release --bin central
cargo build --release --bin peripheral
```

## Flashing

1. Put device into bootloader (double-tap reset)
2. Copy `.uf2` file to the mounted USB drive
3. For split keyboards: flash central and peripheral separately

## Settings Reset

Flash `settings_reset.uf2` to erase all saved keymap/BLE data, then re-flash keyboard firmware.

## CI

Every push builds all devices in parallel via GitHub Actions. UF2 artifacts available as build downloads.

## RMK Version

Based on [RMK](https://github.com/HaoboGu/rmk) 0.8.2 with nRF52840 BLE support.
