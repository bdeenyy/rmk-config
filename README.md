# RMK Config for Lily58 Pro

RMK BLE split firmware for `Lily58 Pro` on `nice!nano v2` / `nRF52840`.

This repository is focused on one keyboard target:

- `Lily58 Pro`
- split BLE
- `Vial` support
- GitHub Actions build with `.uf2` artifacts for both halves

## Hardware

- Keyboard: `Lily58 Pro`
- MCU: `nice!nano v2`
- Chip: `nRF52840`
- Bootloader: Adafruit UF2 / `nice!nano`
- Connection: Bluetooth Low Energy

## Current Status

The firmware target lives in `keyboards/lily58pro`.

Implemented in the current version:

- split BLE firmware
- `central` and `peripheral` binaries
- `Vial` definition
- GitHub Actions build
- battery ADC configuration

Not included yet:

- OLED support

Important:

- matrix pins are currently carried over from an earlier local RMK attempt
- they are build-verified, but still need real hardware validation on the keyboard

## Repository Layout

- `keyboards/lily58pro` - keyboard firmware target
- `libs/nrf-sdc` - vendored BLE stack dependency required for reproducible builds
- `.github/workflows/build.yml` - CI build workflow

## Build on GitHub

Every push to `main` triggers GitHub Actions.

The workflow builds:

- `central` -> left half
- `peripheral` -> right half

Artifacts are uploaded as:

- `lily58pro_left.uf2`
- `lily58pro_right.uf2`

Actions page:

- [GitHub Actions](https://github.com/bdeenyy/rmk-config/actions)

## Local Build

Build from the keyboard directory:

```sh
cd keyboards/lily58pro
cargo build --release --bin central
cargo build --release --bin peripheral
```

On Windows, if needed, set:

```powershell
$env:RUST_MIN_STACK='16777216'
$env:KEYBOARD_TOML_PATH='D:\Code\lily58-rmk\keyboards\lily58pro\keyboard.toml'
$env:VIAL_JSON_PATH='D:\Code\lily58-rmk\keyboards\lily58pro\vial.json'
```

## Flashing

1. Put the half into bootloader mode with a double reset tap.
2. A `NICENANO` drive should appear.
3. Copy the correct `.uf2` file to the drive.

For this keyboard:

- flash `lily58pro_left.uf2` to the left half
- flash `lily58pro_right.uf2` to the right half

## Keymap

The current keymap is based on the existing ZMK layout and includes:

- Base
- Lower
- Raise
- Adjust

The `Adjust` layer is intended for Bluetooth/output actions and bootloader access.

## Vial

`Vial` is enabled in the keyboard config and described by:

- `keyboards/lily58pro/keyboard.toml`
- `keyboards/lily58pro/vial.json`

## Notes

- This repo started from `ergohaven/rmk-eh` and was adapted for `Lily58 Pro`.
- The dependency `libs/nrf-sdc` is vendored intentionally so CI and local builds do not break on missing submodules.
