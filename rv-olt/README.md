# rv-olt

An RV64GC core and SoC written in silane (`hw/*.si`), able to boot off-the-shelf
OpenSBI and Linux (Debian 13's riscv64 builds) in silane's simulator.

- ISA: RV64IMAFDC (RV64GC) with Zicsr, Zifencei, Zicntr; M, S and U modes; Sv39.
- The core is a five-stage in-order pipeline (fetch, decode, execute, memory,
  writeback) with forwarding, a two-word prefetching fetch stage, and precise traps
  taken at the memory stage. Branches are predicted not taken (JAL redirects from
  decode). MUL/DIV and the FPU are multi-cycle and stall execute. Booting Linux
  averages about 2.2 cycles per instruction from a memory that answers at once.
  That's the base to grow branch prediction, caches and out-of-order execution on.
- The SoC is laid out like QEMU's `virt` machine, so stock firmware and kernels need
  nothing special: a boot ROM, a CLINT, a PLIC, an ns16550a UART, a `sifive,test0`
  poweroff/reboot device, and RAM at 0x8000_0000.

There is no software reference model: the hardware is tested by self-checking
programs that run on it (`tests/asm`).

## Layout

| Path | What |
|---|---|
| `hw/core.si` | the core: the pipeline, forwarding and hazards, memory accesses, traps |
| `hw/decode.si`, `hw/rvc.si` | decoder and RVC expander |
| `hw/alu.si`, `hw/muldiv.si` | integer ALU; iterative multiplier and divider |
| `hw/fpu.si` | F and D: one exact fused multiply-add datapath, iterative div/sqrt, a shared rounding stage |
| `hw/csr.si` | CSRs, privilege modes, traps, delegation, interrupts |
| `hw/mmu.si` | Sv39: 8-entry TLB and page-table walker (Svade: A/D bits are set by software) |
| `hw/devices.si`, `hw/soc.si` | CLINT, PLIC, UART; the SoC and its address map |
| `dts/rv-olt.dts` | the device tree (compiled with `dtc` by the runner) |
| `src/machine.rs` | the simulated machine: RAM, console and timer tick around the SoC's RTL |
| `src/bin/rv-olt.rs` | the runner: loads firmware, kernel, device tree, initramfs |
| `src/bin/mkinitramfs.rs` | builds a busybox initramfs |
| `tests/isa.rs`, `tests/asm` | ISA tests; `tests/asm/gen` generates the ALU, M and F/D vectors |

## Memory map

| Address | Device |
|---|---|
| `0x0000_1000` | boot ROM: `a0` = hart id, `a1` = device tree, jump to the entry point |
| `0x0010_0000` | test device: `0x5555` poweroff/pass, `0x7777` reboot, `code << 16 \| 0x3333` fail |
| `0x0200_0000` | CLINT (msip, mtimecmp, mtime) |
| `0x0c00_0000` | PLIC: 31 sources, contexts 0 (M) and 1 (S); the UART is source 10 |
| `0x1000_0000` | UART (ns16550a) |
| `0x8000_0000` | RAM (128 MiB), outside the SoC on the `ram_*` port |

## Booting Linux

The images are Debian's (trixie, riscv64), unpacked into `images/` (not in git):

```sh
mkdir -p images/deb && cd images/deb
# opensbi, linux-image-6.12.*-riscv64, busybox-static from deb.debian.org
for d in *.deb; do dpkg-deb -x $d ../root; done
cd ../..
cargo run --release --bin mkinitramfs -- images/root/usr/bin/busybox images/initramfs.cpio
cargo run --release --bin rv-olt -- \
    --fw images/root/usr/lib/riscv64-linux-gnu/opensbi/generic/fw_jump.bin \
    --kernel images/root/boot/vmlinux-6.12.107+deb13-riscv64 \
    --initrd images/initramfs.cpio
```

The UART is on the terminal; Ctrl-A then `x` quits. `--send-after <pattern> <text>`
types into the console once `<pattern>` has been printed, for scripted runs.

## Verilog

```sh
cargo run --release --bin export            # soc.v (or: export core|fpu [out.v])
```

The SoC's ports are listed in `src/bin/export.rs`. RAM is outside it, on the
`ram_*` request/acknowledge port. The register files are `std:mem` memories, which
synthesis maps to block RAM.

## Tests

```sh
cargo test --release --test isa
```

They need `clang` (with the RISC-V target) and `mold`. They cover RV64I, M, A, C, F/D
(8,779 cases with exact expected results and flags), the privileged architecture,
Sv39, interrupts and pipeline hazards.

For debugging, `RV_TRACE=1` prints every retired instruction, and
`RV_PROBE=w_valid,w_rd,w_val` adds the named signals (registers, by the end of
their hierarchical names) every cycle.
