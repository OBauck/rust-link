#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Flex, Pull, Speed};
use embassy_stm32::usb::{self, Driver};
use embassy_stm32::{Config, bind_interrupts, peripherals};
use embassy_usb::class::cmsis_dap_v2::{CmsisDapV2Class, State};
use embassy_usb::msos::windows_version;
use {defmt_rtt as _, panic_probe as _};

use bitbang_dap::{BitbangAdapter, DelayCycles, InputOutputPin};
use dap_rs::dap::{self, Dap, DapLeds, DelayNs};
use dap_rs::jtag::TapConfig;
use dap_rs::swo::Swo;

use static_cell::{ConstStaticCell, StaticCell};

bind_interrupts!(struct Irqs {
    USB_DRD_FS => usb::InterruptHandler<peripherals::USB>;
});

const CPU_FREQUENCY: u32 = 48_000_000;
const MAX_SCAN_CHAIN_LENGTH: usize = 8;

#[embassy_executor::task]
async fn dap_task(
    mut class: CmsisDapV2Class<'static, Driver<'static, peripherals::USB>>,
    deps: BitbangAdapter<IoPin<'static>, BitDelay>,
) -> ! {
    let mut dap = Dap::new(
        deps,
        LedSignals,
        BitDelay,
        None::<NoSwo>,
        concat!("2.1.0, Adaptor version ", env!("CARGO_PKG_VERSION")),
    );

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
            let len = dap.process_command(&report[..n], &mut resp_buf, dap_rs::dap::DapVersion::V2);
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

    let driver = Driver::new(p.USB, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0x1a86, 0x7021);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link-STM32C071 (CMSIS-DAP v2)"); // Need to have "CMSIS-DAP" in the product name to make probe-rs recognize it
    config.serial_number = Some("12345678");

    config.max_power = 100;
    config.max_packet_size_0 = 64;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static MSOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();

        let builder = embassy_usb::Builder::new(
            driver,
            config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            MSOS_DESCRIPTOR.init([0; 256]),
            CONTROL_BUF.init([0; 128]),
        );
        builder
    };

    builder.msos_descriptor(windows_version::WIN8_1, 0);

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CmsisDapV2Class::new(&mut builder, state, 64, false)
    };

    // Build the builder.
    let mut usb = builder.build();

    static SCAN_CHAIN: ConstStaticCell<[TapConfig; MAX_SCAN_CHAIN_LENGTH]> =
        ConstStaticCell::new([TapConfig::INIT; MAX_SCAN_CHAIN_LENGTH]);

    let deps = BitbangAdapter::new(
        IoPin::new(Flex::new(p.PA1)),
        IoPin::new(Flex::new(p.PA6)),
        IoPin::new(Flex::new(p.PA2)),
        IoPin::new(Flex::new(p.PA3)),
        IoPin::new(Flex::new(p.PA7)),
        BitDelay,
        SCAN_CHAIN.take(),
    );

    spawner.spawn(dap_task(class, deps)).unwrap();

    usb.run().await;
}

struct LedSignals;

impl DapLeds for LedSignals {
    fn react_to_host_status(&mut self, _host_status: dap::HostStatus) {}
}

struct BitDelay;

impl DelayNs for BitDelay {
    fn delay_ns(&mut self, ns: u32) {
        self.delay_cycles((ns as u64 * self.cpu_clock() as u64 / 1_000_000_000_u64) as u32);
    }
}

impl DelayCycles for BitDelay {
    fn delay_cycles(&mut self, cycles: u32) {
        cortex_m::asm::delay(cycles);
    }

    fn cpu_clock(&self) -> u32 {
        // This function is used to calculate the number of cycles to wait in a SWD/JTAG clock
        // cycle, so we don't actually have to return the real CPU frequency.
        // cortex_m __delay divides by 2 and Cortex-M0+ needs 4 CPU cycles per delay loop iteration.
        CPU_FREQUENCY / 2
    }
}

struct IoPin<'a> {
    pin: Flex<'a>,
}

impl<'a> IoPin<'a> {
    fn new(pin: Flex<'a>) -> Self {
        Self { pin }
    }
}

impl InputOutputPin for IoPin<'_> {
    fn set_as_output(&mut self) {
        self.pin.set_as_output(Speed::High);
    }

    fn set_high(&mut self, high: bool) {
        match high {
            true => self.pin.set_high(),
            false => self.pin.set_low(),
        }
    }

    fn set_as_input(&mut self) {
        self.pin.set_as_input(Pull::None);
    }

    fn is_high(&mut self) -> bool {
        self.pin.is_high()
    }
}

struct NoSwo;

impl Swo for NoSwo {
    fn set_transport(&mut self, _transport: dap_rs::swo::SwoTransport) {
        todo!()
    }

    fn set_mode(&mut self, _mode: dap_rs::swo::SwoMode) {
        todo!()
    }

    fn set_baudrate(&mut self, _baudrate: u32) -> u32 {
        todo!()
    }

    fn set_control(&mut self, _control: dap_rs::swo::SwoControl) {
        todo!()
    }

    fn polling_data(&mut self, _buf: &mut [u8]) -> u32 {
        todo!()
    }

    fn streaming_data(&mut self) {
        todo!()
    }

    fn is_active(&self) -> bool {
        todo!()
    }

    fn bytes_available(&self) -> u32 {
        todo!()
    }

    fn buffer_size(&self) -> u32 {
        todo!()
    }

    fn support(&self) -> dap_rs::swo::SwoSupport {
        todo!()
    }

    fn status(&mut self) -> dap_rs::swo::SwoStatus {
        todo!()
    }
}
