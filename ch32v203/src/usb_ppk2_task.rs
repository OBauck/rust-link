// About sampling rate
// CH32V203:
//  - 14 cycles minimum (12.5 + 1.5), 14 MHz maximum.
//  - System-clock needs to be 48MHz, 96MHz or 144MHz
//  - APB2 clock can be system clock divided by 2, 4, 8, 16
//  - ADC clock can be APB2 clock divided by 2, 4, 6 or 8 => 12MHz max => 857Ksps => 800Ksps using timer (divideable by 100khz)
// CH32L103:
//  - 20 cycles minimum at 48MHz => 2.4Msps
//  - system-clock max 96MHz

use ch32_hal::timer::{BasicInstance, CoreInstance};
use embassy_usb::class::cdc_acm::CdcAcmClass;

use ch32_hal::{self as hal, pac, Peri};
use hal::adc::{AdcChannel, AnyAdcChannel, SampleTime};
use hal::dma::{ReadableRingBuffer, Request, TransferOptions};
use hal::gpio::{Level, Output, Speed};
use hal::peripherals::{ADC1, DMA1_CH1, PA6, PB4, TIM3, USBD};
use hal::time::Hertz;
use hal::timer::low_level::Timer as LowLevelTimer;
use hal::usbd::Driver;
use rust_link_common::usb_ppk2::{run as ppk2_run, Adc};

use static_cell::StaticCell;

use super::my_println;

// Println fucks up because it is blocking
// When no debugger is connected, the fw just hangs
// TODO: Switch to ringbuffer + defmt (see defmt_rtt crate for inspiration or use this: https://crates.io/crates/defmt-bbq)
use my_println as println;

const SAMPLING_FREQUENCY_KHZ: usize = 800;
const ADC_BUFFER_SIZE: usize = 8 * 128 * 2; // RAM usage = (8 * 128 * 2) * 2 (bytes / u16) * 1.5 (data_buffer) = 6KB (6144bytes)
                                            // 2048 bytes adc_buffer + sampling frequency of 800KHz = interrupt every 1.28ms (1.25us * 1024)
const OVERSAMPLING_FACTOR: usize = SAMPLING_FREQUENCY_KHZ / 100;

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

#[embassy_executor::task]
pub async fn ppk2_task(
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
