# stm32g431

### Build

```bash
cargo build --bin usb_dap --release
```

### Flash

#### swd

Connect a swd programmer/debugger and then:

```bash
cargo run --bin usb_dap --release
```

### USB DAP example

#### Test results

**test command**
``` shell
probe-rs run --chip nRF52840_xxAA --speed 8000 --log-format "{t} {L} {s}" test/ble_bas_peripheral.elf
```

```
      Erasing ✔ 100% [####################] 252.00 KiB @  44.41 KiB/s (took 6s)
  Programming ✔ 100% [####################] 252.00 KiB @  81.27 KiB/s (took 3s)                               
      Finished in 8.78s
```

### USB serial echo Example

#### Test results
We need over 400 kB/s to be able to stream 100ksamples/s to power profiler application (32bit per data).

```
dd if=/dev/zero of=/dev/ttyACM2 count=10000
10000+0 records in
10000+0 records out
5120000 bytes (5.1 MB, 4.9 MiB) copied, 8.49616 s, 603 kB/s
```