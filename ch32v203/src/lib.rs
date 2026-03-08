#![no_std]
#![no_main]

pub mod usb_dap_task;
pub mod usb_ppk2_task;
pub mod usb_uart_task;

#[macro_export]
macro_rules! my_println {
    ($($arg:tt)*) => {
        ()
    };
}

pub fn bytes_to_hex<'a>(bytes: &[u8], out: &'a mut [u8]) -> Result<&'a str, ()> {
    // Need exactly 2 chars per byte
    if out.len() < bytes.len() * 2 {
        return Err(());
    }

    let mut i = 0;
    for &b in bytes {
        let hi = b >> 4;
        let lo = b & 0x0F;

        out[i] = hex_char(hi);
        out[i + 1] = hex_char(lo);
        i += 2;
    }

    // SAFETY: we only write valid ASCII hex characters
    Ok(core::str::from_utf8(&out[..i]).unwrap())
}

#[inline]
const fn hex_char(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        10..=15 => b'A' + (n - 10),
        _ => b'?',
    }
}
