# stm32g0b1

### Build

```bash
cargo build --release
```

### Flash

#### swd

Connect a swd programmer/debugger and then:

```bash
cargo run --release
```

### USB DAP example

#### Test results

**test command**
``` shell
probe-rs run --chip nRF52840_xxAA --speed 8000 --log-format "{t} {L} {s}" test/test_image_nrf52840.elf
```

```
      Erasing ✔ 100% [####################] 252.00 KiB @  TODO
  Programming ✔ 100% [####################] 252.00 KiB @  TODO                               
      Finished in TODO
```

### USB serial echo Example

#### Test results
For comparison: TinyUSB cdc_msc example managed 314 kB/s.
We need over 400 kB/s to be able to stream 100ksamples/s to power profiler application (32bit per data).

```
dd if=/dev/zero of=/dev/ttyACM0 count=10000
10000+0 records in
10000+0 records out
5120000 bytes (5.1 MB, 4.9 MiB) copied, 27.8211 s, 184 kB/s
```