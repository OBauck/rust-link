#![no_std]
#![no_main]

use ch32_hal as hal;
use dap_rs::dap::{Dap, DapLeds, DelayNs, HostStatus};
use embassy_executor::Spawner;
use embassy_usb::class::cmsis_dap_v2::{CmsisDapV2Class, State};
use embassy_usb::msos::windows_version;
use embedded_hal::digital::PinState;
use hal::gpio::{Flex, Level, Output, Pull, Speed};
use hal::pac::{systick, SYSTICK};
use hal::usbd::Driver;
use hal::{bind_interrupts, peripherals, println};

use ob_dap::{self, Context, DbgDelay, DbgPin, Swo};

use static_cell::StaticCell;

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

#[embassy_executor::task]
async fn dap_task(
    mut class: CmsisDapV2Class<'static, Driver<'static, peripherals::USBD>>,
    swdio_pin: Flex<'static>,
    swclk_pin: Flex<'static>,
    nreset_pin: Flex<'static>,
    led_pin: Output<'static>,
) -> ! {
    let swdio = DebuggerPin::new(swdio_pin, Pull::Down);
    let swclk = DebuggerPin::new(swclk_pin, Pull::Down);
    let nreset = DebuggerPin::new(nreset_pin, Pull::None);

    let bit_bang_delay = MyDelay::new(CPU_FREQUENCY);
    let wait = MyDelay::new(CPU_FREQUENCY);

    let leds = MyLeds { pin: led_pin };

    let context = Context::from_pins(swdio, swclk, nreset, CPU_FREQUENCY, bit_bang_delay);
    let swo: Option<Swo> = None;

    let mut dap_handler = Dap::new(context, leds, wait, swo, "V1");

    let mut report = [0; 64];
    let mut resp_buf = [0; 64];

    loop {
        class.wait_connection().await;
        println!("Connected");
        loop {
            let n = match class.read_packet(&mut report).await {
                Ok(val) => val,
                Err(_) => {
                    println!("Error!");
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
                    println!("Error!");
                    break;
                }
            }
        }
        println!("Disconnected");
    }
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) {
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
            Flex::new(p.PB3),
            Output::new(p.PA9, Level::High, Speed::High),
        ))
        .unwrap();

    usb.run().await;
}

struct MyDelay {
    ticks_per_us: u32,
}

impl MyDelay {
    fn new(cpu_frequency: u32) -> Self {
        SYSTICK.ctlr().modify(|w| {
            w.set_stclk(systick::vals::Stclk::HCLK);
            w.set_ste(true);
            w.set_mode(systick::vals::Mode::DOWNCOUNT);
        });
        Self {
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
        SYSTICK.cntl().read()
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
                true => self.pin.set_low(),
                false => self.pin.set_high(),
            },
            HostStatus::Running(val) => match val {
                true => self.pin.set_low(),
                false => self.pin.set_high(),
            },
        }
    }
}
