# stm32c071

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
      Erasing ✔ 100% [####################] 252.00 KiB @  40.85 KiB/s (took 6s)
  Programming ✔ 100% [####################] 252.00 KiB @  36.12 KiB/s (took 7s)                               
      Finished in 13.15s
```