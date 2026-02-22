# Rust Link

Use any MCU as an SWD/UART/PPK2 debug probe.

- USB to Arm Serial Wire Debug (SWD) port
- Compatible with the CMSIS-DAP standard
- USB to UART bridge
- USB to power profiler using the same protocol as PPK2. Use with [Power Profiler App](https://docs.nordicsemi.com/bundle/nrf-connect-ppk/page/index.html)
- Future(?): USB logic analyzer

The firmware relies heavily on use of [embassy](https://github.com/embassy-rs/embassy) framework.

Example code is provided for some different mcus (CH32, STM32, Rp), but should be easily extended to other mcus which support embassy.

