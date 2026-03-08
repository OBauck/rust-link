use ch32_hal as hal;
use ch32_hal::mode::Async;
use embassy_futures::select::{select, Either};
use embassy_usb::class::cdc_acm::{ControlChanged, Receiver, Sender};
use hal::peripherals;
use hal::usart::{Config as UartConfig, UartRx, UartTx};
use hal::usbd::Driver;

use super::my_println;

// Println fucks up because it is blocking
// When no debugger is connected, the fw just hangs
// TODO: Switch to ringbuffer + defmt (see defmt_rtt crate for inspiration or use this: https://crates.io/crates/defmt-bbq)
use my_println as println;

#[embassy_executor::task]
pub async fn usb_tx_uart_rx_task(
    mut usb_tx: Sender<'static, Driver<'static, peripherals::USBD>>,
    mut uart_rx: UartRx<'static, peripherals::USART2, Async>,
    usb_control: ControlChanged<'static>,
    mut uart_config: UartConfig,
) -> ! {
    let mut buf = [0; 64];

    loop {
        usb_tx.wait_connection().await;
        println!("Usb tx Connected");
        loop {
            match select(
                usb_control.control_changed(),
                uart_rx.read_until_idle(&mut buf),
            )
            .await
            {
                Either::First(_) => {
                    let baud = usb_tx.line_coding().data_rate();
                    println!("Setting baud to: {}", baud);
                    uart_config.baudrate = baud;
                    if let Err(err) = uart_rx.set_config(&uart_config) {
                        println!("Uart config error: {:?}", err);
                    }
                }
                Either::Second(Err(err)) => println!("uart rx error! {:?}", err),
                Either::Second(Ok(n)) => {
                    if let Err(err) = usb_tx.write_packet(&buf[..n]).await {
                        println!("Usb tx error! {:?}", err)
                    }
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn usb_rx_uart_tx_task(
    mut usb_rx: Receiver<'static, Driver<'static, peripherals::USBD>>,
    mut uart_tx: UartTx<'static, peripherals::USART2, Async>,
) -> ! {
    let mut buf = [0; 64];
    loop {
        usb_rx.wait_connection().await;
        println!("Usb rx Connected");
        loop {
            match usb_rx.read_packet(&mut buf).await {
                Ok(n) => {
                    if let Err(err) = uart_tx.write(&buf[..n]).await {
                        println!("uart tx error! {:?}", err);
                    }
                }
                Err(err) => println!("Usb rx error: {:?}"),
            }
        }
    }
}
