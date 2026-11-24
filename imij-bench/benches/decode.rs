//! End-to-end decode throughput: imij v1 (1..16 threads) vs QOI, per class.
//! IMIJ_CORPUS = manifest path, IMIJ_CLASSES = comma list.

#[path = "../src/corpus.rs"]
mod corpus;

use std::path::Path;
use std::sync::Arc;

use imij::{Channels, ImiEncoderConfig as V1Config, Metadata, SampleType};
use zenbench::prelude::*;

struct Item {
    imij: Vec<u8>,
    qoi: Option<Vec<u8>>,
    bytes: usize,
    stride: usize,
}

fn bench(suite: &mut Suite) {
    let manifest = std::env::var("IMIJ_CORPUS")
        .unwrap_or_else(|_| format!("{}/tmp/zqi/corpus_lab.tsv", std::env::var("HOME").unwrap()));
    let classes = std::env::var("IMIJ_CLASSES").unwrap_or_else(|_| "photo,screen,clipart".into());
    let threads: Vec<usize> = std::env::var("IMIJ_THREADS")
        .unwrap_or_else(|_| "1,4,8,16".into())
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    for class in classes.split(',') {
        let mut items = Vec::new();
        for (c, p) in corpus::load_manifest(&manifest, Some(class)) {
            let Ok(img) = corpus::load_png(&c, Path::new(&p)) else {
                continue;
            };
            let mut imgs = corpus::variants(&img);
            imgs.insert(0, img);
            for im in imgs {
                let imij = V1Config::default()
                    .encode_samples(
                        imij::ImageRef::new(&im.pixels, im.info.row_bytes(), &im.info),
                        &Metadata::default(),
                    )
                    .unwrap();
                let qoi = match (im.info.sample, im.info.channels) {
                    (SampleType::U8, Channels::Rgb) => Some(zenbitmaps::PixelLayout::Rgb8),
                    (SampleType::U8, Channels::Rgba) => Some(zenbitmaps::PixelLayout::Rgba8),
                    _ => None,
                }
                .map(|l| {
                    zenbitmaps::encode_qoi(
                        &im.pixels,
                        im.info.width,
                        im.info.height,
                        l,
                        enough::Unstoppable,
                    )
                    .unwrap()
                });
                items.push(Item {
                    imij,
                    qoi,
                    bytes: im.pixels.len(),
                    stride: im.info.row_bytes(),
                });
            }
        }
        if items.is_empty() {
            continue;
        }
        let total: usize = items.iter().map(|i| i.bytes).sum();
        let items = Arc::new(items);
        suite.group(format!("decode_{class}"), |g| {
            g.throughput(Throughput::Bytes(total as u64));
            if items.iter().all(|i| i.qoi.is_some()) {
                let it = items.clone();
                g.bench("qoi", move |b| {
                    b.iter(|| {
                        let mut s = 0u8;
                        for i in it.iter() {
                            let d = zenbitmaps::decode_qoi(
                                i.qoi.as_ref().unwrap(),
                                enough::Unstoppable,
                            )
                            .unwrap();
                            s ^= d.pixels()[0];
                        }
                        s
                    })
                });
            }
            for &t in &threads {
                let it = items.clone();
                g.bench(format!("imij_{t}t"), move |b| {
                    let mut bufs: Vec<Vec<u8>> = it.iter().map(|i| vec![0u8; i.bytes]).collect();
                    b.iter(|| {
                        for (i, buf) in it.iter().zip(bufs.iter_mut()) {
                            imij::decode_into_threads(&i.imij, buf, i.stride, t).unwrap();
                        }
                        bufs[0][0]
                    })
                });
            }
        });
    }
}

zenbench::main!(bench);
