use super::*;
use alloc::collections::BTreeSet;

fn checksum_chunk(file: &[u8]) -> (usize, usize, u8) {
    let mut r = crate::ByteReader::new(file);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    loop {
        let at = r.pos;
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        let body = crate::parse::chunks::payload(&mut r, len.unwrap()).unwrap();
        if tag == *b"cSUM" {
            return (at, r.pos - at, body[2]);
        }
    }
}

fn move_checksum_after_data(file: &[u8]) -> Vec<u8> {
    let mut r = crate::ByteReader::new(file);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let mut output = file[..r.pos].to_vec();
    let (mut checksum, mut index) = (&[][..], &[][..]);
    while r.remaining() != 0 {
        let at = r.pos;
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        r.bytes(len.unwrap() as usize).unwrap();
        match &tag {
            b"cSUM" => checksum = &file[at..r.pos],
            b"uIDX" => index = &file[at..r.pos],
            _ => output.extend_from_slice(&file[at..r.pos]),
        }
    }
    output.extend_from_slice(checksum);
    output.extend_from_slice(index);
    // Moving the excluded chunk retains the exact covered byte sequence.
    assert_eq!(
        crate::decode(&output).unwrap().1,
        crate::decode(file).unwrap().1
    );
    output
}

fn placement(file: &[u8], placement: DirectoryPlacement, open: bool) -> Vec<u8> {
    let mut r = crate::ByteReader::new(file);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let mut unsealed = file[..r.pos].to_vec();
    while r.remaining() != 0 {
        let at = r.pos;
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        r.bytes(len.unwrap() as usize).unwrap();
        if tag != *b"cSUM" {
            unsealed.extend_from_slice(&file[at..r.pos]);
        }
    }
    let changed = crate::v4::file_tests::rewrite_placement(&unsealed, placement, open);
    let sealed = crate::checksum::refinish(changed).unwrap();
    assert_eq!(
        crate::decode(&sealed).unwrap().1,
        crate::decode(file).unwrap().1
    );
    sealed
}

// Independent piece/offset arithmetic: use actual source requests to identify
// which rounded checksum pieces they require, including the excluded seam.
fn permitted(file: &[u8], wanted: &[Range<u64>]) -> Vec<Range<u64>> {
    let (at, len, log2) = checksum_chunk(file);
    let (at, len) = (at as u64, len as u64);
    let covered_len = file.len() as u64 - len;
    let covered = |n: u64| {
        if n <= at {
            n
        } else {
            n.saturating_sub(len).max(at)
        }
    };
    let mut ids = BTreeSet::new();
    for range in wanted {
        let (a, b) = (covered(range.start), covered(range.end));
        if a < b {
            ids.extend((a >> log2)..((b - 1) >> log2) + 1);
        }
    }
    let mut ranges = Vec::new();
    for id in ids {
        let (a, b) = (id << log2, ((id + 1) << log2).min(covered_len));
        if a < at {
            ranges.push(a..b.min(at));
        }
        if b > at {
            ranges.push(a.max(at) + len..b + len);
        }
    }
    ranges
}

fn verify_groups(
    reader: &mut NumericTileReader<'_, Source, fn() -> bool>,
    pixels: &[u8],
    info: &ImageInfo,
) {
    let mut groups = 0;
    reader
        .read_tile(4, &mut Vec::new(), |work| {
            let rect = work.rect()?;
            let row = rect.width() as usize * info.bytes_per_pixel();
            let stride = row + 13;
            let mut output = vec![0xC7; stride * rect.height() as usize];
            work.decode_into(
                &mut output,
                stride,
                &DecodeSession::new(DecodePolicy::default()),
            )?;
            for y in 0..rect.height() as usize {
                let start = (rect.y() as usize + y) * info.row_bytes()
                    + rect.x() as usize * info.bytes_per_pixel();
                assert_eq!(&output[y * stride..][..row], &pixels[start..start + row]);
                assert!(
                    output[y * stride + row..(y + 1) * stride]
                        .iter()
                        .all(|&v| v == 0xC7)
                );
            }
            groups += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(groups, 9);
}

#[test]
fn sparse_checksum_pieces_cover_used_metadata_and_tile_without_refetching_it() {
    let info = ImageInfo::new(511, 257, Channels::Rgba, SampleType::U16);
    let mut rng = 0xF917_463A_8215_6E0Bu64;
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng as u8
        })
        .collect();
    for quick in [false, true] {
        for mode in [ObjectIntegrity::None, ObjectIntegrity::ObjectsAndTiles] {
            let spatial = spatial(mode, TileOrder::Morton);
            let config = if quick {
                ImiEncoderConfig::quick()
            } else {
                ImiEncoderConfig::lossless()
            }
            .with_row_groups(RowGroups::new(9, 7, 8 << 20))
            .with_checksum(true);
            let original = encode_numeric(
                info.clone(),
                &pixels,
                info.row_bytes(),
                &Default::default(),
                &config,
                spatial,
            )
            .unwrap();
            assert!(
                original.len() > 4 * 65536,
                "fixture needs unread checksum pieces"
            );
            let mut head = crate::ByteReader::new(&original);
            let parsed = crate::parse::header::read_header_v4(&mut head, 64 << 10).unwrap();
            let mut bad_header = original.clone();
            bad_header[parsed.checksum.unwrap().at] = 0;
            let source = Source::new(bad_header);
            let reads = Arc::clone(&source.reads);
            let session = DecodeSession::new(DecodePolicy::default());
            assert!(matches!(
                NumericTileReader::open(source, spatial.limits, &session),
                Err(crate::Error::Corrupt(_))
            ));
            assert!(
                reads
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|r| r.end <= head.pos as u64)
            );
            for place in [DirectoryPlacement::Front, DirectoryPlacement::Tail] {
                for open in [false, true] {
                    let bytes = placement(&original, place, open);
                    let session = DecodeSession::new(DecodePolicy::default());
                    let mut reader =
                        NumericTileReader::open(Source::new(bytes), spatial.limits, &session)
                            .unwrap();
                    verify_groups(&mut reader, &pixels, &info);
                }
            }
            for after in [false, true] {
                let bytes = if after {
                    move_checksum_after_data(&original)
                } else {
                    original.clone()
                };
                let (objects, tiles) = physical_tiles(&bytes, 247);
                let selected = tiles.iter().find(|(id, _)| *id == 4).unwrap().1.clone();
                let source = Source::new(bytes.clone());
                let reads = Arc::clone(&source.reads);
                let session = DecodeSession::new(DecodePolicy::default());
                let mut reader = NumericTileReader::open(source, spatial.limits, &session).unwrap();
                let mut wanted = reads.lock().unwrap().clone();
                assert!(
                    wanted
                        .iter()
                        .all(|r| r.end <= objects.start || r.start >= objects.end)
                );
                wanted.push(selected.clone());
                let allowed = permitted(&bytes, &wanted);
                reads.lock().unwrap().clear();
                verify_groups(&mut reader, &pixels, &info);
                let fetched = reads.lock().unwrap().clone();
                assert_eq!(fetched[0], selected);
                for range in &fetched[1..] {
                    assert!(
                        range.end <= selected.start || range.start >= selected.end,
                        "refetched selected tile bytes: {range:?}"
                    );
                    assert!(
                        allowed
                            .iter()
                            .any(|a| a.start <= range.start && a.end >= range.end)
                    );
                    assert!(range.end - range.start <= 65536);
                }
                let opening_bytes: u64 = wanted[..wanted.len() - 1]
                    .iter()
                    .map(|r| r.end - r.start)
                    .sum();
                let fetched_bytes: u64 = fetched.iter().map(|r| r.end - r.start).sum();
                assert_eq!(session.usage().input_bytes, opening_bytes + fetched_bytes);
                reads.lock().unwrap().clear();
                verify_groups(&mut reader, &pixels, &info);
                assert_eq!(*reads.lock().unwrap(), vec![selected.clone()]);

                let mut changed = bytes.clone();
                changed[(selected.start + (selected.end - selected.start) / 2) as usize] ^= 1;
                let session = DecodeSession::new(DecodePolicy::default());
                let mut reader =
                    NumericTileReader::open(Source::new(changed), spatial.limits, &session)
                        .unwrap();
                let mut delivered = 0;
                assert_eq!(
                    reader.read_tile(4, &mut Vec::new(), |_| {
                        delivered += 1;
                        Ok(())
                    }),
                    Err(crate::checksum::MISMATCH)
                );
                assert_eq!(delivered, 0);

                // Unread, unrounded pixel bodies are outside fetched coverage.
                let untouched = tiles
                    .iter()
                    .map(|(_, range)| range.start + 8)
                    .find(|pos| !allowed.iter().any(|r| r.contains(pos)))
                    .expect("fixture needs an unverified pixel piece");
                let mut changed = bytes.clone();
                changed[untouched as usize] ^= 1;
                let session = DecodeSession::new(DecodePolicy::default());
                let mut reader =
                    NumericTileReader::open(Source::new(changed), spatial.limits, &session)
                        .unwrap();
                verify_groups(&mut reader, &pixels, &info);

                let budget = opening_bytes + selected.end - selected.start;
                let session =
                    DecodeSession::new(DecodePolicy::default().with_max_input_bytes(budget));
                let source = Source::new(bytes.clone());
                let reads = Arc::clone(&source.reads);
                let mut reader = NumericTileReader::open(source, spatial.limits, &session).unwrap();
                reads.lock().unwrap().clear();
                assert_eq!(
                    reader.read_tile(4, &mut Vec::new(), |_| panic!(
                        "budget failure published work"
                    )),
                    Err(crate::Error::TooLarge)
                );
                assert_eq!(*reads.lock().unwrap(), vec![selected.clone()]);
                assert_eq!(session.usage().input_bytes, budget);

                let (at, len, _) = checksum_chunk(&bytes);
                let mut changed = bytes;
                changed[at + len - 1] ^= 1;
                let session = DecodeSession::new(DecodePolicy::default());
                let mut reader =
                    NumericTileReader::open(Source::new(changed.clone()), spatial.limits, &session)
                        .unwrap();
                assert_eq!(
                    reader.read_tile(4, &mut Vec::new(), |_| panic!(
                        "bad metadata piece published work"
                    )),
                    Err(crate::checksum::MISMATCH)
                );
                let limits = crate::Limits {
                    verify_checksum: false,
                    ..crate::Limits::DEFAULT
                };
                let session =
                    DecodeSession::new(DecodePolicy::default().with_output_limits(limits));
                let mut reader =
                    NumericTileReader::open(Source::new(changed), spatial.limits, &session)
                        .unwrap();
                verify_groups(&mut reader, &pixels, &info);
            }
        }
    }
}
