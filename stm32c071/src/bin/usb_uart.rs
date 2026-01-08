#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::usart::BufferedUart;
use embassy_stm32::usb::Driver;
use embassy_stm32::{Config, bind_interrupts, peripherals, usart, usb};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use {defmt_rtt as _, panic_probe as _};

use rust_link_common::usb_uart::run_split_uart as usb_uart_run;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_DRD_FS => usb::InterruptHandler<peripherals::USB>;
    USART1 => usart::BufferedInterruptHandler<peripherals::USART1>;
});

#[embassy_executor::task]
async fn usb_uart_task(
    class: CdcAcmClass<'static, Driver<'static, peripherals::USB>>,
    buf_usart: BufferedUart<'static>,
) -> ! {
    let (usart_tx, usart_rx) = buf_usart.split();
    usb_uart_run(usart_tx, usart_rx, class).await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hsi48 = Some(Hsi48Config {
            sync_from_usb: true,
        });
        config.rcc.hsi = Some(Hsi {
            sys_div: HsiSysDiv::DIV1,
            ker_div: HsiKerDiv::DIV3,
        });
        config.rcc.mux.usbsel = mux::Usbsel::HSI48;
    }
    let p = embassy_stm32::init(config);

    info!("STM32C071 USB uart example");

    let driver = Driver::new(p.USB, Irqs, p.PA12, p.PA11);

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

    let buf_usart = {
        static TX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        static RX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        BufferedUart::new(
            p.USART1,
            p.PB7,
            p.PB6,
            TX_BUF.init([0; 64]),
            RX_BUF.init([0; 64]),
            Irqs,
            Default::default(),
        )
        .unwrap()
    };

    spawner.spawn(usb_uart_task(class, buf_usart)).unwrap();

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    usb.run().await;
}
