#!/bin/bash
# Convert ELF to binary
riscv64-unknown-elf-objcopy -O binary $1 target/output.bin
# Convert binary to UF2 using the WCH Family ID
uf2conv -f 0x699b62ec -b 0x08001000 -o target/flash.uf2 target/output.bin
# Copy to the board (assuming it's mounted)
cp target/flash.uf2 /media/$USER/CH32V\ UF2/