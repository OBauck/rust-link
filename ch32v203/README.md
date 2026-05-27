# ch32v203

### Build

```bash
cargo build --bin usb_dap --release --features ch32v203g6u6
```

### Flash

#### wchisp

[wchisp](https://github.com/ch32-rs/wchisp) needs to be installed:

```bash
cargo install wchisp --git https://github.com/ch32-rs/wchisp
```

Reset/power-cycle device while pushing boot0 button to enter usb bootloader and then:

```bash
cargo run --bin usb_dap --release --features ch32v203g6u6
```

#### wlink
[wlink](https://github.com/ch32-rs/wlink) needs to be installed:

```bash
cargo install --git https://github.com/ch32-rs/wlink
```

Change runner in .cargo/config.toml to:
```
runner = "wlink -v flash --enable-sdi-print --watch-serial --erase"
```
(Remove `--enable-sdi-print --watch-serial` if sdi-print logging is not needed)

With a WCH-Link probe connected to your target and then:

```bash
cargo run --bin usb_dap --release --features ch32v203g6u6
```

### USB DAP example

#### Test results

**test command**
``` shell
probe-rs run --chip nRF52840_xxAA --speed 8000 --log-format "{t} {L} {s}" test/ble_bas_peripheral.elf
```

**opt-level = 's':**
```
      Erasing ✔ 100% [####################] 252.00 KiB @  43.84 KiB/s (took 6s)
  Programming ✔ 100% [####################] 252.00 KiB @  69.76 KiB/s (took 4s)                               
      Finished in 9.37s
```

**opt-level = 3:**
```
      Erasing ✔ 100% [####################] 252.00 KiB @  44.78 KiB/s (took 6s)
  Programming ✔ 100% [####################] 252.00 KiB @  83.92 KiB/s (took 3s)                               
      Finished in 8.64s
```

### USB serial echo Example

#### Test results
We need over 400 kB/s to be able to stream 100ksamples/s to power profiler application (32bit per data).

```
dd if=/dev/zero of=/dev/ttyACM1 count=10000
10000+0 records in
10000+0 records out
5120000 bytes (5.1 MB, 4.9 MiB) copied, 8.46966 s, 605 kB/s
```