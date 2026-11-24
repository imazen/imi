// Helpers shared by the decode-side fuzz targets (`header`, `decode`,
// `region`, `frames`, `checksum`, `differential`).
//
// Included by each target's `*_core.rs` (inside its own module), so the
// items are per-module; `allow(dead_code)` covers the targets that use only
// some of them.

use imij::Limits;
use imij::decode_resources::DecodePolicy;

/// Maximum decoded pixels per image in a fuzz run.
pub const MAX_PIXELS: u64 = 2_000_000;
/// Maximum decoded output bytes per image in a fuzz run.
pub const MAX_BYTES: u64 = 32 << 20;
/// Maximum planned codec working bytes per session call.
pub const MAX_WORKING: u64 = 64 << 20;
/// Maximum compressed input a session accepts.
pub const MAX_INPUT: u64 = 4 << 20;

/// Output limits used by every decode-side target: small enough that
/// libFuzzer explores decode logic instead of canvas size, large enough for
/// all frozen parity fixtures, with checksum verification on.
#[allow(dead_code)]
pub fn decode_limits() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.max_pixels = MAX_PIXELS;
    limits.max_bytes = MAX_BYTES;
    limits.verify_checksum = true;
    limits
}

/// [`decode_limits`] as a cumulative session policy (input, metadata, chunk,
/// unit, work and working-memory allowances).
#[allow(dead_code)]
pub fn decode_policy() -> DecodePolicy {
    DecodePolicy::default()
        .with_output_limits(decode_limits())
        .with_max_input_bytes(MAX_INPUT)
        .with_max_metadata_bytes(1 << 20)
        .with_max_chunks(4096)
        .with_max_units(200_000)
        .with_max_work_units(1 << 30)
        .with_max_working_bytes(MAX_WORKING)
}

/// Whether this library build decodes the Extended tier (32- and 64-bit
/// samples). The stable replay runs under builds without the `extended`
/// feature too, so generators must not ask for samples the build refuses.
#[allow(dead_code)]
pub fn extended_tier() -> bool {
    imij::supported_profiles()
        .iter()
        .any(|p| p.profile == imij::Profile::Extended)
}

/// Picks `sample`, or `fallback` when the build has no Extended tier
/// (`fallback` must only name samples that work on every build: baseline and,
/// for lossy coding, lossy-capable).
#[allow(dead_code)]
pub fn sample_for_build(
    sample: imij::SampleType,
    seed: usize,
    fallback: &[imij::SampleType],
) -> imij::SampleType {
    if sample.tier() == imij::Tier::Extended && !extended_tier() && !fallback.is_empty() {
        fallback[seed % fallback.len()]
    } else {
        sample
    }
}

/// A session policy sized from the compressed input: input, metadata and
/// planned codec working capacity may not grow far beyond it. Success under
/// this budget must still agree bit-for-bit with the reference; refusal is
/// legal, because the allowance is a conservative capacity plan, not an
/// allocator cap (the fuzzer's rss limit is the hard detector).
#[allow(dead_code)]
pub fn amplification_policy(data_len: usize) -> DecodePolicy {
    let input = (data_len as u64).saturating_mul(4).saturating_add(1 << 16);
    decode_policy()
        .with_max_input_bytes(input)
        .with_max_metadata_bytes(input)
        .with_max_working_bytes((data_len as u64).saturating_mul(8).saturating_add(1 << 20))
}

/// [`amplification_policy`] for the region reader, which takes [`Limits`]
/// only: output plus record bytes must stay near the input and rect sizes.
#[allow(dead_code)]
pub fn amplification_limits(data_len: usize, out_len: usize) -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.max_pixels = MAX_PIXELS;
    limits.max_bytes = (data_len as u64)
        .saturating_mul(8)
        .saturating_add(out_len as u64)
        .saturating_add(1 << 20);
    limits.verify_checksum = true;
    limits
}

/// FNV-1a over bytes. Used only to compare decoded outputs across processes
/// (the tier check in `imij/tests/fuzz_regression.rs`); not a security hash.
#[allow(dead_code)]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Copies the `rb` logical bytes of every `stride`-wide row into a tight
/// buffer, asserting that the padding stayed at `pad` (a route that writes
/// beyond the row is a bug).
#[allow(dead_code)]
pub fn rows_tight(out: &[u8], stride: usize, rb: usize, height: usize, pad: u8) -> Vec<u8> {
    assert!(stride >= rb);
    let mut tight = Vec::with_capacity(rb * height);
    for row in out.chunks_exact(stride).take(height) {
        assert!(
            row[rb..].iter().all(|&b| b == pad),
            "decode wrote into row padding"
        );
        tight.extend_from_slice(&row[..rb]);
    }
    tight
}

/// Fallible `Vec` allocation helpers are the callers'; this asserts a
/// length fits the fuzz limits before the caller allocates.
#[allow(dead_code)]
pub fn len_within_limits(len: usize) -> bool {
    len as u64 <= MAX_BYTES
}

/// A hand-rolled structure decoder for the `roundtrip_*` generators: reads
/// fields left to right and hands out the rest as pixel bytes. (`arbitrary`
/// would work, but the stable replay in `imij/tests/fuzz_regression.rs`
/// includes the same cores and `imij` must not grow a fuzz-only
/// dev-dependency; this keeps both readers byte-for-byte identical.)
#[allow(dead_code)]
pub struct Bytes<'a> {
    pub data: &'a [u8],
    pub at: usize,
}

#[allow(dead_code)]
impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    pub fn u8(&mut self) -> Option<u8> {
        let v = self.data.get(self.at).copied();
        self.at += 1;
        v
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some((self.u8()? as u16) | ((self.u8()? as u16) << 8))
    }
    /// The not-yet-consumed bytes; consumes them.
    pub fn rest(&mut self) -> &'a [u8] {
        let r = &self.data[self.at.min(self.data.len())..];
        self.at = self.data.len();
        r
    }
}
