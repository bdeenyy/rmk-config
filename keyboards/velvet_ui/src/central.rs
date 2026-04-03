#![no_main]
#![no_std]

mod auto_mouse;

use rmk::macros::rmk_central;

#[rmk_central]
mod keyboard_central {
    use crate::auto_mouse::{AutoMouseProcessor, auto_mouse_tick_task};

    #[overwritten(entry)]
    fn custom_entry() {
        use rmk::input_device::Runnable;

        let mut auto_mouse_processor =
            AutoMouseProcessor::<ROW, COL, NUM_LAYER, NUM_ENCODER>::new(&keymap);

        spawner.spawn(auto_mouse_tick_task()).unwrap();

        ::rmk::embassy_futures::join::join(
            ::rmk::run_devices!((adc_device, matrix) => ::rmk::channel::EVENT_CHANNEL),
            ::rmk::embassy_futures::join::join(
                keyboard.run(),
                ::rmk::embassy_futures::join::join(
                    ::rmk::run_processor_chain!(
                        ::rmk::channel::EVENT_CHANNEL => [battery_processor, auto_mouse_processor, trackball0_processor],
                    ),
                    ::rmk::embassy_futures::join::join(
                        ::rmk::run_rmk(&keymap, driver, &stack, &mut storage, rmk_config),
                        ::rmk::embassy_futures::join::join(
                            ::rmk::split::central::run_peripheral_manager::<4, 6, 4, 0, _>(
                                0,
                                &peripheral_addrs,
                                &stack,
                            ),
                            ::rmk::split::ble::central::scan_peripherals(&stack, &peripheral_addrs),
                        ),
                    ),
                ),
            ),
        )
        .await;
    }
}
