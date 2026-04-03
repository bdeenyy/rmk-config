#![no_main]
#![no_std]

mod trackball_processor;

use rmk::macros::rmk_keyboard;

#[rmk_keyboard]
mod keyboard {
    use crate::trackball_processor::{TrackballProcessor, trackball_tick_task};

    #[overwritten(entry)]
    fn custom_entry() {
        use rmk::input_device::Runnable;

        let mut trackball_processor =
            TrackballProcessor::<ROW, COL, NUM_LAYER, NUM_ENCODER>::new(&keymap);

        spawner.spawn(trackball_tick_task()).unwrap();

        ::rmk::embassy_futures::join::join(
            ::rmk::run_devices!(
                (adc_device) => ::rmk::channel::EVENT_CHANNEL,
                (trackball0_device) => ::rmk::channel::EVENT_CHANNEL,
                (matrix) => ::rmk::channel::EVENT_CHANNEL
            ),
            ::rmk::embassy_futures::join::join(
                keyboard.run(),
                ::rmk::embassy_futures::join::join(
                    ::rmk::run_processor_chain!(
                        ::rmk::channel::EVENT_CHANNEL => [battery_processor, trackball_processor, trackball0_processor],
                    ),
                    ::rmk::run_rmk(&keymap, driver, &stack, &mut storage, rmk_config),
                ),
            ),
        )
        .await;
    }
}
