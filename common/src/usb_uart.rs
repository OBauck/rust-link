use embassy_futures::join::join;
use embassy_futures::select::{select3, Either3};
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use embedded_io_async::{BufRead, Write};

#[cfg(feature = "stm32")]
impl<'d> UartBaud for embassy_stm32::usart::BufferedUart<'d> {
    fn set_baud(&mut self, baudrate: u32) {
        let _ = self.set_baudrate(baudrate);
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
                Either3::Second(res) => match res {
                    Err(_err) => {
                        #[cfg(feature = "defmt")]
                        defmt::warn!("Disconnected {:?}", defmt::Debug2Format(&_err));
                        break;
                    }
                    Ok(n) => {
                        if let Err(_err) = usart.write(&usb_buf[0..n]).await {
                            #[cfg(feature = "defmt")]
                            defmt::warn!(
                                "Unable to write to usart: {:?}",
                                defmt::Debug2Format(&_err)
                            );
                        }
                    }
                },
                Either3::Third(res) => {
                    let usart_buf = match res {
                        Err(_err) => {
                            #[cfg(feature = "defmt")]
                            defmt::error!("usart buf error: {:?}", defmt::Debug2Format(&_err));
                            continue;
                        }
                        Ok(buf) => buf,
                    };
                    if let Err(_err) = usb_tx.write_packet(usart_buf).await {
                        #[cfg(feature = "defmt")]
                        defmt::error!("Disconnected {:?}", defmt::Debug2Format(&_err));
                        break;
                    }
                    let n = usart_buf.len();
                    usart.consume(n);
                }
            }
        }
    }
}
