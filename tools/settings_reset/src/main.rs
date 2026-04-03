//! Ergohaven Settings Reset — bare metal, no Embassy
//!
//! Erases flash from 0x60000 to 0xF4000 using raw NVMC registers.
//! Identical approach to ZMK settings_reset.
//! Then resets into bootloader.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use cortex_m_rt::entry;

/// NVMC register addresses (nRF52840)
const NVMC_BASE: u32 = 0x4001_E000;
const NVMC_READY: *const u32 = (NVMC_BASE + 0x400) as *const u32;
const NVMC_CONFIG: *mut u32 = (NVMC_BASE + 0x504) as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = (NVMC_BASE + 0x508) as *mut u32;

/// NVMC CONFIG values
const NVMC_CONFIG_REN: u32 = 0; // Read-only
const NVMC_CONFIG_EEN: u32 = 2; // Erase enable

/// Flash layout
const ERASE_START: u32 = 0x60000; // RMK storage default for nRF52
const ERASE_END: u32 = 0xF4000;  // Bootloader starts here
const PAGE_SIZE: u32 = 4096;

/// SCB AIRCR for system reset
const SCB_AIRCR: *mut u32 = 0xE000_ED0C as *mut u32;
const AIRCR_VECTKEY: u32 = 0x05FA_0000;
const AIRCR_SYSRESETREQ: u32 = 1 << 2;

/// Wait for NVMC to be ready
#[inline(never)]
fn nvmc_wait() {
    unsafe {
        while core::ptr::read_volatile(NVMC_READY) == 0 {}
    }
}

/// Erase a single flash page
#[inline(never)]
fn erase_page(addr: u32) {
    unsafe {
        // Enable erase
        nvmc_wait();
        core::ptr::write_volatile(NVMC_CONFIG, NVMC_CONFIG_EEN);
        nvmc_wait();

        // Erase page
        core::ptr::write_volatile(NVMC_ERASEPAGE, addr);
        nvmc_wait();

        // Back to read-only
        core::ptr::write_volatile(NVMC_CONFIG, NVMC_CONFIG_REN);
        nvmc_wait();
    }
}

/// System reset
fn system_reset() -> ! {
    unsafe {
        core::ptr::write_volatile(SCB_AIRCR, AIRCR_VECTKEY | AIRCR_SYSRESETREQ);
    }
    loop {}
}

#[entry]
fn main() -> ! {
    // Erase flash from 0x60000 to 0xF4000
    let mut addr = ERASE_START;
    while addr < ERASE_END {
        erase_page(addr);
        addr += PAGE_SIZE;
    }

    // Reset
    system_reset();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    system_reset();
}
