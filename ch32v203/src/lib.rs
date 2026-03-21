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

#[cfg(feature = "bootloader")]
pub unsafe fn enable_backup_registers() {
    ch32_hal::pac::RCC.apb1pcenr().modify(|w| {
        w.set_pwren(true);
        w.set_bkpen(true);
    });
    const R32_PWR_CTLR: usize = 0x40007000;
    unsafe {
        // BDP: Backup domain write enable
        *(R32_PWR_CTLR as *mut u32) |= 1 << 8;
    }
}

#[cfg(feature = "bootloader")]
pub unsafe fn enter_bootloader() {
    const R16_BKP_DATAR10: usize = 0x40006C28;
    unsafe {
        // Set R16_BKP_DATAR10 to 0x624c ('bL') to enter bootloader on next boot
        *(R16_BKP_DATAR10 as *mut u16) = 0x624c;
    }

    // Reset chip to enter bootloader
    ch32_hal::pac::PFIC.sctlr().write(|w| w.set_sysreset(true));
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
