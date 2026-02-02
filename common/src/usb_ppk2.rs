use bitfield_struct::bitfield;
use embassy_futures::select::{select, Either};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::{Driver, EndpointError};
use embedded_hal::digital::OutputPin;

const PPK2_SAMPLES_PER_USB_PACKET: usize = 16;

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

#[allow(async_fn_in_trait)]
pub trait Adc {
    async fn read_data(&mut self) -> &[u16];
    fn start(&mut self);
    fn stop(&mut self);
}

pub async fn run<'d, D, ADC, OUT>(
    mut class: CdcAcmClass<'d, D>,
    mut adc: ADC,
    mut power_out_enable: OUT,
    oversampling_factor: usize,
    adc_bit_resolution: usize,
) -> !
where
    D: Driver<'d>,
    ADC: Adc,
    OUT: OutputPin,
{
    let mut usb_read_buf = [0; 64];
    let mut usb_ppk2_data = [0_u32; PPK2_SAMPLES_PER_USB_PACKET];
    let mut send_data_enable = false;
    let mut measurement_counter = 0_u32;

    loop {
        class.wait_connection().await;
        loop {
            match select(adc.read_data(), class.read_packet(&mut usb_read_buf)).await {
                Either::First(adc_data) => {
                    if send_data_enable {
                        // Original ppk2 sends 16 * 128 / 4 = 512 samples in bulk
                        let mut i = 0;

                        for data_chunk in adc_data.chunks(oversampling_factor) {
                            let data = make_ppk2_measurement_data(
                                data_chunk,
                                adc_bit_resolution,
                                measurement_counter,
                                0,
                            );
                            usb_ppk2_data[i] = data;
                            measurement_counter += 1;
                            i += 1;
                            if i == PPK2_SAMPLES_PER_USB_PACKET {
                                if let Err(_err) = class
                                    .write_packet(bytemuck::cast_slice(&usb_ppk2_data))
                                    .await
                                {
                                    #[cfg(feature = "defmt")]
                                    defmt::error!(
                                        "Usb write error: {:?}. Assume disconnection",
                                        defmt::Debug2Format(&_err)
                                    );
                                    break;
                                }
                                i = 0;
                            }
                        }
                    }
                }
                Either::Second(res) => match res {
                    Err(_err) => {
                        #[cfg(feature = "defmt")]
                        defmt::error!(
                            "Usb read error: {:?}. Assume disconnection",
                            defmt::Debug2Format(&_err)
                        );
                        break;
                    }
                    Ok(n) => {
                        if let Err(_err) = ppk2_process_command(
                            &mut class,
                            &usb_read_buf[..n],
                            &mut send_data_enable,
                            &mut power_out_enable,
                            &mut adc,
                        )
                        .await
                        {
                            #[cfg(feature = "defmt")]
                            defmt::error!(
                                "Usb write error: {:?}. Assume disconnection",
                                defmt::Debug2Format(&_err)
                            );
                            break;
                        }
                    }
                },
            }
        }
    }
}

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

async fn ppk2_process_command<'d, D: Driver<'d>, OUT: OutputPin, ADC: Adc>(
    class: &mut CdcAcmClass<'d, D>,
    usb_data: &[u8],
    send_data_enable: &mut bool,
    power_out_enable: &mut OUT,
    adc: &mut ADC,
) -> Result<(), EndpointError> {
    let command = Commands::from(usb_data[0]);

    match command {
        Commands::GetMetaData => {
            #[cfg(feature = "defmt")]
            defmt::trace!("Sending metadata");
            for chunk in PPK2_META_DATA.chunks(64) {
                class.write_packet(chunk).await?;
            }
        }
        Commands::AverageStart => {
            #[cfg(feature = "defmt")]
            defmt::trace!("Average start");
            *send_data_enable = true;
            adc.start();
        }
        Commands::AverageStop => {
            #[cfg(feature = "defmt")]
            defmt::trace!("Average stop");
            *send_data_enable = false;
            adc.stop();
        }
        Commands::DeviceRunningSet => match usb_data[1] {
            0 => {
                let _ = power_out_enable.set_low();
                #[cfg(feature = "defmt")]
                defmt::trace!("Power out disable");
            }
            1 => {
                let _ = power_out_enable.set_high();
                #[cfg(feature = "defmt")]
                defmt::trace!("Power out enable");
            }
            _data => {
                #[cfg(feature = "defmt")]
                defmt::trace!("Invalid DeviceRunningSet data: {}", _data)
            }
        },
        Commands::RegulatorSet => {
            #[cfg(feature = "defmt")]
            defmt::trace!(
                "Regulator set: {}",
                ((usb_data[1] as u16) << 8) | usb_data[2] as u16
            );
        }
        Commands::Unkown(_val) => {
            #[cfg(feature = "defmt")]
            defmt::trace!("Unknown command: 0x{:x}", _val)
        }
        _ => {
            #[cfg(feature = "defmt")]
            defmt::trace!("Unsupported command: 0x{:x}", usb_data[0])
        }
    }
    Ok(())
}
