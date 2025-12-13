#![no_std]
#![no_main]

use dap_rs::{swj::Dependencies, *};
use embedded_hal::digital::PinState;

pub trait DbgPin {
    fn into_input(&mut self);
    fn into_output_in_state(&mut self, state: PinState);
    fn set_low(&mut self);
    fn set_high(&mut self);
    fn is_high(&self) -> bool;
}

pub trait DbgDelay {
    fn get_current(&self) -> u32;
    fn delay_ticks_from_last(&self, ticks: u32, last: u32) -> u32;
}

pub struct Context<P, D> {
    max_frequency: u32,
    cpu_frequency: u32,
    cycles_per_us: u32,
    half_period_ticks: u32,
    delay: D,
    swdio: P,
    swclk: P,
    nreset: P,
}

impl<P: DbgPin, D: DbgDelay> core::fmt::Debug for Context<P, D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Context")
            .field("max_frequency", &self.max_frequency)
            .field("cpu_frequency", &self.cpu_frequency)
            .field("cycles_per_us", &self.cycles_per_us)
            .field("half_period_ticks", &self.half_period_ticks)
            .finish()
    }
}

impl<P: DbgPin, D: DbgDelay> Context<P, D> {
    fn swdio_to_input(&mut self) {
        self.swdio.into_input();
    }

    fn swdio_to_output(&mut self) {
        self.swdio.into_output_in_state(PinState::High);
    }

    fn swclk_to_input(&mut self) {
        self.swclk.into_input();
    }

    fn swclk_to_output(&mut self) {
        self.swclk.into_output_in_state(PinState::High);
    }

    pub fn from_pins(swdio: P, swclk: P, nreset: P, cpu_frequency: u32, delay: D) -> Self {
        let max_frequency = 100_000;
        let half_period_ticks = cpu_frequency / max_frequency / 2;
        Context {
            max_frequency,
            cpu_frequency,
            cycles_per_us: cpu_frequency / 1_000_000,
            half_period_ticks,
            delay,
            swdio,
            swclk,
            nreset,
        }
    }
}

impl<P: DbgPin, D: DbgDelay> swj::Dependencies<Swd<P, D>, Jtag<P, D>> for Context<P, D> {
    fn process_swj_pins(&mut self, output: swj::Pins, mask: swj::Pins, wait_us: u32) -> swj::Pins {
        if mask.contains(swj::Pins::SWCLK) {
            self.swclk_to_output();
            if output.contains(swj::Pins::SWCLK) {
                self.swclk.set_high();
            } else {
                self.swclk.set_low();
            }
        }

        if mask.contains(swj::Pins::SWDIO) {
            self.swdio_to_output();
            if output.contains(swj::Pins::SWDIO) {
                self.swdio.set_high();
            } else {
                self.swdio.set_low();
            }
        }

        if mask.contains(swj::Pins::NRESET) {
            if output.contains(swj::Pins::NRESET) {
                // "open drain disconnect"
                self.nreset.into_input();
            } else {
                self.nreset.into_output_in_state(PinState::Low);
            }
        }

        // Delay until desired output state or timeout.
        let mut last = self.delay.get_current();
        for _ in 0..wait_us {
            last = self.delay.delay_ticks_from_last(self.cycles_per_us, last);

            // If a pin is selected, make sure its output equals the desired output state, or else
            // continue waiting.
            let swclk_not_in_desired_state = mask.contains(swj::Pins::SWCLK)
                && output.contains(swj::Pins::SWCLK) != self.swclk.is_high();
            let swdio_not_in_desired_state = mask.contains(swj::Pins::SWDIO)
                && output.contains(swj::Pins::SWDIO) != self.swdio.is_high();
            let nreset_not_in_desired_state = mask.contains(swj::Pins::NRESET)
                && output.contains(swj::Pins::NRESET) != self.nreset.is_high();

            if swclk_not_in_desired_state
                || swdio_not_in_desired_state
                || nreset_not_in_desired_state
            {
                continue;
            }

            break;
        }

        let mut ret = swj::Pins::empty();
        self.swclk_to_input();
        ret.set(swj::Pins::SWCLK, self.swclk.is_high());
        self.swdio_to_input();
        ret.set(swj::Pins::SWDIO, self.swdio.is_high());
        self.nreset.into_input();
        ret.set(swj::Pins::NRESET, self.nreset.is_high());

        ret
    }

    fn process_swj_sequence(&mut self, data: &[u8], mut bits: usize) {
        self.swclk_to_output();
        self.swdio_to_output();

        let half_period_ticks = self.half_period_ticks;
        let mut last = self.delay.get_current();
        last = self.delay.delay_ticks_from_last(half_period_ticks, last);

        for byte in data {
            let mut byte = *byte;
            let frame_bits = core::cmp::min(bits, 8);
            for _ in 0..frame_bits {
                let bit = byte & 1;
                byte >>= 1;
                if bit != 0 {
                    self.swdio.set_high();
                } else {
                    self.swdio.set_low();
                }
                self.swclk.set_low();
                last = self.delay.delay_ticks_from_last(half_period_ticks, last);
                self.swclk.set_high();
                last = self.delay.delay_ticks_from_last(half_period_ticks, last);
            }
            bits -= frame_bits;
        }
    }

    fn process_swj_clock(&mut self, max_frequency: u32) -> bool {
        if max_frequency < self.cpu_frequency {
            self.max_frequency = max_frequency;
            self.half_period_ticks = self.cpu_frequency / self.max_frequency / 2;
            true
        } else {
            false
        }
    }

    fn high_impedance_mode(&mut self) {
        self.swdio_to_input();
        self.swclk_to_input();
        self.nreset.into_input();
    }
}

pub struct Jtag<P, D>(Context<P, D>);

impl<P, D> From<Jtag<P, D>> for Context<P, D> {
    fn from(value: Jtag<P, D>) -> Self {
        value.0
    }
}

impl<P, D> From<Context<P, D>> for Jtag<P, D> {
    fn from(value: Context<P, D>) -> Self {
        Self(value)
    }
}

impl<P: DbgPin, D: DbgDelay> jtag::Jtag<Context<P, D>> for Jtag<P, D> {
    const AVAILABLE: bool = false;

    fn sequences(&mut self, _data: &[u8], _rxbuf: &mut [u8]) -> u32 {
        0
    }

    fn set_clock(&mut self, max_frequency: u32) -> bool {
        self.0.process_swj_clock(max_frequency)
    }
}

pub struct Swd<P, D>(Context<P, D>);

impl<P, D> From<Swd<P, D>> for Context<P, D> {
    fn from(value: Swd<P, D>) -> Self {
        value.0
    }
}

impl<P: DbgPin, D: DbgDelay> From<Context<P, D>> for Swd<P, D> {
    fn from(mut value: Context<P, D>) -> Self {
        // Maybe this should go to some `Swd::new`
        value.swdio_to_output();
        value.swclk_to_output();
        value.nreset.into_input();

        Self(value)
    }
}

impl<P: DbgPin, D: DbgDelay> swd::Swd<Context<P, D>> for Swd<P, D> {
    const AVAILABLE: bool = true;

    fn read_inner(&mut self, apndp: swd::APnDP, a: swd::DPRegister) -> swd::Result<u32> {
        // Send request
        let req = swd::make_request(apndp, swd::RnW::R, a);
        self.tx8(req);

        // Read ack, 1 clock for turnaround and 3 for ACK
        let ack = self.rx4() >> 1;

        match swd::Ack::try_ok(ack as u8) {
            Ok(_) => (),
            Err(e) => {
                // On non-OK ACK, target has released the bus but
                // is still expecting a turnaround clock before
                // the next request, and we need to take over the bus.
                self.tx8(0);
                return Err(e);
            }
        }

        // Read data and parity
        let (data, parity) = self.read_data();

        // Turnaround + trailing
        let mut last = self.0.delay.get_current();
        self.read_bit(&mut last);
        self.tx8(0); // Drive the SWDIO line to 0 to not float

        if parity as u8 == (data.count_ones() as u8 & 1) {
            Ok(data)
        } else {
            Err(swd::Error::BadParity)
        }
    }

    fn write_inner(&mut self, apndp: swd::APnDP, a: swd::DPRegister, data: u32) -> swd::Result<()> {
        // Send request
        let req = swd::make_request(apndp, swd::RnW::W, a);
        self.tx8(req);

        // Read ack, 1 clock for turnaround and 3 for ACK and 1 for turnaround
        let ack = (self.rx5() >> 1) & 0b111;
        match swd::Ack::try_ok(ack as u8) {
            Ok(_) => (),
            Err(e) => {
                // On non-OK ACK, target has released the bus but
                // is still expecting a turnaround clock before
                // the next request, and we need to take over the bus.
                self.tx8(0);
                return Err(e);
            }
        }

        // Send data and parity
        let parity = data.count_ones() & 1 == 1;
        self.send_data(data, parity);

        // Send trailing idle
        self.tx8(0);

        Ok(())
    }

    fn write_sequence(&mut self, mut num_bits: usize, data: &[u8]) -> swd::Result<()> {
        self.0.swdio_to_output();
        let mut last = self.0.delay.get_current();

        for b in data {
            let bit_count = core::cmp::min(num_bits, 8);
            for i in 0..bit_count {
                self.write_bit((b >> i) & 0x1, &mut last);
            }
            num_bits -= bit_count;
        }

        Ok(())
    }

    fn read_sequence(&mut self, mut num_bits: usize, data: &mut [u8]) -> swd::Result<()> {
        self.0.swdio_to_input();
        let mut last = self.0.delay.get_current();

        for b in data {
            let bit_count = core::cmp::min(num_bits, 8);
            for i in 0..bit_count {
                let bit = self.read_bit(&mut last);
                *b |= bit << i;
            }
            num_bits -= bit_count;
        }

        Ok(())
    }

    fn set_clock(&mut self, max_frequency: u32) -> bool {
        self.0.process_swj_clock(max_frequency)
    }
}

impl<P: DbgPin, D: DbgDelay> Swd<P, D> {
    fn tx8(&mut self, mut data: u8) {
        self.0.swdio_to_output();

        let mut last = self.0.delay.get_current();

        for _ in 0..8 {
            self.write_bit(data & 1, &mut last);
            data >>= 1;
        }
    }

    fn rx4(&mut self) -> u8 {
        self.0.swdio_to_input();

        let mut data = 0;
        let mut last = self.0.delay.get_current();

        for i in 0..4 {
            data |= (self.read_bit(&mut last) & 1) << i;
        }

        data
    }

    fn rx5(&mut self) -> u8 {
        self.0.swdio_to_input();

        let mut last = self.0.delay.get_current();

        let mut data = 0;

        for i in 0..5 {
            data |= (self.read_bit(&mut last) & 1) << i;
        }

        data
    }

    fn send_data(&mut self, mut data: u32, parity: bool) {
        self.0.swdio_to_output();

        let mut last = self.0.delay.get_current();

        for _ in 0..32 {
            self.write_bit((data & 1) as u8, &mut last);
            data >>= 1;
        }

        self.write_bit(parity as u8, &mut last);
    }

    fn read_data(&mut self) -> (u32, bool) {
        self.0.swdio_to_input();

        let mut data = 0;

        let mut last = self.0.delay.get_current();

        for i in 0..32 {
            data |= (self.read_bit(&mut last) as u32 & 1) << i;
        }

        let parity = self.read_bit(&mut last) != 0;

        (data, parity)
    }

    #[inline(always)]
    fn write_bit(&mut self, bit: u8, last: &mut u32) {
        if bit != 0 {
            self.0.swdio.set_high();
        } else {
            self.0.swdio.set_low();
        }

        let half_period_ticks = self.0.half_period_ticks;

        self.0.swclk.set_low();
        *last = self.0.delay.delay_ticks_from_last(half_period_ticks, *last);
        self.0.swclk.set_high();
        *last = self.0.delay.delay_ticks_from_last(half_period_ticks, *last);
    }

    #[inline(always)]
    fn read_bit(&mut self, last: &mut u32) -> u8 {
        let half_period_ticks = self.0.half_period_ticks;

        self.0.swclk.set_low();
        *last = self.0.delay.delay_ticks_from_last(half_period_ticks, *last);
        let bit = self.0.swdio.is_high() as u8;
        self.0.swclk.set_high();
        *last = self.0.delay.delay_ticks_from_last(half_period_ticks, *last);

        bit
    }
}

#[derive(Debug)]
pub struct Swo {}

impl swo::Swo for Swo {
    fn set_transport(&mut self, _transport: swo::SwoTransport) {}

    fn set_mode(&mut self, _mode: swo::SwoMode) {}

    fn set_baudrate(&mut self, _baudrate: u32) -> u32 {
        0
    }

    fn set_control(&mut self, _control: swo::SwoControl) {}

    fn polling_data(&mut self, _buf: &mut [u8]) -> u32 {
        0
    }

    fn streaming_data(&mut self) {}

    fn is_active(&self) -> bool {
        false
    }

    fn bytes_available(&self) -> u32 {
        0
    }

    fn buffer_size(&self) -> u32 {
        0
    }

    fn support(&self) -> swo::SwoSupport {
        swo::SwoSupport {
            uart: false,
            manchester: false,
        }
    }

    fn status(&mut self) -> swo::SwoStatus {
        swo::SwoStatus {
            active: false,
            trace_error: false,
            trace_overrun: false,
            bytes_available: 0,
        }
    }
}
