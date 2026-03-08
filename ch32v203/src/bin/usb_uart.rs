#![no_std]
#![no_main]

use ch32_hal as hal;
use ch32_hal::mode::Async;
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_usb::class::cdc_acm::{CdcAcmClass, ControlChanged, Receiver, Sender, State};
use hal::usart::{Config as UartConfig, Uart, UartRx, UartTx};
use hal::usbd::Driver;
use hal::{bind_interrupts, peripherals, println as ch32_println};
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
    USART2 => hal::usart::InterruptHandler<hal::peripherals::USART2>;
});

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // let _ = println!("\n\n\n{}", _info);
    println!("Panic!");

    loop {}
}

#[embassy_executor::task]
async fn usb_tx_uart_rx_task(
    mut usb_tx: Sender<'static, Driver<'static, peripherals::USBD>>,
    mut uart_rx: UartRx<'static, peripherals::USART2, Async>,
    usb_control: ControlChanged<'static>,
    mut uart_config: UartConfig,
) -> ! {
    let mut buf = [0; 64];

    loop {
        usb_tx.wait_connection().await;
        println!("Usb tx Connected");
        loop {
            match select(
                usb_control.control_changed(),
                uart_rx.read_until_idle(&mut buf),
            )
            .await
            {
                Either::First(_) => {
                    let baud = usb_tx.line_coding().data_rate();
                    println!("Setting baud to: {}", baud);
                    uart_config.baudrate = baud;
                    if let Err(err) = uart_rx.set_config(&uart_config) {
                        println!("Uart config error: {:?}", err);
                    }
                }
                Either::Second(Err(err)) => println!("uart rx error! {:?}", err),
                Either::Second(Ok(n)) => {
                    if let Err(err) = usb_tx.write_packet(&buf[..n]).await {
                        println!("Usb tx error! {:?}", err)
                    }
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn usb_rx_uart_tx_task(
    mut usb_rx: Receiver<'static, Driver<'static, peripherals::USBD>>,
    mut uart_tx: UartTx<'static, peripherals::USART2, Async>,
) -> ! {
    let mut buf = [0; 64];
    loop {
        usb_rx.wait_connection().await;
        println!("Usb rx Connected");
        loop {
            match usb_rx.read_packet(&mut buf).await {
                Ok(n) => {
                    if let Err(err) = uart_tx.write(&buf[..n]).await {
                        println!("uart tx error! {:?}", err);
                    }
                }
                Err(err) => println!("Usb rx error: {:?}"),
            }
        }
    }
}
// If you are trying this and your USB device doesn't connect, the most
// common issues are the RCC config and vbus_detection
//
// See https://embassy.dev/book/#_the_usb_examples_are_not_working_on_my_board_is_there_anything_else_i_need_to_configure
// for more information.
//
// println will not work without probe.
#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) {
    let p = hal::init(hal::Config {
        rcc: hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI,
        ..Default::default()
    });

    let driver = Driver::new(p.USBD, Irqs, p.PA12, p.PA11);

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

    // Build the builder.
    let mut usb = builder.build();

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
    let (usb_tx, usb_rx, usb_control) = class.split_with_control();

    spawner
        .spawn(usb_tx_uart_rx_task(
            usb_tx,
            uart_rx,
            usb_control,
            uart_config,
        ))
        .unwrap();
    spawner.spawn(usb_rx_uart_tx_task(usb_rx, uart_tx)).unwrap();

    // Run the USB device.
    usb.run().await;
}
