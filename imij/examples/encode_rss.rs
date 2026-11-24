//! One lossy encode per process, for external peak-memory accounting
//! (`/usr/bin/time -v`, heaptrack): `encode_rss RAW:WxH EFFORT THREADS
//! QUALITY [off]`. RAW is tight RGB8; XYB, run-level, automatic stripes.
//! `off` (lab builds) turns the effort's trellis off. Prints the file size.
use imij::{Channels, ImageInfo, Metadata, SampleType};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, dim) = args[0].rsplit_once(':').expect("RAW:WxH");
    let (w, h) = dim.split_once('x').expect("WxH");
    let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
    let px = std::fs::read(path).unwrap();
    assert_eq!(px.len(), (w * h * 3) as usize);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    #[allow(unused_mut)]
    let mut cfg = imij::ImiEncoderConfig::lossy(args[3].parse::<f32>().unwrap())
        .with_effort(args[1].parse().unwrap())
        .with_threads(args[2].parse().unwrap());
    if args.get(4).map(String::as_str) == Some("off") {
        #[cfg(feature = "lab")]
        {
            cfg = cfg.tune_lossy(|t| {
                t.lab.trellis = Some(imij::LossyTrellis {
                    passes: 0,
                    ..imij::LossyTrellis::default()
                });
            });
        }
        #[cfg(not(feature = "lab"))]
        panic!("`off` needs the lab feature");
    }
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&px, w as usize * 3, &info),
            &Metadata::default(),
        )
        .unwrap();
    println!("{}", file.len());
}
