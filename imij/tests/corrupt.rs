//! Decoding corrupted files must return an error or garbage pixels, never
//! panic, hang or allocate unboundedly.

use imij::{Channels, ImageInfo, Metadata, SampleType, decode};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn sample(w: u32, h: u32, ch: Channels, st: SampleType, seed: u64) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, ch, st);
    let mut r = Rng(seed | 1);
    let mut px = vec![0u8; info.row_bytes() * h as usize];
    for (i, b) in px.iter_mut().enumerate() {
        *b = if (i / 7) % 5 == 0 {
            (r.next() & 0xFF) as u8
        } else {
            ((i * 3) % 251) as u8
        };
    }
    (info, px)
}

#[test]
fn mutated_files_do_not_panic() {
    let mut files = Vec::new();
    for (w, h, ch, st) in [
        (40, 33, Channels::Rgb, SampleType::U8),
        (300, 20, Channels::Rgba, SampleType::U8),
        (77, 50, Channels::Rgb, SampleType::U16),
        (65, 9, Channels::GrayAlpha, SampleType::F16),
        (5, 5, Channels::Gray, SampleType::U8),
        (61, 17, Channels::Rgb, SampleType::F32),
        (40, 12, Channels::GrayAlpha, SampleType::I32),
    ] {
        if !cfg!(feature = "extended") && st.tier() == imij::Tier::Extended {
            continue;
        }
        let (info, px) = sample(w, h, ch, st, w as u64 * 31 + h as u64);
        files.push(
            imij::ImiEncoderConfig::default()
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap(),
        );
    }
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut errs = 0usize;
    let iters = std::env::var("IMIJ_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    for it in 0..iters {
        let src = &files[it % files.len()];
        let mut f = src.clone();
        match r.next() % 4 {
            0 => {
                // Flip 1-8 random bits.
                for _ in 0..1 + r.next() % 8 {
                    let i = (r.next() as usize) % f.len();
                    f[i] ^= 1 << (r.next() % 8);
                }
            }
            1 => {
                // Overwrite a random byte run.
                let i = (r.next() as usize) % f.len();
                let n = (1 + r.next() % 16) as usize;
                for j in i..(i + n).min(f.len()) {
                    f[j] = r.next() as u8;
                }
            }
            2 => {
                f.truncate((r.next() as usize) % f.len());
            }
            _ => {
                // Mutate within the header/model area specifically.
                let i = (r.next() as usize) % f.len().min(120);
                f[i] = r.next() as u8;
            }
        }
        if decode(&f).is_err() {
            errs += 1;
        }
    }
    eprintln!("{errs}/{iters} mutated files rejected");
}
