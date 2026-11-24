//! Original RGB8 IQA samples and typed item metadata, without quality conversion.
#![forbid(unsafe_code)]
use enough::Unstoppable;
use imij::item_metadata::{ItemMetadata, Value};
use imij::{FrameDecoder, ImageInfo, Metadata};
use sha2::{Digest, Sha256};
use std::{
    fs,
    hint::black_box,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

struct Image {
    info: ImageInfo,
    pixels: Vec<u8>,
    values: ItemMetadata,
    metadata: Metadata,
}

struct ArchiveObservation {
    lookup_ms: f64,
    copy_ms: f64,
    decode_ms: f64,
    rasters: Vec<(usize, Vec<u8>)>,
    copied_bytes: usize,
}

// The separately validated original CSV is projected to these seven TSV fields.
// No score rounding, resampling, PNG optimization or synthetic distortions.
fn load(table: &Path, images: &Path, log: &mut dyn Write) -> Result<Vec<Image>> {
    let mut result = Vec::new();
    for line in fs::read_to_string(table)?.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 7);
        let name = fields[0];
        assert!(
            name.bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_.".contains(&v))
        );
        let encoded = fs::read(images.join(name))?;
        let decoded = zenpng::decode(
            &encoded,
            &zenpng::PngDecodeConfig::strict().with_max_threads(1),
            &Unstoppable,
        )?;
        assert_eq!(decoded.info.bit_depth, 8);
        assert_eq!(decoded.info.color_type, 2);
        let info = ImageInfo::new(
            decoded.info.width,
            decoded.info.height,
            imij::Channels::Rgb,
            imij::SampleType::U8,
        );
        let pixels = decoded.pixels.copy_to_contiguous_bytes();
        assert_eq!(pixels.len(), info.checked_buffer_len().unwrap());
        let mut values = ItemMetadata::default();
        values.filename = Some(name.into());
        for (key, value) in [
            ("reference", fields[1]),
            ("dmos", fields[2]),
            ("variance", fields[3]),
        ] {
            if !value.is_empty() {
                values
                    .attributes
                    .insert(key.into(), Value::Text(value.into()));
            }
        }
        values
            .attributes
            .insert("is_reference".into(), Value::Bool(fields[6] == "1"));
        if fields[6] == "0" {
            values.attributes.insert(
                "distortion_type".into(),
                Value::Unsigned(fields[4].parse()?),
            );
            values.attributes.insert(
                "distortion_level".into(),
                Value::Unsigned(fields[5].parse()?),
            );
        }
        let mut metadata = Metadata::default();
        values.write_metadata(&mut metadata)?;
        writeln!(
            log,
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}",
            encoded.len(),
            hash(&encoded),
            pixels.len(),
            hash(&pixels),
            info.width,
            info.height
        )?;
        result.push(Image {
            info,
            pixels,
            values,
            metadata,
        });
    }
    assert_eq!(result.len(), 126);
    assert!(result.iter().all(|image| image.info == result[0].info));
    Ok(result)
}

fn decode_current(
    decoder: &mut FrameDecoder<'_>,
    previous: &[Vec<u8>],
    image: &Image,
) -> Result<Vec<u8>> {
    assert_eq!(decoder.frame_info().unwrap(), image.info);
    assert_eq!(
        ItemMetadata::from_metadata(decoder.frame_metadata().unwrap())?,
        image.values
    );
    let mut out = vec![0; image.pixels.len()];
    let stride = image.info.row_bytes();
    if let Some(index) = decoder.reference_index() {
        decoder.decode_into_with_reference(&previous[index], stride, &mut out, stride, 1)?;
    } else {
        decoder.decode_into(&mut out, stride, 1)?;
    }
    assert_eq!(out, image.pixels, "decoded original samples differ");
    Ok(out)
}

fn verify(file: &[u8], images: &[Image], policy: &str) -> Result<(usize, Vec<Option<usize>>)> {
    let mut decoder = FrameDecoder::new(file)?;
    assert_eq!(decoder.frame_count(), images.len());
    let mut decoded = Vec::new();
    let mut references = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let head = decoder.next_frame()?.unwrap();
        assert_eq!(head.index, index);
        let reference = decoder.reference_index();
        if let Some(reference) = reference {
            assert!(index > 0);
            assert_eq!(
                reference,
                match policy {
                    "reference" => 0,
                    "previous" => index - 1,
                    _ => panic!("unexpected reference"),
                }
            );
        }
        references.push(reference);
        decoded.push(decode_current(&mut decoder, &decoded, image)?);
    }
    assert!(decoder.next_frame()?.is_none());
    Ok((
        references.iter().filter(|v| v.is_some()).count(),
        references,
    ))
}

// Random ordinal access reconstructs only the declared dependency chain, from
// encoded bytes. Never supply original PNG pixels as a free decoder reference.
fn verify_access(file: &[u8], images: &[Image], ordinal: usize) -> Result<usize> {
    let mut decoder = FrameDecoder::new(file)?;
    let mut chain = vec![ordinal];
    let mut cursor = ordinal;
    while let Some(reference) = decoder.frame_seek_info(cursor)?.reference_index {
        assert!(reference < cursor);
        chain.push(reference);
        cursor = reference;
    }
    let mut cached = vec![Vec::new(); images.len()];
    for index in chain.iter().rev().copied() {
        decoder.seek_frame(index)?;
        cached[index] = decode_current(&mut decoder, &cached, &images[index])?;
    }
    Ok(chain.len())
}

fn config() -> imij::ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.with_effort(2.5);
    cfg = cfg.with_threads(1);
    cfg
}

fn encode_sequence(images: &[Image], kind: imij::SequenceKind, policy: &str) -> Result<Vec<u8>> {
    let mut encoder = imij::SequenceEncoder::new(kind, imij::FrameConfig::new(config()));
    for (index, image) in images.iter().enumerate() {
        let mut item = imij::SequenceItem::new(&image.pixels, image.info.row_bytes(), &image.info)
            .with_metadata(&image.metadata);
        if index > 0 && policy != "none" {
            let reference = if policy == "reference" { 0 } else { index - 1 };
            item = item.with_reference(imij::ImageReference {
                index: reference,
                pixels: &images[reference].pixels,
                stride: image.info.row_bytes(),
            });
        }
        // Includes the API's reconstruction and the returned-raster assertion.
        assert_eq!(encoder.push(item)?, image.pixels);
    }
    Ok(encoder.finish()?)
}

fn generate(table: &Path, source: &Path, output: &Path) -> Result<()> {
    fs::create_dir(output)?;
    let mut hashes = fs::File::create_new(output.join("source.tsv"))?;
    let images = load(table, source, &mut hashes)?;
    fs::copy(table, output.join("items.tsv"))?;
    let config = config();
    let mut results = fs::File::create_new(output.join("results.tsv"))?;
    writeln!(
        results,
        "arm\tbytes\tencode_seconds_informational\tselected_references\tpixels\tsha256"
    )?;
    fs::create_dir(output.join("independent"))?;
    let mut total = 0usize;
    let mut seconds = 0.0;
    for (index, image) in images.iter().enumerate() {
        let start = Instant::now();
        let file = config.encode_samples(
            imij::ImageRef::new(&image.pixels, image.info.row_bytes(), &image.info),
            &image.metadata,
        )?;
        seconds += start.elapsed().as_secs_f64();
        verify(&file, std::slice::from_ref(image), "none")?;
        total += file.len();
        fs::write(
            output.join("independent").join(format!("{index:03}.imi")),
            &file,
        )?;
    }
    let pixels: u64 = images
        .iter()
        .map(|i| u64::from(i.info.width) * u64::from(i.info.height))
        .sum();
    writeln!(results, "independent\t{total}\t{seconds}\t0\t{pixels}\t-")?;
    for (arm, kind, policy) in [
        ("pages", imij::SequenceKind::Pages, "none"),
        ("reference", imij::SequenceKind::Pages, "reference"),
        ("previous", imij::SequenceKind::Animation, "previous"),
    ] {
        let start = Instant::now();
        let file = encode_sequence(&images, kind, policy)?;
        let seconds = start.elapsed().as_secs_f64();
        let (selected, references) = verify(&file, &images, policy)?;
        let mut access = fs::File::create_new(output.join(format!("{arm}-access.tsv")))?;
        // Last level of every distortion type, not a hand-picked cheap access.
        for ordinal in (5..images.len()).step_by(5) {
            let decoded = verify_access(&file, &images, ordinal)?;
            writeln!(access, "{ordinal}\t{decoded}")?;
        }
        let mut refs = fs::File::create_new(output.join(format!("{arm}-references.tsv")))?;
        for (index, reference) in references.iter().enumerate() {
            writeln!(
                refs,
                "{index}\t{}",
                reference.map_or_else(|| "-".into(), |i| i.to_string())
            )?;
        }
        writeln!(
            results,
            "{arm}\t{}\t{seconds}\t{selected}\t{pixels}\t{}",
            file.len(),
            hash(&file)
        )?;
        fs::write(output.join(format!("{arm}.imi")), file)?;
        results.flush()?;
    }
    Ok(())
}

// Fixed first reference group timing. Inputs and encoded files are loaded before
// the clock: decoder CPU work, not filesystem or archive lookup throughput.
fn measure(group: &Path, pngs: &Path, arm: &str, action: &str, threads: usize) -> Result<()> {
    assert!(matches!(
        arm,
        "independent" | "pages" | "reference" | "previous"
    ));
    assert!(matches!(action, "encode" | "decode" | "access"));
    assert!([1, 2, 4, 8].contains(&threads));
    let images = load(&group.join("items.tsv"), pngs, &mut std::io::sink())?;
    let files: Vec<Vec<u8>> = if arm == "independent" {
        (0..images.len())
            .map(|i| fs::read(group.join("independent").join(format!("{i:03}.imi"))))
            .collect::<std::io::Result<_>>()?
    } else {
        vec![fs::read(group.join(format!("{arm}.imi")))?]
    };
    let policy = match arm {
        "reference" => "reference",
        "previous" => "previous",
        _ => "none",
    };
    let encode = || -> Result<Vec<Vec<u8>>> {
        if arm == "independent" {
            images
                .iter()
                .map(|image| {
                    Ok(config().encode_samples(
                        imij::ImageRef::new(&image.pixels, image.info.row_bytes(), &image.info),
                        &image.metadata,
                    )?)
                })
                .collect()
        } else {
            Ok(vec![encode_sequence(
                &images,
                if arm == "previous" {
                    imij::SequenceKind::Animation
                } else {
                    imij::SequenceKind::Pages
                },
                policy,
            )?])
        }
    };
    let decode = || -> Result<(Vec<Vec<u8>>, usize)> {
        let mut rasters = vec![Vec::new(); images.len()];
        let mut count = 0;
        if arm == "independent" {
            for (i, file) in files.iter().enumerate() {
                if action == "access" && i != images.len() - 1 {
                    continue;
                }
                let mut out = vec![0; images[i].pixels.len()];
                imij::decode_into_threads(file, &mut out, images[i].info.row_bytes(), threads)?;
                rasters[i] = out;
                count += 1;
            }
        } else {
            let mut decoder = FrameDecoder::new(&files[0])?;
            let mut ordinals: Vec<_> = (0..images.len()).collect();
            if action == "access" {
                ordinals.clear();
                let mut next = Some(images.len() - 1);
                while let Some(i) = next {
                    ordinals.push(i);
                    next = decoder.frame_seek_info(i)?.reference_index;
                }
                ordinals.reverse();
            }
            for i in ordinals {
                if action == "access" {
                    decoder.seek_frame(i)?;
                } else {
                    decoder.next_frame()?.unwrap();
                }
                let stride = images[i].info.row_bytes();
                let mut out = vec![0; images[i].pixels.len()];
                if let Some(r) = decoder.reference_index() {
                    decoder.decode_into_with_reference(
                        &rasters[r],
                        stride,
                        &mut out,
                        stride,
                        threads,
                    )?;
                } else {
                    decoder.decode_into(&mut out, stride, threads)?;
                }
                rasters[i] = out;
                count += 1;
            }
        }
        Ok((rasters, count))
    };
    let (elapsed, decoded_count) = if action == "encode" {
        assert_eq!(threads, 1);
        let warmup_start = Instant::now();
        assert_eq!(encode()?, files); // One untimed full-group warmup.
        let warmup = warmup_start.elapsed();
        let start = Instant::now();
        let encoded = encode()?;
        let elapsed = start.elapsed();
        assert_eq!(encoded, files); // Exact original generation bytes.
        println!(
            "encode_total_ms={}",
            (warmup + elapsed).as_secs_f64() * 1000.0
        );
        (elapsed, images.len())
    } else {
        for _ in 0..3 {
            std::hint::black_box(decode()?);
        }
        let start = Instant::now();
        let (decoded, count) = decode()?;
        let elapsed = start.elapsed();
        for (i, out) in decoded.iter().enumerate() {
            if !out.is_empty() {
                assert_eq!(out, &images[i].pixels);
            } else {
                assert_eq!(action, "access");
                assert_ne!(i, images.len() - 1);
            }
        }
        (elapsed, count)
    };
    let first = if action == "access" {
        images.len() - 1
    } else {
        0
    };
    let mut digest = Sha256::new();
    for image in &images[first..] {
        digest.update(&image.pixels);
    }
    let digest: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("elapsed_ms={}", elapsed.as_secs_f64() * 1000.0);
    println!("decoded_images={decoded_count}");
    println!(
        "check={{\"sha256\":\"{digest}\",\"images\":{},\"pixels_exact\":true,\"encoded_bytes_match\":true}}",
        images.len() - first
    );
    Ok(())
}

// Random access through a real archive. The archive is opened for each access;
// its indexed payload digest and the native sequence reference digest are both
// checked. Original PNGs are loaded before the clock and check decoded pixels
// after it. The file system cache state is reported by the caller.
fn archive_access(
    archive: &Path,
    group: &Path,
    pngs: &Path,
    arm: &str,
    threads: usize,
) -> Result<()> {
    assert!(matches!(arm, "independent" | "reference"));
    assert!([1, 2, 4, 8].contains(&threads));
    let images = load(&group.join("items.tsv"), pngs, &mut std::io::sink())?;
    let target = images.len() - 1;
    let key = if arm == "independent" {
        target as u64 * 2 + 1
    } else {
        1
    };

    let once = || -> Result<ArchiveObservation> {
        let start = Instant::now();
        let mut reader =
            imij_set::Reader::open(imij::planes::SeekReader::new(fs::File::open(archive)?))?;
        let entry = reader.get(key)?.ok_or("missing archive entry")?;
        let location = if arm == "reference" {
            let binding = imij_set::SequenceIndexRef::from_metadata(entry.metadata())?
                .ok_or("missing sequence index")?;
            let mut index = binding.open(&mut reader, &entry)?;
            Some(index.get(target as u64)?)
        } else {
            None
        };
        let lookup_ms = start.elapsed().as_secs_f64() * 1000.0;

        let copy = Instant::now();
        let file = reader.read_entry_bounded(&entry, MAX_ENTRY_BYTES)?;
        let copy_ms = copy.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(file.len() as u64, entry.len());

        let decode = Instant::now();
        let mut rasters = Vec::new();
        if arm == "independent" {
            let mut out = vec![0; images[target].pixels.len()];
            imij::decode_into_threads(&file, &mut out, images[target].info.row_bytes(), threads)?;
            rasters.push((target, out));
        } else {
            let mut decoder = FrameDecoder::new(&file)?;
            assert_eq!(decoder.frame_count(), images.len());
            let native = decoder.frame_seek_info(target)?;
            let indexed = location.as_ref().unwrap();
            assert_eq!(indexed.encoded, native.encoded);
            assert_eq!(
                indexed.reference_index,
                native.reference_index.map(|n| n as u64)
            );
            assert_eq!(indexed.reference_hash, native.reference_hash);
            let mut chain = vec![target];
            while let Some(reference) = decoder
                .frame_seek_info(*chain.last().unwrap())?
                .reference_index
            {
                assert!(reference < *chain.last().unwrap());
                chain.push(reference);
            }
            let mut previous = vec![Vec::new(); images.len()];
            for i in chain.iter().rev().copied() {
                decoder.seek_frame(i)?;
                let stride = images[i].info.row_bytes();
                let mut out = vec![0; images[i].pixels.len()];
                if let Some(reference) = decoder.reference_index() {
                    decoder.decode_into_with_reference(
                        &previous[reference],
                        stride,
                        &mut out,
                        stride,
                        threads,
                    )?;
                } else {
                    decoder.decode_into(&mut out, stride, threads)?;
                }
                previous[i] = out;
            }
            for i in chain.into_iter().rev() {
                rasters.push((i, core::mem::take(&mut previous[i])));
            }
        }
        let decode_ms = decode.elapsed().as_secs_f64() * 1000.0;
        Ok(ArchiveObservation {
            lookup_ms,
            copy_ms,
            decode_ms,
            rasters,
            copied_bytes: file.len(),
        })
    };

    for _ in 0..3 {
        for (i, pixels) in once()?.rasters {
            assert_eq!(pixels, images[i].pixels);
        }
    }
    let ArchiveObservation {
        lookup_ms,
        copy_ms,
        decode_ms,
        rasters,
        copied_bytes,
    } = once()?;
    for (i, pixels) in &rasters {
        assert_eq!(pixels, &images[*i].pixels);
    }
    println!("lookup_ms={lookup_ms}");
    println!("copy_ms={copy_ms}");
    println!("decode_ms={decode_ms}");
    println!("elapsed_ms={}", lookup_ms + copy_ms + decode_ms);
    println!("decoded_images={}", rasters.len());
    println!("copied_bytes={copied_bytes}");
    println!(
        "check={{\"sha256\":\"{}\",\"images\":{},\"pixels_exact\":true,\"archive_digest_verified\":true}}",
        hash(&images[target].pixels),
        1
    );
    Ok(())
}

// One group-scoped session for the level-five distortions. It retains the
// verified group bytes and pristine raster while seeking to each requested
// item; independent archive entries still use their own indexed payloads.
fn archive_batch(
    archive: &Path,
    group: &Path,
    pngs: &Path,
    arm: &str,
    threads: usize,
) -> Result<()> {
    assert!(matches!(
        arm,
        "independent" | "independent-list" | "reference"
    ));
    assert!([1, 2, 4, 8].contains(&threads));
    let images = load(&group.join("items.tsv"), pngs, &mut std::io::sink())?;
    let targets: Vec<_> = (5..images.len()).step_by(5).collect();
    assert_eq!(targets.len(), 25);

    let once = || -> Result<ArchiveObservation> {
        let open = Instant::now();
        let mut reader =
            imij_set::Reader::open(imij::planes::SeekReader::new(fs::File::open(archive)?))?;
        let mut lookup_ms = open.elapsed().as_secs_f64() * 1000.0;
        let mut copy_ms = 0.0;
        let mut decode_ms = 0.0;
        let mut copied_bytes = 0;
        let mut rasters = Vec::new();
        if arm != "reference" {
            let listed = if arm == "independent-list" {
                let start = Instant::now();
                let mut listing = reader.listing()?;
                let mut entries = Vec::new();
                for &target in &targets {
                    let wanted = target as u64 * 2 + 1;
                    loop {
                        let entry = listing.next(&mut reader)?.ok_or("missing archive entry")?;
                        if entry.key() == wanted {
                            entries.push(entry);
                            break;
                        }
                        assert!(entry.key() < wanted);
                    }
                }
                lookup_ms += start.elapsed().as_secs_f64() * 1000.0;
                Some(entries)
            } else {
                None
            };
            for (ordinal, &target) in targets.iter().enumerate() {
                let entry = if let Some(entries) = &listed {
                    entries[ordinal].clone()
                } else {
                    let start = Instant::now();
                    let entry = reader
                        .get(target as u64 * 2 + 1)?
                        .ok_or("missing archive entry")?;
                    lookup_ms += start.elapsed().as_secs_f64() * 1000.0;
                    entry
                };
                let start = Instant::now();
                let file = reader.read_entry_bounded(&entry, MAX_ENTRY_BYTES)?;
                copy_ms += start.elapsed().as_secs_f64() * 1000.0;
                copied_bytes += file.len();
                let start = Instant::now();
                let mut out = vec![0; images[target].pixels.len()];
                imij::decode_into_threads(
                    &file,
                    &mut out,
                    images[target].info.row_bytes(),
                    threads,
                )?;
                decode_ms += start.elapsed().as_secs_f64() * 1000.0;
                rasters.push((target, out));
            }
        } else {
            let start = Instant::now();
            let entry = reader.get(1)?.ok_or("missing reference group")?;
            let binding = imij_set::SequenceIndexRef::from_metadata(entry.metadata())?
                .ok_or("missing sequence index")?;
            let mut index = binding.open(&mut reader, &entry)?;
            let locations: Vec<_> = targets
                .iter()
                .map(|&target| index.get(target as u64))
                .collect::<std::io::Result<_>>()?;
            lookup_ms += start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let file = reader.read_entry_bounded(&entry, MAX_ENTRY_BYTES)?;
            copy_ms += start.elapsed().as_secs_f64() * 1000.0;
            copied_bytes += file.len();

            let start = Instant::now();
            let mut decoder = FrameDecoder::new(&file)?;
            assert_eq!(decoder.frame_count(), images.len());
            let pristine_needed = locations.iter().any(|item| item.reference_index == Some(0));
            let mut pristine = Vec::new();
            if pristine_needed {
                decoder.seek_frame(0)?;
                pristine = vec![0; images[0].pixels.len()];
                decoder.decode_into(&mut pristine, images[0].info.row_bytes(), threads)?;
            }
            for (&target, location) in targets.iter().zip(&locations) {
                let native = decoder.frame_seek_info(target)?;
                assert_eq!(location.encoded, native.encoded);
                assert_eq!(
                    location.reference_index,
                    native.reference_index.map(|n| n as u64)
                );
                assert_eq!(location.reference_hash, native.reference_hash);
                decoder.seek_frame(target)?;
                let stride = images[target].info.row_bytes();
                let mut out = vec![0; images[target].pixels.len()];
                if let Some(reference) = decoder.reference_index() {
                    assert_eq!(reference, 0);
                    decoder.decode_into_with_reference(
                        &pristine,
                        images[0].info.row_bytes(),
                        &mut out,
                        stride,
                        threads,
                    )?;
                } else {
                    decoder.decode_into(&mut out, stride, threads)?;
                }
                rasters.push((target, out));
            }
            if pristine_needed {
                rasters.push((0, pristine));
            }
            decode_ms += start.elapsed().as_secs_f64() * 1000.0;
        }
        Ok(ArchiveObservation {
            lookup_ms,
            copy_ms,
            decode_ms,
            rasters,
            copied_bytes,
        })
    };

    for _ in 0..3 {
        for (i, pixels) in once()?.rasters {
            assert_eq!(pixels, images[i].pixels);
        }
    }
    let ArchiveObservation {
        lookup_ms,
        copy_ms,
        decode_ms,
        rasters,
        copied_bytes,
    } = once()?;
    for (i, pixels) in &rasters {
        assert_eq!(pixels, &images[*i].pixels);
    }
    println!("lookup_ms={lookup_ms}");
    println!("copy_ms={copy_ms}");
    println!("decode_ms={decode_ms}");
    println!("elapsed_ms={}", lookup_ms + copy_ms + decode_ms);
    println!("decoded_images={}", rasters.len());
    println!("copied_bytes={copied_bytes}");
    let mut digest = Sha256::new();
    for &target in &targets {
        digest.update(&images[target].pixels);
    }
    println!(
        "check={{\"sha256\":\"{}\",\"images\":25,\"pixels_exact\":true,\"archive_digest_verified\":true}}",
        digest
            .finalize()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>()
    );
    Ok(())
}

// Attribution only: load and verify the original pixels and archive outside
// the sampling loop, then repeatedly reconstruct one fixed image. This is not
// an archive access or cold-I/O timing.
fn archive_decode_profile(archive: &Path, group: &Path, pngs: &Path, arm: &str) -> Result<()> {
    assert!(matches!(arm, "independent" | "reference"));
    let images = load(&group.join("items.tsv"), pngs, &mut std::io::sink())?;
    let target = images.len() - 1;
    let key = if arm == "independent" {
        target as u64 * 2 + 1
    } else {
        1
    };
    let mut reader =
        imij_set::Reader::open(imij::planes::SeekReader::new(fs::File::open(archive)?))?;
    let entry = reader.get(key)?.ok_or("missing archive entry")?;
    let file = reader.read_entry_bounded(&entry, MAX_ENTRY_BYTES)?;
    let stride = images[target].info.row_bytes();
    let mut output = vec![0; images[target].pixels.len()];
    let mut iterations = 0u64;
    let start;
    if arm == "reference" {
        let mut decoder = FrameDecoder::new(&file)?;
        assert_eq!(decoder.frame_count(), images.len());
        decoder.seek_frame(0)?;
        let mut pristine = vec![0; images[0].pixels.len()];
        decoder.decode_into(&mut pristine, images[0].info.row_bytes(), 1)?;
        assert_eq!(pristine, images[0].pixels);
        decoder.seek_frame(target)?;
        assert_eq!(decoder.reference_index(), Some(0));
        decoder.decode_into_with_reference(&pristine, stride, &mut output, stride, 1)?;
        assert_eq!(output, images[target].pixels);
        start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            decoder.seek_frame(target)?;
            decoder.decode_into_with_reference(&pristine, stride, &mut output, stride, 1)?;
            black_box(&output);
            iterations += 1;
        }
    } else {
        imij::decode_into_threads(&file, &mut output, stride, 1)?;
        assert_eq!(output, images[target].pixels);
        start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            imij::decode_into_threads(&file, &mut output, stride, 1)?;
            black_box(&output);
            iterations += 1;
        }
    }
    assert_eq!(output, images[target].pixels);
    println!("profile_arm={arm}");
    println!("profile_iterations={iterations}");
    println!("profile_seconds={}", start.elapsed().as_secs_f64());
    println!("profile_output_sha256={}", hash(&output));
    Ok(())
}

fn pack(list: &Path, output: &Path, indexed: bool) -> Result<()> {
    let paths: Vec<_> = fs::read_to_string(list)?
        .lines()
        .map(std::path::PathBuf::from)
        .collect();
    let mut writer = imij_set::Writer::new(std::io::BufWriter::new(fs::File::create_new(output)?));
    let mut expected = Vec::new();
    let mut payload_bytes = 0u64;
    let mut index_bytes = 0u64;
    for (ordinal, path) in paths.iter().enumerate() {
        let bytes = fs::read(path)?;
        let mut decoder = FrameDecoder::new(&bytes)?;
        decoder.next_frame()?.unwrap();
        let info = decoder.frame_info().unwrap();
        let mut metadata = ItemMetadata::from_metadata(decoder.frame_metadata().unwrap())?;
        let mut summary = imij_set::Summary::default();
        summary.width = info.width;
        summary.height = info.height;
        summary.channels = info.channels as u8;
        summary.sample = info.sample as u8;
        summary.cicp = Some(info.cicp);
        let header = imij::header(&bytes)?;
        summary.coding = header.coding as u8;
        summary.features = header.features.bits();
        summary.image_count = Some(decoder.frame_count() as u64);
        let key = ordinal as u64 * 2 + 1;
        if indexed {
            let index = writer.insert_sequence_index(key - 1, &bytes, &imij::Limits::default())?;
            index_bytes += index.encoded_len();
            index.write_metadata(&mut metadata)?;
        }
        writer.insert_with_metadata(
            key,
            bytes.len() as u64,
            0,
            summary,
            &metadata,
            &mut bytes.as_slice(),
        )?;
        payload_bytes += bytes.len() as u64;
        expected.push((key, hash(&bytes), metadata, summary));
    }
    let (mut file, _) = writer.finish()?;
    file.flush()?;
    drop(file);
    let mut reader =
        imij_set::Reader::open(imij::planes::SeekReader::new(fs::File::open(output)?))?;
    for (key, digest, metadata, summary) in expected {
        let entry = reader.get(key)?.unwrap();
        assert_eq!(entry.metadata(), &metadata);
        assert_eq!(entry.summary(), summary);
        let mut bytes = Vec::new();
        reader.copy_to(&entry, &mut bytes)?;
        assert_eq!(hash(&bytes), digest);
        let header = imij::header(&bytes)?;
        assert_eq!(entry.summary().coding, header.coding as u8);
        assert_eq!(entry.summary().features, header.features.bits());
        if indexed {
            let binding = imij_set::SequenceIndexRef::from_metadata(&metadata)?.unwrap();
            let mut index = binding.open(&mut reader, &entry)?;
            let decoder = FrameDecoder::new(&bytes)?;
            assert_eq!(index.image_count(), decoder.frame_count() as u64);
            for ordinal in 0..decoder.frame_count() {
                let location = index.get(ordinal as u64)?;
                let native = decoder.frame_seek_info(ordinal)?;
                assert_eq!(location.encoded, native.encoded);
                assert_eq!(
                    location.reference_index,
                    native.reference_index.map(|n| n as u64)
                );
                assert_eq!(location.reference_hash, native.reference_hash);
            }
        }
    }
    println!(
        "{{\"entries\":{},\"payload_bytes\":{payload_bytes},\"sequence_index_bytes\":{index_bytes},\"archive_bytes\":{},\"all_entries_identical\":true}}",
        paths.len(),
        fs::metadata(output)?.len()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    match args.get(1).and_then(|s| s.to_str()) {
        Some("archive-access") if args.len() == 7 => archive_access(
            Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4]),
            args[5].to_str().unwrap(), args[6].to_str().unwrap().parse()?,
        ),
        Some("archive-batch") if args.len() == 7 => archive_batch(
            Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4]),
            args[5].to_str().unwrap(), args[6].to_str().unwrap().parse()?,
        ),
        Some("archive-decode-profile") if args.len() == 6 => archive_decode_profile(
            Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4]),
            args[5].to_str().unwrap(),
        ),
        Some("measure") if args.len() == 7 => measure(Path::new(&args[2]), Path::new(&args[3]),
            args[4].to_str().unwrap(), args[5].to_str().unwrap(), args[6].to_str().unwrap().parse()?),
        Some("generate") if args.len() == 5 => generate(Path::new(&args[2]), Path::new(&args[3]), Path::new(&args[4])),
        Some("pack") if args.len() == 5 && (args[4] == "indexed" || args[4] == "plain") => pack(Path::new(&args[2]), Path::new(&args[3]), args[4] == "indexed"),
        _ => Err("iqa_storage generate ITEMS.tsv PNG-DIR NEW-OUTPUT | pack FILE-LIST NEW-ARCHIVE indexed|plain".into()),
    }
}
