//! Sample-word pixel packets, shared with the standalone interoperability harness.
//! Original implementation; IMI unit framing is in the parent module.
#![forbid(unsafe_code)]
extern crate alloc;
#[cfg(feature = "encode")]
use alloc::vec::Vec;

type Result<T> = core::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub bytes: usize,
}
impl Shape {
    pub fn row(self) -> Result<usize> {
        if self.width == 0
            || self.height == 0
            || !(1..=4).contains(&self.channels)
            || ![1, 2, 4, 8].contains(&self.bytes)
            || self.width > u32::MAX as usize
            || self.height > u32::MAX as usize
        {
            return Err("shape");
        }
        self.width
            .checked_mul(self.channels)
            .and_then(|n| n.checked_mul(self.bytes))
            .ok_or("row size")
    }
    #[cfg(feature = "encode")]
    pub fn check(self, size: usize, stride: usize) -> Result<usize> {
        let row = self.row()?;
        let extent = (self.height - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row))
            .ok_or("extent")?;
        if stride < row || size < extent {
            return Err("buffer bounds");
        }
        Ok(row)
    }
}

// Monomorphizing by sample storage width keeps the RGB8 cache at 256 bytes.
pub(crate) trait Lane: Copy + Eq + Default {
    const BYTES: usize;
    const MASK: u64;
    fn bits(self) -> u64;
    fn from_bits(v: u64) -> Self;
    fn read(p: &[u8]) -> Self;
    fn write(self, p: &mut [u8]);
}
macro_rules! lane {
    ($t:ty, $n:expr) => {
        impl Lane for $t {
            const BYTES: usize = $n;
            const MASK: u64 = <$t>::MAX as u64;
            #[inline]
            fn bits(self) -> u64 {
                self as u64
            }
            #[inline]
            fn from_bits(v: u64) -> Self {
                v as Self
            }
            #[inline]
            fn read(p: &[u8]) -> Self {
                Self::from_le_bytes(p[..$n].try_into().unwrap())
            }
            #[inline]
            fn write(self, p: &mut [u8]) {
                p[..$n].copy_from_slice(&self.to_le_bytes());
            }
        }
    };
}
lane!(u8, 1);
lane!(u16, 2);
lane!(u32, 4);
lane!(u64, 8);

type Pixel<T> = [T; 4];
#[inline]
fn hash<T: Lane>(p: Pixel<T>) -> usize {
    // Byte folding agrees with QOI for 8-bit samples and includes the high
    // bytes of wide samples. The weights come from the QOI specification.
    let mut h = 0u64;
    for (v, weight) in p.into_iter().zip([3, 5, 7, 11]) {
        let mut v = v.bits();
        let mut fold = 0;
        for _ in 0..T::BYTES {
            fold += v & 255;
            v >>= 8;
        }
        h += fold * weight;
    }
    (h & 63) as usize
}
#[inline]
fn initial<T: Lane>() -> Pixel<T> {
    [
        T::default(),
        T::default(),
        T::default(),
        T::from_bits(T::MASK),
    ]
}
#[inline]
#[cfg(feature = "encode")]
fn load<T: Lane, const C: usize>(p: &[u8]) -> Pixel<T> {
    let first = T::read(p);
    if C <= 2 {
        [
            first,
            first,
            first,
            if C == 2 {
                T::read(&p[T::BYTES..])
            } else {
                T::from_bits(T::MASK)
            },
        ]
    } else {
        [
            first,
            T::read(&p[T::BYTES..]),
            T::read(&p[2 * T::BYTES..]),
            if C == 4 {
                T::read(&p[3 * T::BYTES..])
            } else {
                T::from_bits(T::MASK)
            },
        ]
    }
}
#[inline]
fn store<T: Lane, const C: usize>(p: Pixel<T>, out: &mut [u8]) {
    if C <= 2 {
        p[0].write(out);
        if C == 2 {
            p[3].write(&mut out[T::BYTES..]);
        }
    } else {
        p[0].write(out);
        p[1].write(&mut out[T::BYTES..]);
        p[2].write(&mut out[2 * T::BYTES..]);
        if C == 4 {
            p[3].write(&mut out[3 * T::BYTES..]);
        }
    }
}
#[inline]
#[cfg(feature = "encode")]
fn bias<T: Lane>(a: T, b: T, amount: u64) -> u64 {
    a.bits().wrapping_sub(b.bits()).wrapping_add(amount) & T::MASK
}
#[inline]
#[cfg(feature = "encode")]
fn literal<T: Lane, const C: usize>(p: Pixel<T>, alpha: bool, out: &mut Vec<u8>) {
    // One capacity check and length update per packet, including wide words.
    let mut packet = [0u8; 33];
    packet[0] = if alpha { 255 } else { 254 };
    let mut n = 1;
    for &v in &p[..if C <= 2 { 1 } else { 3 }] {
        v.write(&mut packet[n..n + T::BYTES]);
        n += T::BYTES;
    }
    if alpha {
        p[3].write(&mut packet[n..n + T::BYTES]);
        n += T::BYTES;
    }
    out.extend_from_slice(&packet[..n]);
}

#[cfg(feature = "encode")]
fn encode_rows<T: Lane, const C: usize>(
    data: &[u8],
    stride: usize,
    shape: Shape,
    out: &mut Vec<u8>,
    checkpoint: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut cache = [[T::default(); 4]; 64];
    let mut previous = initial::<T>();
    let mut run = 0u8;
    let row = shape.row()?;
    for y in 0..shape.height {
        checkpoint()?;
        for bytes in data[y * stride..y * stride + row].chunks_exact(C * T::BYTES) {
            let p = load::<T, C>(bytes);
            if p == previous {
                // Do not rely on indexing the initial run color: some deployed
                // QOI readers omit cache insertion for runs. Later run colors
                // already have a cache entry from a non-run packet.
                run += 1;
                if run == 62 {
                    out.push(0xc0 + run - 1);
                    run = 0;
                }
                continue;
            }
            if run != 0 {
                out.push(0xc0 + run - 1);
                run = 0;
            }
            let slot = hash(p);
            if cache[slot] == p {
                out.push(slot as u8);
            } else {
                cache[slot] = p;
                if p[3] != previous[3] {
                    literal::<T, C>(p, true, out);
                } else {
                    let small = [
                        bias(p[0], previous[0], 2),
                        bias(p[1], previous[1], 2),
                        bias(p[2], previous[2], 2),
                    ];
                    if small.iter().all(|&d| d < 4) {
                        out.push(
                            0x40 | ((small[0] as u8) << 4)
                                | ((small[1] as u8) << 2)
                                | small[2] as u8,
                        );
                    } else {
                        let dg = p[1].bits().wrapping_sub(previous[1].bits());
                        let dg32 = dg.wrapping_add(32) & T::MASK;
                        let r8 = p[0]
                            .bits()
                            .wrapping_sub(previous[0].bits())
                            .wrapping_sub(dg)
                            .wrapping_add(8)
                            & T::MASK;
                        let b8 = p[2]
                            .bits()
                            .wrapping_sub(previous[2].bits())
                            .wrapping_sub(dg)
                            .wrapping_add(8)
                            & T::MASK;
                        if dg32 < 64 && r8 < 16 && b8 < 16 {
                            out.extend_from_slice(&[
                                0x80 | dg32 as u8,
                                ((r8 as u8) << 4) | b8 as u8,
                            ]);
                        } else {
                            literal::<T, C>(p, false, out);
                        }
                    }
                }
            }
            previous = p;
        }
    }
    if run != 0 {
        out.push(0xc0 + run - 1);
    }
    checkpoint()
}

struct Cursor<'a> {
    remaining: &'a [u8],
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (packet, rest) = self
            .remaining
            .split_at_checked(n)
            .ok_or("truncated packet")?;
        self.remaining = rest;
        Ok(packet)
    }
    fn byte(&mut self) -> Result<u8> {
        let (&byte, rest) = self.remaining.split_first().ok_or("truncated packet")?;
        self.remaining = rest;
        Ok(byte)
    }
}
#[cfg(feature = "encode")]
macro_rules! channels {
    ($fun:ident, $t:ty, $c:expr, $($args:expr),*) => {
        match $c {
            1 => $fun::<$t,1>($($args),*), 2 => $fun::<$t,2>($($args),*),
            3 => $fun::<$t,3>($($args),*), 4 => $fun::<$t,4>($($args),*), _ => Err("channels")
        }
    };
}
#[cfg(feature = "encode")]
macro_rules! dispatch {
    ($fun:ident, $s:expr, $($args:expr),*) => {
        match $s.bytes {
            1 => channels!($fun,u8,$s.channels,$($args),*),
            2 => channels!($fun,u16,$s.channels,$($args),*),
            4 => channels!($fun,u32,$s.channels,$($args),*),
            8 => channels!($fun,u64,$s.channels,$($args),*), _ => Err("sample width")
        }
    };
}

// Retained correctness-only interleaving experiment; it is not the shipping kernel.
#[cfg(all(test, feature = "std", feature = "encode"))]
#[inline]
fn packet<T: Lane, const C: usize>(
    input: &mut Cursor<'_>,
    pixel: &mut Pixel<T>,
    run: &mut usize,
    cache: &mut [Pixel<T>; 64],
) -> Result<()> {
    let op = input.byte()?;
    match op {
        254 | 255 => {
            let colors = if C <= 2 { 1 } else { 3 };
            let words = colors + usize::from(op == 255);
            let literal = input.take(words * T::BYTES)?;
            pixel[0] = T::read(literal);
            if C <= 2 {
                pixel[1] = pixel[0];
                pixel[2] = pixel[0];
            } else {
                pixel[1] = T::read(&literal[T::BYTES..]);
                pixel[2] = T::read(&literal[2 * T::BYTES..]);
            }
            if op == 255 {
                pixel[3] = T::read(&literal[colors * T::BYTES..]);
            }
        }
        0..=63 => *pixel = cache[op as usize],
        64..=127 => {
            for (c, shift) in [4, 2, 0].into_iter().enumerate() {
                pixel[c] = T::from_bits(
                    pixel[c]
                        .bits()
                        .wrapping_add(((op >> shift) & 3) as u64)
                        .wrapping_sub(2),
                );
            }
        }
        128..=191 => {
            let extra = input.byte()?;
            let green = (op & 63) as u64;
            for (c, delta) in [
                green + (extra >> 4) as u64,
                green + 8,
                green + (extra & 15) as u64,
            ]
            .into_iter()
            .enumerate()
            {
                pixel[c] = T::from_bits(pixel[c].bits().wrapping_add(delta).wrapping_sub(40));
            }
        }
        _ => *run = (op & 63) as usize + 1,
    }
    if C <= 2 && (pixel[0] != pixel[1] || pixel[0] != pixel[2]) {
        return Err("gray packet");
    }
    cache[hash(*pixel)] = *pixel;
    Ok(())
}

/// Alternate packets from two independent RGB8 units. Separate caches and
/// cursors expose independent dependency chains without changing unit syntax.
#[cfg(all(test, feature = "std", feature = "encode"))]
pub(crate) fn pair_rgb8(a: &[u8], b: &[u8], ao: &mut [u8], bo: &mut [u8]) -> Result<()> {
    if !ao.len().is_multiple_of(3) || !bo.len().is_multiple_of(3) {
        return Err("output row");
    }
    #[inline]
    fn step(
        input: &mut Cursor<'_>,
        pixel: &mut Pixel<u8>,
        run: &mut usize,
        cache: &mut [Pixel<u8>; 64],
        output: &mut [u8],
    ) -> Result<usize> {
        if *run == 0 {
            packet::<u8, 3>(input, pixel, run, cache)?;
        }
        let count = if *run == 0 {
            1
        } else {
            (*run).min(output.len() / 3)
        };
        let bytes = count * 3;
        for dst in output[..bytes].as_chunks_mut::<3>().0 {
            *dst = [pixel[0], pixel[1], pixel[2]];
        }
        *run = run.saturating_sub(count);
        Ok(bytes)
    }
    let (mut ai, mut bi) = (Cursor { remaining: a }, Cursor { remaining: b });
    let (mut ap, mut bp) = (initial::<u8>(), initial::<u8>());
    let (mut ac, mut bc) = ([[0u8; 4]; 64], [[0u8; 4]; 64]);
    let (mut ar, mut br, mut at, mut bt) = (0, 0, 0, 0);
    while at < ao.len() || bt < bo.len() {
        if at < ao.len() {
            at += step(&mut ai, &mut ap, &mut ar, &mut ac, &mut ao[at..])?;
        }
        if bt < bo.len() {
            bt += step(&mut bi, &mut bp, &mut br, &mut bc, &mut bo[bt..])?;
        }
    }
    if ar != 0 || br != 0 || !ai.remaining.is_empty() || !bi.remaining.is_empty() {
        return Err("packet extent");
    }
    Ok(())
}

struct RowState<T: Lane> {
    cache: [Pixel<T>; 64],
    pixel: Pixel<T>,
    run: usize,
    pos: usize,
    /// `hash(pixel)` maintained incrementally; the per-packet fold runs only
    /// when a packet computes a fresh pixel value.
    slot: usize,
}
impl<T: Lane> RowState<T> {
    fn new() -> Self {
        Self {
            cache: [[T::default(); 4]; 64],
            pixel: initial::<T>(),
            run: 0,
            pos: 0,
            slot: hash(initial::<T>()),
        }
    }
    /// Decode one packet: fetch the opcode byte, update `pixel`, `run`,
    /// `slot` and the cache. Shared by the byte-cursor and u8 pixel-array
    /// row loops so both keep identical packet semantics.
    #[inline(always)]
    fn packet<const C: usize>(
        &mut self,
        input: &mut Cursor<'_>,
        pixel: &mut Pixel<T>,
        run: &mut usize,
        slot: &mut usize,
    ) -> Result<()> {
        let op = input.byte()?;
        match op {
            254 | 255 => {
                let colors = if C <= 2 { 1 } else { 3 };
                let words = colors + usize::from(op == 255);
                let literal = input.take(words * T::BYTES)?;
                pixel[0] = T::read(literal);
                if C <= 2 {
                    pixel[1] = pixel[0];
                    pixel[2] = pixel[0];
                } else {
                    pixel[1] = T::read(&literal[T::BYTES..]);
                    pixel[2] = T::read(&literal[2 * T::BYTES..]);
                }
                if op == 255 {
                    pixel[3] = T::read(&literal[colors * T::BYTES..]);
                }
                *slot = hash(*pixel);
                self.cache[*slot] = *pixel;
            }
            // A written cache entry sits at its own hash slot, so
            // reinserting it is a no-op. An untouched slot yields the
            // zero tuple, whose hash is 0: it must be stored into slot
            // 0, evicting whatever a nonzero literal left there.
            0..=63 => {
                *pixel = self.cache[op as usize];
                if *pixel == [T::default(); 4] {
                    *slot = 0;
                    self.cache[0] = *pixel;
                } else {
                    *slot = op as usize;
                }
            }
            64..=127 => {
                for (c, shift) in [4, 2, 0].into_iter().enumerate() {
                    pixel[c] = T::from_bits(
                        pixel[c]
                            .bits()
                            .wrapping_add(((op >> shift) & 3) as u64)
                            .wrapping_sub(2),
                    );
                }
                if C <= 2 && (pixel[0] != pixel[1] || pixel[0] != pixel[2]) {
                    return Err("gray packet");
                }
                *slot = hash(*pixel);
                self.cache[*slot] = *pixel;
            }
            128..=191 => {
                let extra = input.byte()?;
                let green = (op & 63) as u64;
                for (c, delta) in [
                    green + (extra >> 4) as u64,
                    green + 8,
                    green + (extra & 15) as u64,
                ]
                .into_iter()
                .enumerate()
                {
                    pixel[c] = T::from_bits(pixel[c].bits().wrapping_add(delta).wrapping_sub(40));
                }
                if C <= 2 && (pixel[0] != pixel[1] || pixel[0] != pixel[2]) {
                    return Err("gray packet");
                }
                *slot = hash(*pixel);
                self.cache[*slot] = *pixel;
            }
            // A run keeps the previous pixel; its cache slot was
            // written when the pixel was produced. Reinserting by the
            // tracked slot covers a first-packet run (initial pixel).
            _ => {
                *run = (op & 63) as usize + 1;
                self.cache[*slot] = *pixel;
            }
        }
        Ok(())
    }
    fn row<const C: usize>(&mut self, payload: &[u8], output: &mut [u8]) -> Result<()> {
        let bpp = C * T::BYTES;
        let mut input = Cursor {
            remaining: payload.get(self.pos..).ok_or("truncated packet")?,
        };
        let mut pixel = self.pixel;
        let mut run = self.run;
        let mut slot = self.slot;
        let mut done = false;
        if T::BYTES == 1 && C >= 3 {
            let (pixels, rem) = output.as_chunks_mut::<C>();
            if rem.is_empty() {
                // u8 lanes decode into fixed-width pixel slots: one slice
                // destructure and one store per pixel.
                let mut at = 0;
                while at < pixels.len() {
                    if run == 0 {
                        self.packet::<C>(&mut input, &mut pixel, &mut run, &mut slot)?;
                        if run == 0 {
                            pixels[at] = core::array::from_fn(|i| pixel[i].bits() as u8);
                            at += 1;
                            continue;
                        }
                    }
                    // A run includes its first pixel and may cross row boundaries.
                    let n = run.min(pixels.len() - at);
                    pixels[at..at + n].fill(core::array::from_fn(|i| pixel[i].bits() as u8));
                    run -= n;
                    at += n;
                }
                done = true;
            }
        }
        if !done {
            let mut at = 0;
            while at < output.len() {
                if run == 0 {
                    self.packet::<C>(&mut input, &mut pixel, &mut run, &mut slot)?;
                    // Ordinary pixels avoid run-span division, min and copy setup.
                    if run == 0 {
                        if T::BYTES == 1 && C >= 3 && (C == 4 || at + 4 <= output.len()) {
                            // One 32-bit store instead of per-channel byte stores.
                            // For RGB the fourth byte is scratch: the next pixel
                            // or run fill overwrites it. The row-final pixel falls
                            // through to the exact-width store.
                            let w = u32::from_le_bytes([
                                pixel[0].bits() as u8,
                                pixel[1].bits() as u8,
                                pixel[2].bits() as u8,
                                pixel[3].bits() as u8,
                            ]);
                            output[at..at + 4].copy_from_slice(&w.to_le_bytes());
                        } else {
                            store::<T, C>(pixel, &mut output[at..at + bpp]);
                        }
                        at += bpp;
                        continue;
                    }
                    // A run includes its first pixel and may cross row boundaries.
                }
                let n = run.min((output.len() - at) / bpp);
                let dest = &mut output[at..at + n * bpp];
                if C == 1 && T::BYTES == 1 {
                    dest.fill(pixel[0].bits() as u8);
                } else {
                    // Fixed pixel stores let LLVM vectorize a repeated color;
                    // doubling needed several tiny overlapping copies per run.
                    for output in dest.chunks_exact_mut(bpp) {
                        store::<T, C>(pixel, output);
                    }
                }
                run -= n;
                at += n * bpp;
            }
        }
        self.pixel = pixel;
        self.run = run;
        self.slot = slot;
        self.pos = payload.len() - input.remaining.len();
        Ok(())
    }
    fn end(&self, payload: &[u8]) -> Result<()> {
        if self.run != 0 || self.pos != payload.len() {
            Err("packet extent")
        } else {
            Ok(())
        }
    }
}

// The fixed cache stays allocation-free for unit kernels and bounded row decoders.
// Owning stream routes box their enclosing State once, rather than each cache width.
#[allow(clippy::large_enum_variant)]
enum Samples {
    U8(RowState<u8>),
    U16(RowState<u16>),
    #[cfg(any(feature = "extended", feature = "encode"))]
    U32(RowState<u32>),
    #[cfg(any(feature = "extended", feature = "encode"))]
    U64(RowState<u64>),
}
/// Offset-only state; compressed ownership can move between row calls.
/// Output is raw little-endian storage, without color or numeric conversion.
pub struct Decoder {
    samples: Samples,
    shape: Shape,
    rows: usize,
    failed: Option<&'static str>,
}
impl Decoder {
    pub fn new(shape: Shape) -> Result<Self> {
        shape.row()?;
        Ok(Self {
            samples: match shape.bytes {
                1 => Samples::U8(RowState::new()),
                2 => Samples::U16(RowState::new()),
                #[cfg(any(feature = "extended", feature = "encode"))]
                4 => Samples::U32(RowState::new()),
                #[cfg(any(feature = "extended", feature = "encode"))]
                8 => Samples::U64(RowState::new()),
                _ => return Err("sample width"),
            },
            shape,
            rows: 0,
            failed: None,
        })
    }
    pub fn done(&self) -> bool {
        self.rows == self.shape.height
    }
    pub fn next(&mut self, payload: &[u8], output: &mut [u8]) -> Result<()> {
        let done = self.done();
        match &mut self.samples {
            Samples::U8(s) => checked_row(
                self.shape,
                done,
                &mut self.rows,
                &mut self.failed,
                s,
                payload,
                output,
            ),
            Samples::U16(s) => checked_row(
                self.shape,
                done,
                &mut self.rows,
                &mut self.failed,
                s,
                payload,
                output,
            ),
            #[cfg(any(feature = "extended", feature = "encode"))]
            Samples::U32(s) => checked_row(
                self.shape,
                done,
                &mut self.rows,
                &mut self.failed,
                s,
                payload,
                output,
            ),
            #[cfg(any(feature = "extended", feature = "encode"))]
            Samples::U64(s) => checked_row(
                self.shape,
                done,
                &mut self.rows,
                &mut self.failed,
                s,
                payload,
                output,
            ),
        }
    }
}

/// Monomorphic storage avoids reserving a wide-word cache for every byte plane.
pub(crate) struct TypedDecoder<T: Lane> {
    shape: Shape,
    state: RowState<T>,
    rows: usize,
    failed: Option<&'static str>,
}
pub(crate) type ByteDecoder = TypedDecoder<u8>;
impl<T: Lane> TypedDecoder<T> {
    pub(crate) fn new(shape: Shape) -> Result<Self> {
        shape.row()?;
        if shape.bytes != T::BYTES {
            return Err("packet storage width");
        }
        Ok(Self {
            shape,
            state: RowState::new(),
            rows: 0,
            failed: None,
        })
    }
    pub(crate) fn next(&mut self, payload: &[u8], out: &mut [u8]) -> Result<()> {
        checked_row(
            self.shape,
            self.rows == self.shape.height,
            &mut self.rows,
            &mut self.failed,
            &mut self.state,
            payload,
            out,
        )
    }
}
#[inline]
fn checked_row<T: Lane>(
    shape: Shape,
    done: bool,
    rows: &mut usize,
    failed: &mut Option<&'static str>,
    state: &mut RowState<T>,
    payload: &[u8],
    output: &mut [u8],
) -> Result<()> {
    if let Some(e) = *failed {
        return Err(e);
    }
    let row = shape.row()?;
    if done || output.len() < row {
        return Err("output row");
    }
    let result = match shape.channels {
        1 => state.row::<1>(payload, &mut output[..row]),
        2 => state.row::<2>(payload, &mut output[..row]),
        3 => state.row::<3>(payload, &mut output[..row]),
        4 => state.row::<4>(payload, &mut output[..row]),
        _ => Err("channels"),
    }
    .and_then(|()| {
        if *rows + 1 == shape.height {
            state.end(payload)
        } else {
            Ok(())
        }
    });
    match result {
        Ok(()) => {
            *rows += 1;
            Ok(())
        }
        Err(e) => {
            *failed = Some(e);
            Err(e)
        }
    }
}

/// Append packets after a caller-owned envelope. Validate before writing.
#[cfg(feature = "encode")]
pub fn encode_into(
    data: &[u8],
    stride: usize,
    shape: Shape,
    out: &mut Vec<u8>,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<()> {
    shape.check(data.len(), stride)?;
    dispatch!(
        encode_rows,
        shape,
        data,
        stride,
        shape,
        out,
        &mut checkpoint
    )
}
