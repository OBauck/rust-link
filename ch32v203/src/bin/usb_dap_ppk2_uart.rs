#![no_std]
#![no_main]

use ch32_hal as hal;
use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State as CdcState};
use embassy_usb::class::cmsis_dap_v2::{CmsisDapV2Class, State as DapV2State};
use embassy_usb::msos::windows_version;
use hal::bind_interrupts;
use hal::gpio::{Flex, Level, Output, Speed};
use hal::usart::{Config as UartConfig, Uart};
use hal::usbd::Driver;

use ch32v203::usb_dap_task::dap_task;
use ch32v203::usb_ppk2_task::ppk2_task;
use ch32v203::usb_uart_task::{usb_rx_uart_tx_task, usb_tx_uart_rx_task};
use ch32v203::{bytes_to_hex, my_println};

use rust_link_common::usb_ppk2_dfu::Ppk2DfuClass;

use static_cell::StaticCell;

// Println fucks up because it is blocking
// When no debugger is connected, the fw just hangs
// TODO: Switch to ringbuffer + defmt (see defmt_rtt crate for inspiration or use this: https://crates.io/crates/defmt-bbq)
// use hal::println;
use my_println as println;

bind_interrupts!(struct Irqs {
    USB_LP_CAN1_RX0 => hal::usbd::InterruptHandler<hal::peripherals::USBD>;
    USART2 => hal::usart::InterruptHandler<hal::peripherals::USART2>;
});

const CPU_FREQUENCY: u32 = 144_000_000;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    let _ = println!("\n\n\n{}", _info);
    println!("Panic!");

    loop {}
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) {
    let mut config = hal::Config::default();
    config.rcc = hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI;
    config.rcc.apb2_pre = ch32_hal::rcc::APBPrescaler::DIV2;
    let p = hal::init(config);

    let driver = Driver::new(p.USBD, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    // usb vid and pid needs to be Nordic Semiconductor for power profiler application to recognize it
    let mut config = embassy_usb::Config::new(0x1915, 0xc00a);
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

    // Windows compatibility requires these; CDC-ACM
    config.device_class = 0x02;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x00;
    config.composite_with_iads = false;

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

    Ppk2DfuClass::new(&mut builder);

    let class_dap = {
        static STATE: StaticCell<DapV2State> = StaticCell::new();
        let state = STATE.init(DapV2State::new());
        CmsisDapV2Class::new(&mut builder, state, 64, false)
    };

    let class_ppk2 = {
        static STATE: StaticCell<CdcState> = StaticCell::new();
        let state = STATE.init(CdcState::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    // max_packet_size = 24 bytes is the max size before we get endpoint memory full
    // (with dap and ppk2 max_packet_size = 64 bytes)
    let class_uart = {
        static STATE: StaticCell<CdcState> = StaticCell::new();
        let state = STATE.init(CdcState::new());
        CdcAcmClass::new(&mut builder, state, 24)
    };

    // Build the builder.
    let mut usb = builder.build();

    spawner
        .spawn(dap_task(
            class_dap,
            Flex::new(p.PA0),
            Flex::new(p.PA1),
            Flex::new(p.PB3),
            Some(Output::new(p.PA9, Level::High, Speed::High)),
            CPU_FREQUENCY,
        ))
        .unwrap();

    spawner
        .spawn(ppk2_task(class_ppk2, p.PB4, p.PA6, p.DMA1_CH1, p.TIM3))
        .unwrap();

    let uart_config = UartConfig::default();
    let uart = Uart::new(
        p.USART2,
        p.PA3,
        p.PA2,
        Irqs,
        p.DMA1_CH7,
        p.DMA1_CH6,
        uart_config,
    )
    .unwrap();

    let (uart_tx, uart_rx) = uart.split();
    let (usb_tx, usb_rx, usb_control) = class_uart.split_with_control();

    spawner
        .spawn(usb_tx_uart_rx_task(
            usb_tx,
            uart_rx,
            usb_control,
            uart_config,
        ))
        .unwrap();
    spawner.spawn(usb_rx_uart_tx_task(usb_rx, uart_tx)).unwrap();

    usb.run().await;
}
