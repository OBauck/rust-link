#![no_std]
#![no_main]

use ch32_hal as hal;
use embassy_executor::Spawner;
use embassy_usb::class::cmsis_dap_v2::{CmsisDapV2Class, State};
use embassy_usb::msos::windows_version;
use hal::bind_interrupts;
use hal::gpio::{Flex, Level, Output, Speed};
use hal::usbd::Driver;

use ch32v203::usb_dap_task::dap_task;
use ch32v203::{bytes_to_hex, my_println};

use static_cell::StaticCell;

// Println fucks up because it is blocking
// When no debugger is connected, the fw just hangs
// TODO: Switch to ringbuffer + defmt (see defmt_rtt crate for inspiration or use this: https://crates.io/crates/defmt-bbq)
use my_println as println;

bind_interrupts!(struct Irqs {
    USB_LP_CAN1_RX0 => hal::usbd::InterruptHandler<hal::peripherals::USBD>;
});

const CPU_FREQUENCY: u32 = 144_000_000;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // let _ = println!("\n\n\n{}", _info);
    println!("Panic!");

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

    let p = hal::init(hal::Config {
        rcc: hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI,
        ..Default::default()
    });

    let driver = Driver::new(p.USBD, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    // let mut config = embassy_usb::Config::new(0xC0DE, 0xCAFE);
    // See usb device info in linux: "lsusb -d 1a86:7021 -v"
    let mut config = embassy_usb::Config::new(0x1a86, 0x7021);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link-CH32V203 (CMSIS-DAP v2)"); // Need to have "CMSIS-DAP" in the product name to make probe-rs recognize it

    // We set serial number to be hex representation of byte 2 to 7 (6 bytes) of the chips unique ID,
    // as it seems that these distinguish two ch32v203 chips from each other.
    // For example [0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56] will be "ABCDEF123456"
    static UID_STR: StaticCell<[u8; 12]> = StaticCell::new();
    let uid_str = UID_STR.init([0; 12]);
    let uid = hal::signature::unique_id();
    let str = bytes_to_hex(&uid[2..8], uid_str).unwrap();
    config.serial_number = Some(str);

    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static MSOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 96]> = StaticCell::new();

        let builder = embassy_usb::Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            MSOS_DESCRIPTOR.init([0; 256]),
            CONTROL_BUF.init([0; 96]),
        );
        builder
    };

    builder.msos_descriptor(windows_version::WIN8_1, 2);

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CmsisDapV2Class::new(&mut builder, state, 64, false)
    };

    // Build the builder.
    let mut usb = builder.build();

    spawner
        .spawn(dap_task(
            class,
            Flex::new(p.PA0),
            Flex::new(p.PA1),
            Flex::new(p.PB3),
            Some(Output::new(p.PA9, Level::High, Speed::High)),
            CPU_FREQUENCY,
        ))
        .unwrap();

    usb.run().await;
}
