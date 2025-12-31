#![no_std]
#![no_main]

use defmt::{error, info, Debug2Format};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select3, Either3};
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::BufferedUart;
use embassy_stm32::usb::Driver;
use embassy_stm32::{bind_interrupts, peripherals, usart, usb, Config};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::Driver as UsbDriver;
use {defmt_rtt as _, panic_probe as _};

use embedded_io_async::{BufRead, Write};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_LP => usb::InterruptHandler<peripherals::USB>;
    USART1 => usart::BufferedInterruptHandler<peripherals::USART1>;
    USART2 => usart::BufferedInterruptHandler<peripherals::USART2>;
});

pub trait UartBaud {
    fn set_baud(&mut self, baudrate: u32);
}

impl<'d> UartBaud for BufferedUart<'d> {
    fn set_baud(&mut self, baudrate: u32) {
        let _ = self.set_baudrate(baudrate);
    }
}

pub async fn run<'d, D: UsbDriver<'d>>(
    mut usart: impl BufRead + Write + UartBaud,
    cdc_acm: CdcAcmClass<'d, D>,
) -> ! {
    let mut usb_buf = [0; 64];

    let (mut usb_tx, mut usb_rx, usb_control) = cdc_acm.split_with_control();

    loop {
        join(usb_rx.wait_connection(), usb_tx.wait_connection()).await;
        info!("Connected");
        loop {
            match select3(
                usb_control.control_changed(),
                usb_rx.read_packet(&mut usb_buf),
                usart.fill_buf(),
            )
            .await
            {
                Either3::First(_) => {
                    let baud = usb_rx.line_coding().data_rate();
                    info!("Setting baud to: {}", baud);
                    usart.set_baud(baud);
                }
                Either3::Second(res) => match res {
                    Err(err) => {
                        error!("Disconnected {:?}", err);
                        break;
                    }
                    Ok(n) => {
                        if let Err(err) = usart.write(&usb_buf[0..n]).await {
                            error!("Unable to write to usart: {:?}", Debug2Format(&err));
                        }
                    }
                },
                Either3::Third(res) => {
                    let usart_buf = match res {
                        Err(err) => {
                            error!("usart buf error: {:?}", Debug2Format(&err));
                            continue;
                        }
                        Ok(buf) => buf,
                    };
                    if let Err(err) = usb_tx.write_packet(usart_buf).await {
                        error!("Disconnected {:?}", err);
                        break;
                    }
                    let n = usart_buf.len();
                    usart.consume(n);
                }
            }
        }
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn uart_task(
    class: CdcAcmClass<'static, Driver<'static, peripherals::USB>>,
    buf_usart: BufferedUart<'static>,
) -> ! {
    run(buf_usart, class).await;
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

    spawner.spawn(uart_task(class, buf_usart)).unwrap();

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

    spawner.spawn(uart_task(class, buf_usart)).unwrap();

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    usb.run().await;
}
