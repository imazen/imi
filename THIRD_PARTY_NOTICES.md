# Third-party notices

IMIJ is original Rust code, but several algorithms and constants in it follow
published work from other projects. This file records those debts and the
licenses that apply. It is not a list of dependencies (those are in
`Cargo.toml`/`Cargo.lock` and their licenses are reported separately); it
covers code and constants that this repository derives from other projects'
sources.

If you believe something here is missing or misattributed, please open an
issue so it can be corrected.

## Independent JPEG Group (libjpeg)

The scaled float inverse DCT in `imij/src/lossy/dct.rs` is the Arai, Agui and
Nakajima scheme as used in libjpeg's `jidctflt.c`, including its scale-factor
constants. libjpeg is by the Independent JPEG Group (Thomas G. Lane, Guido
Vollbeding, and others). The IJG license asks for an acknowledgement in
product literature:

> This software is based in part on the work of the Independent JPEG Group.

The IJG license text ships with libjpeg (<https://www.ijg.org/>).

## libjxl (JPEG XL Project Authors) — BSD-3-Clause

The XYB opsin constants in `imij/src/lossy/color.rs` (`OPSIN_BIAS`,
`CBRT_BIAS`, `INV_OPSIN`) follow the opsin transform in the JPEG XL reference
implementation, and the XYB color space itself is JPEG XL's. The SSIMULACRA2
weights attributed in `imij-cmp/src/ssim2map.rs` originate in libjxl's
SSIMULACRA2 tool (see also the SSIMULACRA2 entry in `docs/REFERENCES.md`).

Copyright (c) the JPEG XL Project Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## jpegli — BSD-3-Clause

The adaptive-quantization field studied in `imij/src/lossy/aq_field_lab.rs`
and `docs/LOSSY.md` (E6d) is jpegli's AQ; the quantization-matrix shapes
behind the XYB base matrix follow jpegli's; and matrix 5 (`imij/src/lossy/
mod.rs`, `JPEGLI_YCBCR`, `JPEGLI_YCBCR_FACTOR`, `JPEGLI_FREQUENCY_EXPONENT`,
`jpegli_distance_to_scale`; SPEC 2.8.1) uses jpegli's YCbCr base tables
(`kBaseQuantMatrixYCbCr`), its per-frequency exponents (`kFrequencyExponent`)
and its nonlinear distance law, taken from jpegli and from our port of them
in zenjpeg (`src/foundation/consts.rs`, `src/quant/mod.rs`). jpegli is part
of the JPEG XL project; its code is Copyright (c) the JPEG XL Project
Authors and carries the same BSD-3-Clause license text as above
(<https://github.com/google/jpegli>).
Jpegli developed within libjxl before moving to its separate repository.
These credits name the particular source of each technique or constant;
the shared project history does not make every jpegli and JPEG XL path
interchangeable for attribution.

## Kornel Lesiński's overshoot deringing and mozjpeg — BSD-3-Clause / IJG

Kornel Lesiński described the overshoot and spline-extrapolation technique
in [*Deringing in DCT via overshoot and clipping*](https://kornel.ski/deringing/)
and contributed an implementation to mozjpeg. The overshoot deringing in
`imij/src/lossy/dering.rs` follows that mozjpeg code (`jcdctmgr.c`), as ported
by this project's zenjpeg. The trellis quantization in
`imij/src/lossy/trellis.rs` also follows mozjpeg.
mozjpeg is by Mozilla and contributors, built on libjpeg-turbo and libjpeg;
its license is the BSD-3-Clause (Mozilla) plus the IJG terms above
(<https://github.com/mozilla/mozjpeg>).

## LZ4 block format

`imij/src/metadata_pack.rs` implements the LZ4 block format grammar from the
published specification (<https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md>).
The implementation is original safe Rust; no LZ4 code is copied. Credit to
Yann Collet for the format.

## zenjpeg component reconstruction

The whole-plane f32 triangle in `imij/src/lossy/jpeg.rs` follows the
operation order of zenjpeg's component reconstruction
(<https://github.com/imazen/zenjpeg>). The shared AAN inverse DCT and
level shift retain the IJG acknowledgement above. The Rust triangle
implementation is used under this repository's MIT OR Apache-2.0 license.

## No copied code elsewhere

The rANS layer (`imij/src/rans.rs`), the lossless prediction and mapping
layers, the container, the metadata packer, the Huffman side information and
the transfer-function tables are independent implementations written from the
standards and papers cited in `docs/REFERENCES.md`. Where a comment names a
project, it names the source of the idea being followed, not copied source.

## QOI format

The original experimental implementation in `tools/codec-speed/quick-pixels`
follows Dominic Szablewski's
[QOI v1.0 specification](https://qoiformat.org/qoi-specification.pdf).
No QOI implementation source is copied into that directory. The authoring
agent had previously read zenbitmaps' rapid-qoi source; no clean-room claim is
made. The sample-word extension is independently specified in the tool's
README and is not part of QOI. See that README for provenance and scope.
