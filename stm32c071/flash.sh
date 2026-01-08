#!/bin/bash
# Convert ELF to BIN
arm-none-eabi-objcopy -O binary "$1" "$1.bin"
# Flash it
dfu-util -a 0 -s 0x08000000:leave -D "$1.bin"