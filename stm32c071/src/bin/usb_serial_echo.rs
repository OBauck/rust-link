#![no_std]
#![no_main]

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use embassy_stm32::usb::Driver;
use embassy_stm32::{Config, bind_interrupts, peripherals, usb};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use {defmt_rtt as _, panic_probe as _};

use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_DRD_FS => usb::InterruptHandler<peripherals::USB>;
});

#[embassy_executor::task]
async fn usb_uart_task(class: CdcAcmClass<'static, Driver<'static, peripherals::USB>>) -> ! {
    let mut buf = [0; 64];

    let (mut usb_tx, mut usb_rx, usb_control) = class.split_with_control();

    loop {
        join(usb_rx.wait_connection(), usb_tx.wait_connection()).await;
        info!("Connected");
        loop {
            match select(usb_control.control_changed(), usb_rx.read_packet(&mut buf)).await {
                Either::First(_) => {
                    let baud = usb_rx.line_coding().data_rate();
                    info!("Baud change: {:?}", baud);
                }
                Either::Second(res) => match res {
                    Err(err) => {
                        warn!("Usb rx error: {:?}. Assume disconnection", err);
                        break;
                    }
                    Ok(n) => {
                        // Echo data
                        if let Err(err) = usb_tx.write_packet(&buf[..n]).await {
                            warn!("Usb tx error: {:?}. Assume disconnection", err);
                            break;
                        }
                    }
                },
            }
        }
    }
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

    spawner.spawn(usb_uart_task(class)).unwrap();

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    usb.run().await;
}
