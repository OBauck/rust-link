use embassy_futures::join::join;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use embedded_io_async::{BufRead, Write};

#[cfg(feature = "stm32")]
mod stm32 {
    impl<'d> super::UartBaud for embassy_stm32::usart::BufferedUart<'d> {
        fn set_baud(&mut self, baudrate: u32) {
            let _ = self.set_baudrate(baudrate);
        }
    }
    impl<'d> super::UartBaud for embassy_stm32::usart::BufferedUartTx<'d> {
        fn set_baud(&mut self, baudrate: u32) {
            let _ = self.set_baudrate(baudrate);
        }
    }
}

#[cfg(feature = "rp")]
mod rp {
    impl super::UartBaud for embassy_rp::uart::BufferedUart {
        fn set_baud(&mut self, baudrate: u32) {
            let _ = self.set_baudrate(baudrate);
        }
    }
    impl super::UartBaud for embassy_rp::uart::BufferedUartTx {
        fn set_baud(&mut self, baudrate: u32) {
            // Uncomment when this PR is merged: https://github.com/embassy-rs/embassy/pull/5159
            // let _ = self.set_baudrate(baudrate);
        }
    }
}

pub trait UartBaud {
    fn set_baud(&mut self, baudrate: u32);
}

pub async fn run<'d, D: Driver<'d>>(
    mut usart: impl BufRead + Write + UartBaud,
    cdc_acm: CdcAcmClass<'d, D>,
) -> ! {
    let mut usb_buf = [0; 64];

    let (mut usb_tx, mut usb_rx, usb_control) = cdc_acm.split_with_control();

    loop {
        join(usb_rx.wait_connection(), usb_tx.wait_connection()).await;
        #[cfg(feature = "defmt")]
        defmt::debug!("Uart Connected");
        loop {
            match select3(
                usb_control.control_changed(),
                usb_rx.read_packet(&mut usb_buf),
                usart.fill_buf(),
            )
            .await
            {
                Either3::First(_) => {
                    let baud = usb_rx.line_coding().data_rate();
                    #[cfg(feature = "defmt")]
                    defmt::debug!("Setting baud to: {}", baud);
                    usart.set_baud(baud);
                }
                Either3::Second(Err(_err)) => {
                    #[cfg(feature = "defmt")]
                    defmt::warn!(
                        "Usb read error: {:?}. Assume disconnection",
                        defmt::Debug2Format(&_err)
                    );
                    break;
                }
                Either3::Second(Ok(n)) => {
                    if let Err(_err) = usart.write_all(&usb_buf[..n]).await {
                        #[cfg(feature = "defmt")]
                        defmt::error!("Unable to write to usart: {:?}", defmt::Debug2Format(&_err),);
                    }
                }
                Either3::Third(Err(_err)) => {
                    #[cfg(feature = "defmt")]
                    defmt::error!("Uart read error: {:?}", defmt::Debug2Format(&_err));
                }
                Either3::Third(Ok(usart_buf)) => match usb_tx.write(usart_buf).await {
                    Err(_err) => {
                        #[cfg(feature = "defmt")]
                        defmt::error!(
                            "Usb write error: {:?}. Assume disconnection",
                            defmt::Debug2Format(&_err)
                        );
                        break;
                    }
                    Ok(n) => usart.consume(n),
                },
            }
        }
    }
}

// Run uart in split mode to avoid TX blocking RX and vice versa
pub async fn run_split_uart<'d, D: Driver<'d>>(
    mut usart_tx: impl Write + UartBaud,
    mut usart_rx: impl BufRead,
    cdc_acm: CdcAcmClass<'d, D>,
) -> ! {
    let mut usb_buf = [0; 64];

    let (mut usb_tx, mut usb_rx, usb_control) = cdc_acm.split_with_control();

    let usb_tx_uart_rx = async {
        loop {
            usb_tx.wait_connection().await;
            #[cfg(feature = "defmt")]
            defmt::debug!("USB Uart RX Connected");
            loop {
                match usart_rx.fill_buf().await {
                    Ok(usart_buf) => match usb_tx.write(usart_buf).await {
                        Err(_err) => {
                            #[cfg(feature = "defmt")]
                            defmt::error!("Disconnected {:?}", defmt::Debug2Format(&_err));
                            break;
                        }
                        Ok(n) => usart_rx.consume(n),
                    },
                    Err(_err) => {
                        #[cfg(feature = "defmt")]
                        defmt::error!("usart buf error: {:?}", defmt::Debug2Format(&_err));
                    }
                }
            }
        }
    };

    let usb_rx_uart_tx = async {
        loop {
            usb_rx.wait_connection().await;
            #[cfg(feature = "defmt")]
            defmt::debug!("USB Uart TX Connected");
            loop {
                match select(
                    usb_control.control_changed(),
                    usb_rx.read_packet(&mut usb_buf),
                )
                .await
                {
                    Either::First(_) => {
                        let baud = usb_rx.line_coding().data_rate();
                        #[cfg(feature = "defmt")]
                        defmt::debug!("Setting baud to: {}", baud);
                        usart_tx.set_baud(baud);
                    }
                    Either::Second(Err(_err)) => {
                        #[cfg(feature = "defmt")]
                        defmt::error!("Disconnected {:?}", defmt::Debug2Format(&_err));
                        break;
                    }
                    Either::Second(Ok(n)) => {
                        if let Err(_err) = usart_tx.write_all(&usb_buf[..n]).await {
                            #[cfg(feature = "defmt")]
                            defmt::error!(
                                "Unable to write to usart: {:?}",
                                defmt::Debug2Format(&_err),
                            );
                        }
                    }
                }
            }
        }
    };

    // Use join to allow rx and tx to work independently
    join(usb_tx_uart_rx, usb_rx_uart_tx).await;
    unreachable!("The futures should not complete");
}
