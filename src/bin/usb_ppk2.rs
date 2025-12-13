#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::Builder;
use hal::usbd::{Driver, Instance};
use hal::{bind_interrupts, println};
use {ch32_hal as hal, panic_halt as _};

use bitfield_struct::bitfield;

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

static PPK2_META_DATA: &[u8] = concat!(
    "Calibrated: 0\n",
    "R0: 1.0\n", // Shunt resistor value
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

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(_spawner: Spawner) {
    let p = hal::init(hal::Config {
        rcc: hal::rcc::Config::SYSCLK_FREQ_144MHZ_HSI,
        ..Default::default()
    });

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

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];

    let mut state = State::new();

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [], // no msos descriptors
        &mut control_buf,
    );

    // Create classes on the builder.
    let mut class = CdcAcmClass::new(&mut builder, &mut state, 64);

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    let usb_fut = usb.run();

    // Do stuff with the class!
    let echo_fut = async {
        loop {
            class.wait_connection().await;
            println!("Connected");
            let _ = ppk2_run(&mut class).await;
            println!("Disconnected");
        }
    };

    // Run everything concurrently.
    // If we had made everything `'static` above instead, we could do this using separate tasks instead.
    join(usb_fut, echo_fut).await;
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

// fn ppk2_process_data<const N: usize, const M: usize>(
//     adc_data: &[u16; M],
//     ppk2_data: &mut [u16; N],
//     adc_data_bit_resolution: usize,
// ) {
//     static
//     static NR_OF_AVERAGE: usize = M / N;
//     let mut sum = 0_u32;
//     let mut average_count = 0;
//     let mut count = 0;
//     for data in adc_data {
//         sum += data;
//         average_count += 1;
//         if average_count == NR_OF_AVERAGE {
//             ppk2_data[count] = sum / NR_OF_AVERAGE;
//         }
//     }
// }

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

async fn ppk2_process_command<'d, T: Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    usb_data: &[u8],
    send_data_enable: &mut bool,
) -> Result<(), Disconnected> {
    let command = Commands::from(usb_data[0]);

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
        Commands::Unkown(val) => println!("Unknown command: 0x{:x}", val),
        _ => println!("Unsupported command: 0x{:x}", usb_data[0]),
    }
    Ok(())
}

async fn ppk2_run<'d, T: Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
) -> Result<(), Disconnected> {
    let mut buf = [0; 64];
    let mut measurement_counter = 0_u32;
    let mut usb_data = [0_u32; 16];
    let mut counter = 0_u16;
    let mut send_data_enable = false;
    let usb_send_interval = Duration::from_micros(5120);
    let mut deadline = Instant::now() + usb_send_interval;

    loop {
        match select(Timer::at(deadline), class.read_packet(&mut buf)).await {
            Either::First(_) => {
                deadline += usb_send_interval;
                if send_data_enable {
                    // ppk2 sends 16 * 128 / 4 = 512 samples in bulk
                    for _ in 0..32 {
                        for data in usb_data.iter_mut() {
                            *data =
                                make_ppk2_measurement_data(&[counter], 14, measurement_counter, 0);
                            // *data = counter as u32 & 0x3fff
                            //     | ((measurement_counter & 0x3f) << 18);
                            measurement_counter += 1;
                            counter += 1;
                        }
                        class.write_packet(bytemuck::cast_slice(&usb_data)).await?;
                    }
                }
            }
            Either::Second(res) => {
                let n = res?;
                ppk2_process_command(class, &buf[..n], &mut send_data_enable).await?;
            }
        }
    }
}
