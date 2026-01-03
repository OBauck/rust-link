#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_rp::uart::{BufferedUart, BufferedInterruptHandler};
use embassy_rp::{bind_interrupts, peripherals};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use {defmt_rtt as _, panic_probe as _};

use ob_link_common::usb_uart::run_split_uart as usb_uart_run;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<peripherals::USB>;
    UART0_IRQ => BufferedInterruptHandler<peripherals::UART0>;
});

#[embassy_executor::task(pool_size = 2)]
async fn usb_uart_task(
    class: CdcAcmClass<'static, Driver<'static, peripherals::USB>>,
    buf_uart: BufferedUart,
) -> ! {
    let (uart_tx, uart_rx) = buf_uart.split();
    usb_uart_run(uart_tx, uart_rx, class).await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    info!("RP235x USB uart example");

    let driver = Driver::new(p.USB, Irqs);

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0xC0DE, 0xCAFE);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link (COM port)");
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

    // Create classes on the builder.
    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    let buf_uart = {
        static TX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        static RX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        BufferedUart::new(
            p.UART0,
            p.PIN_0,
            p.PIN_1,
            Irqs,
            &mut TX_BUF.init([0; 64])[..],
            &mut RX_BUF.init([0; 64])[..],
            Default::default(),
        )
    };

    spawner.spawn(usb_uart_task(class, buf_uart)).unwrap();

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    usb.run().await;
}
