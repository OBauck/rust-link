#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};

use ch32_hal::{self as hal, bind_interrupts};
use hal::usbd::Driver;

use ch32v203::usb_ppk2_task::ppk2_task;
use rust_link_common::usb_ppk2_dfu::Ppk2DfuClass;

use static_cell::StaticCell;

#[macro_export]
macro_rules! my_println {
    ($($arg:tt)*) => {
        ()
    };
}

// Println fucks up because it is blocking
// When no debugger is connected, the fw just hangs
// TODO: Switch to ringbuffer + defmt (see defmt_rtt crate for inspiration or use this: https://crates.io/crates/defmt-bbq)
use my_println as println;

bind_interrupts!(struct Irqs {
    USB_LP_CAN1_RX0 => hal::usbd::InterruptHandler<hal::peripherals::USBD>;
});

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Printing out info here will use too much flash
    // Only print "Panic" to save flash size
    // let _ = println!("\n\n\n{}", _info);
    println!("Panic");

    loop {}
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) {
    #[cfg(feature = "bootloader")]
    unsafe {
        qingke::register::mtvec::write(
            0x00001000,
            qingke::register::mtvec::TrapMode::VectoredAddress,
        );
    }

    let mut config = hal::Config::default();
    config.rcc = hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI;
    config.rcc.apb2_pre = ch32_hal::rcc::APBPrescaler::DIV2;
    let p = hal::init(config);

    println!("OB-Link PPK2!");
    let driver = Driver::new(p.USBD, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    // usb vid and pid needs to be Nordic Semiconductor for power profiler application to recognize it
    let mut config = embassy_usb::Config::new(0x1915, 0xc00a);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link (PPK2)");
    config.serial_number = Some("12345678");

    // Windows compatibility requires these; CDC-ACM
    config.device_class = 0x02;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x00;
    config.composite_with_iads = false;

    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();

        let builder = embassy_usb::Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            &mut [], // no msos descriptors
            CONTROL_BUF.init([0; 64]),
        );
        builder
    };

    Ppk2DfuClass::new(&mut builder);

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    // Build the builder.
    let mut usb = builder.build();

    spawner
        .spawn(ppk2_task(class, p.PB4, p.PA6, p.DMA1_CH1, p.TIM3))
        .unwrap();

    // Run the USB device.
    usb.run().await;
}
