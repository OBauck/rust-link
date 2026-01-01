#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::BufferedUart;
use embassy_stm32::usb::Driver;
use embassy_stm32::{bind_interrupts, peripherals, usart, usb, Config};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use {defmt_rtt as _, panic_probe as _};

use ob_link_common::usb_uart::run_split_uart as usb_uart_run;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_LP => usb::InterruptHandler<peripherals::USB>;
    USART1 => usart::BufferedInterruptHandler<peripherals::USART1>;
    USART2 => usart::BufferedInterruptHandler<peripherals::USART2>;
});

#[embassy_executor::task(pool_size = 2)]
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
        // Sets up the Clock Recovery System (CRS) to use the USB SOF to trim the HSI48 oscillator.
        config.rcc.hsi48 = Some(Hsi48Config {
            sync_from_usb: true,
        });
        config.rcc.hse = Some(Hse {
            freq: Hertz(8_000_000),
            mode: HseMode::Oscillator,
        });
        config.rcc.pll = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV2,
            mul: PllMul::MUL72,
            divp: None,
            divq: Some(PllQDiv::DIV6), // 48mhz
            divr: Some(PllRDiv::DIV2), // Main system clock at 144 MHz
        });
        config.rcc.sys = Sysclk::PLL1_R;
        config.rcc.boost = true; // BOOST!
        config.rcc.mux.clk48sel = mux::Clk48sel::HSI48;
        //config.rcc.mux.clk48sel = mux::Clk48sel::PLL1_Q; // uncomment to use PLL1_Q instead.
    }
    let p = embassy_stm32::init(config);

    info!("STM32G431 USB uart example");

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
            p.PA10,
            p.PA9,
            TX_BUF.init([0; 64]),
            RX_BUF.init([0; 64]),
            Irqs,
            Default::default(),
        )
        .unwrap()
    };

    spawner.spawn(usb_uart_task(class, buf_usart)).unwrap();

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    let buf_usart = {
        static TX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        static RX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
        BufferedUart::new(
            p.USART2,
            p.PA3,
            p.PA2,
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
