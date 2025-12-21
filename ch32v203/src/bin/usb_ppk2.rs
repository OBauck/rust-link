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

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;

use ch32_hal::{
    self as hal, bind_interrupts, pac, println as ch32_println, Peripheral, PeripheralRef,
};
use embedded_hal::digital::OutputPin;
use hal::adc::{AdcChannel, AnyAdcChannel, SampleTime};
use hal::dma::{ReadableRingBuffer, Request, TransferOptions};
use hal::gpio::{Level, Output, Speed};
use hal::peripherals::{ADC1, DMA1_CH1, PA6, PB4, TIM3, USBD};
use hal::time::Hertz;
use hal::timer::low_level::Timer as LowLevelTimer;
use hal::usbd::{Driver, Instance as UsbInstance};

use bitfield_struct::bitfield;
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

#[bitfield(u32)]
pub struct Ppk2MeasurementData {
    #[bits(14)]
    pub adc_data: u16,
    #[bits(3)]
    pub range: u8,
    #[bits(6)]
    pub counter: u8,
    #[bits(8)]
    pub logic_data: u8,
    #[bits(1)]
    pub rsvd: bool,
}

#[repr(u8)]
enum Commands {
    NoOp = 0x00,
    TriggerSet = 0x01,
    AvgNumSet = 0x02,
    TriggerWindowSet = 0x03,
    TriggerIntervalSet = 0x04,
    TriggerSingleSet = 0x05,
    AverageStart = 0x06,
    AverageStop = 0x07,
    RangeSet = 0x08,
    LcdSet = 0x09,
    TriggerStop = 0x0A,
    DeviceRunningSet = 0x0C,
    RegulatorSet = 0x0D,
    SwitchPointDown = 0x0E,
    SwitchPointUp = 0x0F,
    TriggerExtToggle = 0x10,
    SetPowerMode = 0x11,
    ResUserSet = 0x12,
    SpikeFilteringOn = 0x15,
    SpikeFilteringOff = 0x16,
    GetMetaData = 0x19,
    Reset = 0x20,
    SetUserGains = 0x25,
    Unkown(u8),
}

impl From<u8> for Commands {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Commands::NoOp,
            0x01 => Commands::TriggerSet,
            0x02 => Commands::AvgNumSet,
            0x03 => Commands::TriggerWindowSet,
            0x04 => Commands::TriggerIntervalSet,
            0x05 => Commands::TriggerSingleSet,
            0x06 => Commands::AverageStart,
            0x07 => Commands::AverageStop,
            0x08 => Commands::RangeSet,
            0x09 => Commands::LcdSet,
            0x0A => Commands::TriggerStop,
            0x0C => Commands::DeviceRunningSet,
            0x0D => Commands::RegulatorSet,
            0x0E => Commands::SwitchPointDown,
            0x0F => Commands::SwitchPointUp,
            0x10 => Commands::TriggerExtToggle,
            0x11 => Commands::SetPowerMode,
            0x12 => Commands::ResUserSet,
            0x15 => Commands::SpikeFilteringOn,
            0x16 => Commands::SpikeFilteringOff,
            0x19 => Commands::GetMetaData,
            0x20 => Commands::Reset,
            0x25 => Commands::SetUserGains,
            _ => Commands::Unkown(value),
        }
    }
}

// TODO: Adjust these to match 2.2ohm shunt + gain of 50 ++
static PPK2_META_DATA: &[u8] = concat!(
    "Calibrated: 0\n",
    "R0: 0.535\n", // Shunt resistor value
    "R1: 1.0\n",
    "R2: 1.0\n",
    "R3: 1.0\n",
    "R4: 1.0\n",
    "GS0: 0.00215\n",
    "GS1: 0.0\n",
    "GS2: 0.0\n",
    "GS3: 0.0\n",
    "GS4: 0.0\n",
    "GI0: 0.0215\n",
    "GI1: 1.0\n",
    "GI2: 1.0\n",
    "GI3: 1.0\n",
    "GI4: 1.0\n",
    "O0: 0.0\n", // Offset voltage on ADC
    "O1: 0.0\n",
    "O2: 0.0\n",
    "O3: 0.0\n",
    "O4: 0.0\n",
    "VDD: 3300\n",
    "HW: 2663\n",
    "mode: 1\n", // 1 = Ampere, 2 = SMU
    "S0: 0.0\n",
    "S1: 0.0\n",
    "S2: 0.0\n",
    "S3: 0.0\n",
    "S4: 0.0\n",
    "I0: -0.00023\n", // Offset current (?) TODO: Use offset voltage instead (should be 231) -> need to figure out how calculation works
    "I1: 0.0\n",
    "I2: 0.0\n",
    "I3: 0.0\n",
    "I4: 0.0\n",
    "UG0: 1.0\n", // User gain, needs to be between 0.9 and 1.1
    "UG1: 1.0\n",
    "UG2: 1.0\n",
    "UG3: 1.0\n",
    "UG4: 1.0\n",
    "IA: 56\n",
    "END\n",
)
.as_bytes();

const SAMPLING_FREQUENCY_KHZ: usize = 800;
const ADC_BUFFER_SIZE: usize = 8 * 128 * 2; // RAM usage = (8 * 128 * 2) * 2 (bytes / u16) * 1.5 (data_buffer) = 6KB (6144bytes)
                                            // 2048 bytes adc_buffer + sampling frequency of 800KHz = interrupt every 1.28ms (1.25us * 1024)
const OVERSAMPLING_FACTOR: usize = SAMPLING_FREQUENCY_KHZ / 100;
const PPK2_SAMPLES_PER_USB_PACKET: usize = 16;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Printing out info here will use too much flash
    // Only print "Panic" to save flash size
    // let _ = println!("\n\n\n{}", _info);
    println!("Panic");

    loop {}
}

fn dma_setup<'d>(
    dma_channel: PeripheralRef<'d, DMA1_CH1>,
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
    mut class: CdcAcmClass<'static, Driver<'static, USBD>>,
    power_out_enable_pin: PB4,
    adc_pin: PA6,
    dma_channel: PeripheralRef<'static, DMA1_CH1>,
    timer: TIM3,
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
    let mut ring_buf = dma_setup(dma_channel, adc_buffer);
    let tim = LowLevelTimer::new(timer);
    tim.set_frequency(Hertz(800_000));
    pac::TIM3
        .ctlr2()
        .modify(|w| w.set_mms(pac::timer::vals::Mms::UPDATE));

    println!("Starting sampling");
    tim.start();

    loop {
        class.wait_connection().await;
        println!("Connected");
        let _ = ppk2_run(
            &mut class,
            &mut ring_buf,
            data_buffer,
            &mut power_out_enable,
        )
        .await;
        println!("Disconnected");
    }
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
    let mut config = embassy_usb::Config::new(0xC0DE, 0xCAFE);
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

    let class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    // Build the builder.
    let mut usb = builder.build();

    spawner
        .spawn(ppk2_task(
            class,
            p.PB4,
            p.PA6,
            p.DMA1_CH1.into_ref(),
            p.TIM3,
        ))
        .unwrap();

    // Run the USB device.
    usb.run().await;
}

struct Disconnected {}

impl From<EndpointError> for Disconnected {
    fn from(val: EndpointError) -> Self {
        match val {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Disconnected {},
        }
    }
}

/*
    Every data is 32bit, with this composition (nr_of_bits, bit_position)
    const MEAS_ADC = generateMask(14, 0);
    const MEAS_RANGE = generateMask(3, 14);
    const MEAS_COUNTER = generateMask(6, 18);
    const MEAS_LOGIC = generateMask(8, 24);
*/

fn make_ppk2_measurement_data(
    adc_raw_data: &[u16],
    adc_bit_resolution: usize,
    counter: u32,
    range: u8,
) -> u32 {
    let mut adc_data_sum = 0_u32;
    for &raw_data in adc_raw_data {
        adc_data_sum += raw_data as u32;
    }

    let adc_data = match adc_bit_resolution {
        0..14 => adc_data_sum * (1 << (14 - adc_bit_resolution)) / adc_raw_data.len() as u32,
        14 => adc_data_sum / adc_raw_data.len() as u32,
        15.. => adc_data_sum / ((1 << (adc_bit_resolution - 14)) * adc_raw_data.len() as u32),
    };

    (adc_data & 0x3fff) | ((range as u32 & 0b111) << 14) | ((counter & 0b111111) << 18)
}

async fn ppk2_process_command<'d, T: UsbInstance + 'd, OUT: OutputPin>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    usb_data: &[u8],
    send_data_enable: &mut bool,
    power_out_enable: &mut OUT,
) -> Result<(), Disconnected> {
    let command = Commands::from(usb_data[0]);
    println!("Command data len: {}", usb_data.len());

    match command {
        Commands::GetMetaData => {
            println!("Sending metadata");
            for chunk in PPK2_META_DATA.chunks(64) {
                class.write_packet(chunk).await?;
            }
        }
        Commands::AverageStart => {
            println!("Average start");
            *send_data_enable = true;
        }
        Commands::AverageStop => {
            println!("Average stop");
            *send_data_enable = false;
        }
        Commands::DeviceRunningSet => {
            match usb_data[1] {
                0 => {
                    let _ = power_out_enable.set_low();
                    println!("Power out disable");
                }
                1 => {
                    let _ = power_out_enable.set_high();
                    println!("Power out enable");
                }
                data => println!("Invalid DeviceRunningSet data: {}", data),
            }
            println!("Device running: {}", usb_data[1])
        }
        Commands::RegulatorSet => {
            println!(
                "Regulator set: {}",
                ((usb_data[1] as u16) << 8) | usb_data[2] as u16
            );
        }
        Commands::Unkown(val) => println!("Unknown command: 0x{:x}", val),
        _ => println!("Unsupported command: 0x{:x}", usb_data[0]),
    }
    Ok(())
}

async fn ppk2_run<'d, T: UsbInstance + 'd, OUT: OutputPin>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    ring_buffer: &mut ReadableRingBuffer<'d, u16>,
    data_buffer: &mut [u16],
    power_out_enable: &mut OUT,
) -> Result<(), Disconnected> {
    let mut buf = [0; 64];
    let mut measurement_counter = 0_u32;
    let mut usb_data = [0_u32; PPK2_SAMPLES_PER_USB_PACKET];
    let mut send_data_enable = false;

    loop {
        // TODO: Stop sampling if send_data_enable is 0
        match select(
            ring_buffer.read_exact(data_buffer),
            class.read_packet(&mut buf),
        )
        .await
        {
            Either::First(res) => {
                if let Err(err) = res {
                    println!("Error: {:?}", err);
                    ring_buffer.clear();
                    continue;
                }
                if send_data_enable {
                    // Original ppk2 sends 16 * 128 / 4 = 512 samples in bulk
                    let mut i = 0;
                    for data_chunk in data_buffer.chunks(OVERSAMPLING_FACTOR) {
                        let data =
                            make_ppk2_measurement_data(data_chunk, 12, measurement_counter, 0);
                        usb_data[i] = data;
                        measurement_counter += 1;
                        i += 1;
                        if i == PPK2_SAMPLES_PER_USB_PACKET {
                            class.write_packet(bytemuck::cast_slice(&usb_data)).await?;
                            i = 0;
                        }
                    }
                }
            }
            Either::Second(res) => {
                let n = res?;
                ppk2_process_command(class, &buf[..n], &mut send_data_enable, power_out_enable)
                    .await?;
            }
        }
    }
}
