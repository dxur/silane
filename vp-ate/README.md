# vp-ate

A VP8 key-frame decoder written in silane (`hw/*.si`), with the software reference
model it is checked against (`src/model`).

Supported: key frames up to 1920 pixels wide, one token partition, no segmentation.
The loop filter is not applied, so the output equals libvpx/ffmpeg's output with
`-skip_loop_filter all` (and their normal output when the frame's filter level is 0).

## Interface (`VpAte`, `hw/vp_ate.si`)

| Port | Dir | Meaning |
|---|---|---|
| `p0_valid/p0_data/p0_ready` | in | the frame from its first byte through partition 0 |
| `p1_valid/p1_data/p1_last/p1_ready` | in | partition 1, from byte `part1_offset`; `p1_last` on its last byte |
| `px_valid/px_data/px_ready` | out | reconstructed pixels, 4 per word (leftmost in bits 7..0) |
| `frame_done` | out | pulse: the frame's last row was produced |
| `info_valid`, `width`, `height`, `part1_offset` | out | frame header fields |
| `error` | out | unsupported or invalid frame; decoding stops until reset |

All streams are valid/ready. Frames can follow each other back to back on both input
streams. Each macroblock is output in raster order as its 4x4 blocks (16 luma blocks in
raster order, then 4 U, then 4 V), each as four rows of four pixels: 96 words.

## Architecture

```text
 p0 ─► Headers ──MbHeader──► Tokens ──coefficients──► Recon ─► px
        │ bool decoder 0      │ bool decoder 1         │ IDCT/IWHT, prediction
        │ frame header,       │ 1 bool/cycle,          │ 4 pixels/cycle
        │ probability updates │ dequantization         │ line buffer
        └──── probabilities ──┘
```

- `bool_decoder.si`: the arithmetic decoder, one bool per cycle from a 32-bit window
  of the partition; both outcomes are normalized in parallel.
- `headers.si`: the frame header, the 1056 coefficient probability updates and the
  macroblock modes, at two cycles per bool (the probability is registered). Sub-block
  mode contexts of the row above live in a small RAM.
- `tokens.si`: the residual tokens at one bool per cycle. Everything that depends on a
  bool (`TokStep`) is computed for both of its values from registers, so the decoded
  bit only selects, and the next bool's probability is a register. The probability RAM
  reads ahead (two positions while a DCT_0 can follow), and a second, four-entry RAM
  holds the first position of each plane for the next block. Coefficients go to a
  two-bank buffer, so token decoding of one macroblock overlaps the reconstruction of
  the previous one.
- `recon.si`: a front stage reads one coefficient column per cycle and applies the
  vertical transform into a pair of transpose buffers; the back stages take a row,
  apply the horizontal transform, predict the row, and add the two and output the row,
  one row per cycle. Above edges come from one line buffer holding the bottom row of the
  macroblock row above; left edges are registers. About 4 cycles per 4x4 block (5 for
  B_PRED sub-blocks, which wait for their left neighbour).
- `src/rtl.rs`: the constant tables (default and update probabilities, quantizers,
  sub-block mode probabilities) as block-RAM ROMs, built from the model's tables.

There is no frame buffer: key frames need no reference frame, and the line buffer is
all intra prediction needs.

## Timing

On an ECP5-85 (speed grade 6,
Yosys and nextpnr) the decoder runs at about 57 MHz, with 11.5k LUT4, 3.7k FFs, 12
DP16KD and 11 multipliers. The limit is the bool decoder's loop (range times
probability, comparison, normalization), which is inherent to one bool per cycle.

## Commands

```text
cargo test --release                      # all testbenches (the decoder ones take minutes)
cargo run --release --bin export          # writes vp_ate.v
cargo run --release --bin decode -- in.ivf out.yuv   # the software model, as raw I420
```

Testbenches (`tests/`) compare each block with the model on the conformance samples in
`samples/`, with random stalls on every stream: the bool decoder bit by bit, the header
parser (every field, probability and macroblock header), the token decoder (every
dequantized coefficient) and the whole decoder (every pixel). The model itself matches
ffmpeg bit for bit on every supported sample.
