// About sampling rate
// CH32V203:
//  - 14 cycles minimum (12.5 + 1.5), 14 MHz maximum.
//  - System-clock needs to be 48MHz, 96MHz or 144MHz
//  - APB2 clock can be system clock divided by 2, 4, 8, 16
//  - ADC clock can be APB2 clock divided by 2, 4, 6 or 8 => 12MHz max => 857Ksps => 800Ksps using timer (divideable by 100khz)
// CH32L103:
//  - 20 cycles minimum at 48MHz => 2.4Msps
//  - system-clock max 96MHz

#![no_std]
#![no_main]

use ch32_hal::timer::{BasicInstance, CoreInstance};
use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};

use ch32_hal::{self as hal, bind_interrupts, pac, Peri};
use hal::adc::{AdcChannel, AnyAdcChannel, SampleTime};
use hal::dma::{ReadableRingBuffer, Request, TransferOptions};
use hal::gpio::{Level, Output, Speed};
use hal::peripherals::{ADC1, DMA1_CH1, PA6, PB4, TIM3, USBD};
use hal::time::Hertz;
use hal::timer::low_level::Timer as LowLevelTimer;
use hal::usbd::Driver;

use rust_link_common::usb_ppk2::{run as ppk2_run, Adc};
use rust_link_common::usb_ppk2_dfu::Ppk2DfuClass;

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
});

const SAMPLING_FREQUENCY_KHZ: usize = 800;
const ADC_BUFFER_SIZE: usize = 8 * 128 * 2; // RAM usage = (8 * 128 * 2) * 2 (bytes / u16) * 1.5 (data_buffer) = 6KB (6144bytes)
                                            // 2048 bytes adc_buffer + sampling frequency of 800KHz = interrupt every 1.28ms (1.25us * 1024)
const OVERSAMPLING_FACTOR: usize = SAMPLING_FREQUENCY_KHZ / 100;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Printing out info here will use too much flash
    // Only print "Panic" to save flash size
    // let _ = println!("\n\n\n{}", _info);
    println!("Panic");

    loop {}
}

struct CH32Adc<'a, T: CoreInstance> {
    timer: LowLevelTimer<'a, T>,
    ring_buffer: ReadableRingBuffer<'a, u16>,
    data_buffer: &'static mut [u16],
}

impl<'a, T> CH32Adc<'a, T>
where
    T: BasicInstance,
{
    fn new(
        timer: LowLevelTimer<'a, T>,
        ring_buffer: ReadableRingBuffer<'a, u16>,
        data_buffer: &'static mut [u16],
    ) -> Self {
        Self {
            timer,
            ring_buffer,
            data_buffer,
        }
    }
}

impl<'a, T> Adc for CH32Adc<'a, T>
where
    T: BasicInstance,
{
    async fn read_data(&mut self) -> &[u16] {
        if let Err(_err) = self.ring_buffer.read_exact(self.data_buffer).await {
            println!("Error: {:?}", _err);
            self.ring_buffer.clear();
        }
        self.data_buffer
    }

    fn start(&mut self) {
        println!("Starting sampling");
        self.timer.start();
    }

    fn stop(&mut self) {
        println!("Stopping sampling");
        self.timer.stop();
        self.ring_buffer.clear();
    }
}

fn dma_setup<'d>(
    dma_channel: Peri<'d, DMA1_CH1>,
    adc_buffer: &'d mut [u16],
) -> ReadableRingBuffer<'d, u16> {
    let mut dma_options = TransferOptions::default();
    dma_options.circular = true;
    dma_options.half_transfer_ir = true;
    let mut ring_buf = unsafe {
        ReadableRingBuffer::new(
            dma_channel,
            Request::default(),
            pac::ADC1.rdatar().as_ptr() as *mut u16,
            adc_buffer,
            dma_options,
        )
    };
    ring_buf.start();
    ring_buf
}

fn adc_setup() {
    // Should be able to do this with "enable_and_reset_with_cs"
    pac::RCC.apb2pcenr().modify(|w| {
        w.set_adc1en(true);
    });

    // ADC1 does not have a ctlr3 register
    // ADC Clock = 72MHZ / 6 = 12 MHZ
    pac::RCC
        .cfgr0()
        .modify(|w| w.set_adcpre(pac::rcc::vals::Adcpre::DIV6));

    pac::ADC1.ctlr1().modify(|w| {
        w.set_dualmod(0); // Independent mode
        w.set_bufen(false);
        w.set_pga(pac::adc::vals::Pga::X1);
        w.set_scan(false);
    });

    pac::ADC1.ctlr2().modify(|w| {
        w.set_align(false); // align right
        w.set_extsel(pac::adc::vals::Extsel::TIM3_TRGO);
        w.set_cont(false);
        w.set_dma(true);
    });

    // ADC channel 6, 1.5 cycle sampling (14 cycles total)
    pac::ADC1.samptr2().modify(|w| {
        w.set_smp(6, SampleTime::CYCLES1_5);
    });
    pac::ADC1.rsqr3().modify(|w| w.set_sq(0, 6));

    // ADC on
    pac::ADC1.ctlr2().modify(|w| w.set_adon(true));

    // Calibrate
    pac::ADC1.ctlr2().modify(|w| w.set_rstcal(true));
    while pac::ADC1.ctlr2().read().rstcal() {}
    pac::ADC1.ctlr2().modify(|w| w.set_cal(true));
    while pac::ADC1.ctlr2().read().cal() {}

    // Enable external trigger
    pac::ADC1.ctlr2().modify(|w| w.set_exttrig(true));
}

#[embassy_executor::task]
async fn ppk2_task(
    class: CdcAcmClass<'static, Driver<'static, USBD>>,
    power_out_enable_pin: Peri<'static, PB4>,
    adc_pin: Peri<'static, PA6>,
    dma_channel: Peri<'static, DMA1_CH1>,
    timer: Peri<'static, TIM3>,
) -> ! {
    let mut power_out_enable = Output::new(power_out_enable_pin, Level::Low, Speed::Low);
    power_out_enable.set_high();

    static ADC_BUFFER: StaticCell<[u16; ADC_BUFFER_SIZE]> = StaticCell::new();
    let adc_buffer = ADC_BUFFER.init([0; ADC_BUFFER_SIZE]);
    static DATA_BUFFER: StaticCell<[u16; ADC_BUFFER_SIZE / 2]> = StaticCell::new();
    let data_buffer = DATA_BUFFER.init([0; ADC_BUFFER_SIZE / 2]);

    // This will call SealedAdcChannel::set_as_analog() which will in turn call SealedPin::set_as_analog(),
    // which will call this: self.set_mode_cnf(vals::Mode::INPUT, vals::Cnf::ANALOG_IN__PUSH_PULL_OUT);
    let _channel: AnyAdcChannel<ADC1> = adc_pin.degrade_adc();
    adc_setup();
    let ring_buf = dma_setup(dma_channel, adc_buffer);
    let tim = LowLevelTimer::new(timer);
    tim.set_frequency(Hertz(800_000));
    pac::TIM3
        .ctlr2()
        .modify(|w| w.set_mms(pac::timer::vals::Mms::UPDATE));

    let adc = CH32Adc::new(tim, ring_buf, data_buffer);

    ppk2_run(class, adc, power_out_enable, OVERSAMPLING_FACTOR, 12).await;
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) {
    hal::debug::SDIPrint::enable();
    let mut config = hal::Config::default();
    config.rcc = hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI;
    config.rcc.apb2_pre = ch32_hal::rcc::APBPrescaler::DIV2;
    let p = hal::init(config);

    println!("OB-Link PPK2!");
    let driver = Driver::new(p.USBD, Irqs, p.PA12, p.PA11);

    // Create embassy-usb Config
    // usb vid and pid needs to be Nordic Semiconductor for power profiler application to recognize it
    let mut config = embassy_usb::Config::new(0x1915, 0xc00a);
    config.manufacturer = Some("Bauck");
    config.product = Some("OB-Link (PPK2)");
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

    Ppk2DfuClass::new(&mut builder);

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    // Build the builder.
    let mut usb = builder.build();

    spawner
        .spawn(ppk2_task(class, p.PB4, p.PA6, p.DMA1_CH1, p.TIM3))
        .unwrap();

    // Run the USB device.
    usb.run().await;
}
