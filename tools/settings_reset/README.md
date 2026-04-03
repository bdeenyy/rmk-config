# Settings Reset for Ergohaven nRF52840 Keyboards

Universal firmware that erases the application flash area, clearing all stored settings:
- Keymaps / Vial configuration
- BLE bonds and profiles
- Layout options

## Usage

1. Put the keyboard into bootloader mode (double-tap reset)
2. Drag `settings_reset.uf2` to the USB drive
3. Device will erase settings and enter bootloader again
4. Flash your normal keyboard firmware (.uf2)

## Safe Zones

Only the application area (0x26000–0xF4000) is erased.
The Adafruit bootloader and its settings are preserved.

## Compatible Devices

All Ergohaven keyboards with nRF52840 + Adafruit bootloader:
- K:03 (both halves)
- Imperial44 (both halves)
- Velvet / Velvet UI (both halves)
- OP36 (both halves)
- Trackball Royale
- Trackball Mini v3.0 / v3.1
