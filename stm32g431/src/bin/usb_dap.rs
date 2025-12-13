#![no_std]
#![no_main]

use cortex_m::peripheral::{syst::SystClkSource, Peripherals as CorePeripherals, SYST};
use dap_rs::dap::{Dap, DapLeds, DelayNs, HostStatus};
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Flex, Level, Output, Pull, Speed};
use embassy_stm32::time::Hertz;
use embassy_stm32::usb::{self, Driver};
use embassy_stm32::{bind_interrupts, peripherals, Config};
use embassy_usb::class::cmsis_dap_v2::{CmsisDapV2Class, State};
use embassy_usb::msos::windows_version;
use embedded_hal::digital::PinState;
use {defmt_rtt as _, panic_probe as _};

use ob_dap::{self, Context, DbgDelay, DbgPin, Swo};

use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USB_LP => usb::InterruptHandler<peripherals::USB>;
});

const CPU_FREQUENCY: u32 = 144_000_000;

#[embassy_executor::task]
async fn dap_task(
    mut class: CmsisDapV2Class<'static, Driver<'static, peripherals::USB>>,
    swdio_pin: Flex<'static>,
    swclk_pin: Flex<'static>,
    nreset_pin: Flex<'static>,
    led_pin: Output<'static>,
) -> ! {
    let swdio = DebuggerPin::new(swdio_pin, Pull::Down);
    let swclk = DebuggerPin::new(swclk_pin, Pull::Down);
    let nreset = DebuggerPin::new(nreset_pin, Pull::None);

    let core_peripherals_1 = CorePeripherals::take().unwrap();
    let core_peripherals_2 = unsafe { CorePeripherals::steal() };

    let bit_bang_delay = MyDelay::new(core_peripherals_1.SYST, CPU_FREQUENCY);
    let wait = MyDelay::new(core_peripherals_2.SYST, CPU_FREQUENCY);

    let leds = MyLeds { pin: led_pin };

    let context = Context::from_pins(swdio, swclk, nreset, CPU_FREQUENCY, bit_bang_delay);
    let swo: Option<Swo> = None;

    let mut dap_handler = Dap::new(context, leds, wait, swo, "V1");

    let mut report = [0; 64];
    let mut resp_buf = [0; 64];

    loop {
        class.wait_connection().await;
        info!("Connected");
        loop {
            let n = match class.read_packet(&mut report).await {
                Ok(val) => val,
                Err(_) => {
                    error!("Error!");
                    break;
                }
            };
            let len = dap_handler.process_command(
                &report[..n],
                &mut resp_buf,
                dap_rs::dap::DapVersion::V2,
            );
            if len > 0 {
                if let Err(_err) = class.write_packet(&resp_buf[..len]).await {
                    info!("Error!");
                    break;
                }
            }
        }
        info!("Disconnected");
    }
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

    let driver = Driver::new(p.USB, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0x1a86, 0x7021);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link-STM32G431 (CMSIS-DAP v2)"); // Need to have "CMSIS-DAP" in the product name to make probe-rs recognize it
    config.serial_number = Some("12345678");

    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static MSOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();

        let builder = embassy_usb::Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            MSOS_DESCRIPTOR.init([0; 256]),
            CONTROL_BUF.init([0; 64]),
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
            Flex::new(p.PA4),
            Output::new(p.PC6, Level::Low, Speed::High),
        ))
        .unwrap();

    usb.run().await;
}

struct MyDelay {
    systick: SYST,
    ticks_per_us: u32,
}

impl MyDelay {
    fn new(mut systick: SYST, cpu_frequency: u32) -> Self {
        // Set clock source to processor clock
        systick.set_clock_source(SystClkSource::Core);

        // Set reload and current values
        systick.set_reload(0xffffff);
        systick.clear_current();

        // Enable the counter
        systick.enable_counter();
        Self {
            systick,
            ticks_per_us: (cpu_frequency + 500_000) / 1_000_000,
        }
    }
    fn delay_micros(&self, mut us: u32) {
        while us > 0x1fff {
            let ticks = (us & 0x1fff) * self.ticks_per_us;
            self.delay_ticks(ticks as u32);
            us -= us & 0x1fff;
        }
    }

    fn delay_ticks(&self, mut ticks: u32) {
        let mut last = self.get_current();
        loop {
            let now = self.get_current();
            let delta = last.wrapping_sub(now) & 0xffffff;

            if delta >= ticks {
                break;
            } else {
                ticks -= delta;
                last = now;
            }
        }
    }
}

impl DelayNs for MyDelay {
    fn delay_ns(&mut self, ns: u32) {
        self.delay_micros(ns * 1000);
    }
}

impl DbgDelay for MyDelay {
    fn delay_ticks_from_last(&self, mut ticks: u32, mut last: u32) -> u32 {
        loop {
            let now = self.get_current();
            let delta = last.wrapping_sub(now);

            if delta >= ticks {
                break now;
            } else {
                ticks -= delta;
                last = now;
            }
        }
    }
    #[inline(always)]
    fn get_current(&self) -> u32 {
        self.systick.cvr.read()
    }
}

struct DebuggerPin<'a> {
    is_output: bool,
    pull_input: Pull,
    pin: Flex<'a>,
}

impl<'a> DebuggerPin<'a> {
    fn new(mut pin: Flex<'a>, pull_input: Pull) -> Self {
        pin.set_as_input(pull_input);
        Self {
            is_output: false,
            pin,
            pull_input,
        }
    }
}

impl<'a> DbgPin for DebuggerPin<'a> {
    fn into_input(&mut self) {
        self.pin.set_as_input(self.pull_input);
        self.is_output = false;
    }
    fn into_output_in_state(&mut self, state: PinState) {
        match state {
            PinState::High => self.pin.set_high(),
            PinState::Low => self.pin.set_low(),
        }
        self.pin.set_as_output(Speed::High);
        self.is_output = true;
    }
    fn set_high(&mut self) {
        self.pin.set_high();
    }
    fn set_low(&mut self) {
        self.pin.set_low();
    }
    fn is_high(&self) -> bool {
        let level = match self.is_output {
            true => self.pin.get_output_level(),
            false => self.pin.get_level(),
        };
        match level {
            Level::High => true,
            Level::Low => false,
        }
    }
}

struct MyLeds<'a> {
    pin: Output<'a>,
}

impl<'a> DapLeds for MyLeds<'a> {
    fn react_to_host_status(&mut self, host_status: HostStatus) {
        match host_status {
            HostStatus::Connected(val) => match val {
                true => self.pin.set_high(),
                false => self.pin.set_low(),
            },
            HostStatus::Running(val) => match val {
                true => self.pin.set_high(),
                false => self.pin.set_low(),
            },
        }
    }
}
