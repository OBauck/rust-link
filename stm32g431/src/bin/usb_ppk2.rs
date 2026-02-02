#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::time::Hertz;
use embassy_stm32::usb::Driver;
use embassy_stm32::{bind_interrupts, peripherals, usb, Config};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use {defmt_rtt as _, panic_probe as _};

use ob_link_common::usb_ppk2::{run as ppk2_run, Adc};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_LP => usb::InterruptHandler<peripherals::USB>;
});

const ADC_BUFFER_SIZE: usize = 512;
const ADC_SEND_INTERVAL_US: u64 = ADC_BUFFER_SIZE as u64 * 10; // 100KHz = 10us

struct DummyAdc {
    buffer: &'static mut [u16],
    next_send_time: Instant,
}

impl DummyAdc {
    fn new(buffer: &'static mut [u16]) -> Self {
        Self {
            buffer,
            next_send_time: Instant::MAX,
        }
    }
}

impl Adc for DummyAdc {
    async fn read_data(&mut self) -> &[u16] {
        if self.next_send_time == Instant::MAX {
            Timer::after_secs(10).await;
            return self.buffer;
        }
        Timer::at(self.next_send_time).await;
        self.next_send_time = self.next_send_time + Duration::from_micros(ADC_SEND_INTERVAL_US);
        let first_value = self.buffer.last().unwrap().wrapping_add(1);
        for i in 0..self.buffer.len() {
            self.buffer[i] = first_value.wrapping_add(i as u16);
        }
        self.buffer
    }
    fn start(&mut self) {
        self.next_send_time = Instant::now() + Duration::from_micros(ADC_SEND_INTERVAL_US);
    }
    fn stop(&mut self) {
        self.next_send_time = Instant::MAX;
    }
}

#[embassy_executor::task]
async fn usb_ppk2_task(
    class: CdcAcmClass<'static, Driver<'static, peripherals::USB>>,
    power_out_enable: Output<'static>,
) -> ! {
    static ADC_BUFFER: StaticCell<[u16; ADC_BUFFER_SIZE]> = StaticCell::new();
    let buffer = ADC_BUFFER.init([0; ADC_BUFFER_SIZE]);
    for i in 0..buffer.len() {
        buffer[i] = i as u16;
    }
    let adc = DummyAdc::new(buffer);
    ppk2_run(class, adc, power_out_enable, 1, 12).await;
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

    info!("STM32G431 USB ppk2 example");

    let driver = Driver::new(p.USB, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0xC0DE, 0xCAFE);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link (ppk2)");
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

    let power_out_enable = Output::new(p.PC6, Level::High, Speed::Low);

    spawner
        .spawn(usb_ppk2_task(class, power_out_enable))
        .unwrap();

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    usb.run().await;
}
