//! `imij::ImiEncoderConfig::lossy_bounded`: the common job over the bounded lossy
//! encoder, with codec-owned scratch.
use imij::codec::Scratch;
use imij::{DecodeSpeed, Error, ImiEncoderConfig};
use std::path::PathBuf;
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{ResourceLimits, StopToken};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 300;
const H: u32 = 170;

/// Smooth gradients, edges and texture, rows of `stride` bytes.
fn source(desc: PixelDescriptor, stride: usize) -> Vec<u8> {
    let bpp = desc.bytes_per_pixel();
    let mut bytes = vec![0x5a; stride * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            for c in 0..bpp {
                let v = (x * (c + 1) + y * 2 + ((x / 13 + y / 7) % 3) * 40) as u8;
                bytes[y * stride + x * bpp + c] = v;
            }
        }
    }
    bytes
}

fn lossy() -> ImiEncoderConfig {
    imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast)
}

fn whole_pixels(desc: PixelDescriptor, bytes: &[u8], stride: usize) -> Vec<u8> {
    let pixels = PixelSlice::new(bytes, W, H, stride, desc).unwrap();
    let out = lossy().job().encoder().unwrap().encode(pixels).unwrap();
    imij::decode(out.data()).unwrap().1
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entries(dir: &PathBuf) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

#[test]
fn encode_push_and_pull_decode_to_the_whole_image_pixels() {
    for desc in [PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGBA8_SRGB] {
        let stride = (W as usize + 2) * desc.bytes_per_pixel();
        let bytes = source(desc, stride);
        let expected = whole_pixels(desc, &bytes, stride);
        let dir = scratch_dir(&format!("codec_bounded_{}", desc.bytes_per_pixel()));
        for scratch in [Scratch::Memory, Scratch::Dir(dir.clone())] {
            let config = lossy();
            // In place.
            let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
            let file = config
                .clone()
                .job()
                .with_scratch(scratch.clone())
                .encoder()
                .unwrap()
                .encode(pixels)
                .unwrap();
            assert_eq!(
                imij::decode(file.data()).unwrap().1,
                expected,
                "{scratch:?}"
            );
            // Pushed in uneven batches, spooled.
            let mut enc = config
                .clone()
                .job()
                .with_scratch(scratch.clone())
                .with_canvas_size(W, H)
                .encoder()
                .unwrap();
            let mut y = 0;
            for n in [1u32, 37, 64, 68] {
                let rows = &bytes[y as usize * stride..];
                let batch = PixelSlice::new(rows, W, n, stride, desc).unwrap();
                enc.push_rows(batch).unwrap();
                y += n;
            }
            assert_eq!(y, H);
            let pushed = enc.finish().unwrap();
            assert_eq!(pushed.data(), file.data(), "push vs in place, {scratch:?}");
            // Pulled.
            let pulled = config
                .clone()
                .job()
                .with_scratch(scratch.clone())
                .with_canvas_size(W, H)
                .with_input_descriptor(desc)
                .encoder()
                .unwrap()
                .encode_from(&mut |row, mut out| {
                    let r = row as usize * stride;
                    let n = W as usize * desc.bytes_per_pixel();
                    out.row_mut(0)[..n].copy_from_slice(&bytes[r..r + n]);
                    1
                })
                .unwrap();
            assert_eq!(pulled.data(), file.data(), "pull vs in place, {scratch:?}");
        }
        // Every temporary file is gone.
        assert_eq!(entries(&dir), 0);
    }
}

#[test]
fn checksummed_output_uses_codec_scratch() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let dir = scratch_dir("codec_bounded_checksum");
    let config = lossy().with_checksum(true);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    let file = config
        .job()
        .with_scratch(Scratch::Dir(dir.clone()))
        .encoder()
        .unwrap()
        .encode(pixels)
        .unwrap();
    assert_eq!(
        imij::decode(file.data()).unwrap().1,
        whole_pixels(desc, &bytes, stride)
    );
    assert_eq!(entries(&dir), 0);
}

#[test]
fn memory_limit_admits_the_working_bound() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    let encode = |cap: u64, scratch: Scratch| {
        lossy()
            .job()
            .with_scratch(scratch)
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .encoder()
            .unwrap()
            .encode(pixels.clone())
    };
    assert!(matches!(
        encode(1 << 20, Scratch::Memory),
        Err(Error::TooLarge)
    ));
    assert!(encode(64 << 20, Scratch::Memory).is_ok());
    // Pushed rows spooled to the heap count against the limit too.
    let raster = (stride * H as usize) as u64;
    let bound = 64 << 20;
    let mut enc = lossy()
        .job()
        .with_scratch(Scratch::Memory)
        .with_canvas_size(W, H)
        .with_limits(ResourceLimits::none().with_max_memory(bound))
        .encoder()
        .unwrap();
    enc.push_rows(pixels.clone()).unwrap();
    assert!(enc.finish().is_ok());
    assert!(raster < bound);
}

#[test]
fn whole_image_configurations_and_lossless_fidelity_are_refused() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    let mut small = lossy();
    small = small.tune_lossy(|t| {
        t.small_transforms = true;
    });
    let r = small
        .clone()
        .job()
        .with_scratch(Scratch::Memory)
        .encoder()
        .unwrap()
        .encode(pixels.clone());
    assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
    // Refused at the first pushed batch, before spooling the raster.
    let mut enc = small
        .job()
        .with_scratch(Scratch::Memory)
        .with_canvas_size(W, H)
        .encoder()
        .unwrap();
    let first = PixelSlice::new(&bytes, W, 1, stride, desc).unwrap();
    assert!(matches!(enc.push_rows(first), Err(Error::Unsupported(_))));
    let lossless = lossy().with_lossless(true);
    assert!(matches!(
        lossless.job().with_scratch(Scratch::Memory).encoder().err(),
        Some(Error::Unsupported(_))
    ));
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn a_stop_during_the_encode_cancels() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    // Allow the job's own checks before the encode starts, then stop at the
    // encoder's first band reads.
    for allowed in [2usize, 6] {
        let left = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(allowed));
        let r = lossy()
            .job()
            .with_scratch(Scratch::Memory)
            .with_stop(StopToken::new(After(left)))
            .encoder()
            .and_then(|e| e.encode(pixels.clone()));
        assert!(matches!(r, Err(Error::Cancelled)), "{:?}", r.err());
    }
}

#[derive(Clone)]
struct After(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl zencodec::enough::Stop for After {
    fn check(&self) -> Result<(), zencodec::enough::StopReason> {
        use std::sync::atomic::Ordering;
        if self.0.load(Ordering::SeqCst) == 0 {
            return Err(zencodec::enough::StopReason::Cancelled);
        }
        self.0.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn output_limit_applies_to_heap_and_directory_scratch() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    let dir = scratch_dir("codec_bounded_output_limit");
    for scratch in [Scratch::Memory, Scratch::Dir(dir.clone())] {
        let config = lossy();
        let expected = config
            .clone()
            .job()
            .with_scratch(scratch.clone())
            .encoder()
            .unwrap()
            .encode(pixels.clone())
            .unwrap();
        let limit = expected.data().len() as u64;
        assert!(
            config
                .clone()
                .job()
                .with_scratch(scratch.clone())
                .with_limits(ResourceLimits::none().with_max_output(limit))
                .encoder()
                .unwrap()
                .encode(pixels.clone())
                .is_ok()
        );
        assert!(matches!(
            config
                .job()
                .with_scratch(scratch.clone())
                .with_limits(ResourceLimits::none().with_max_output(limit - 1))
                .encoder()
                .unwrap()
                .encode(pixels.clone()),
            Err(Error::TooLarge)
        ));
    }
    assert_eq!(entries(&dir), 0);
}

#[test]
fn failed_spool_is_not_reusable() {
    let desc = PixelDescriptor::RGB8_SRGB;
    let stride = W as usize * 3;
    let bytes = source(desc, stride);
    let pixels = PixelSlice::new(&bytes, W, H, stride, desc).unwrap();
    let mut encoder = lossy()
        .job()
        .with_scratch(Scratch::Memory)
        .with_canvas_size(W, H)
        .encoder()
        .unwrap();
    encoder.push_rows(pixels.clone()).unwrap();
    assert!(encoder.push_rows(pixels.clone()).is_err());
    assert!(encoder.push_rows(pixels).is_err());
    assert!(encoder.finish().is_err());
}
