//! Explicit metadata remuxing. Existing encoders and `Metadata` values retain
//! their byte representation unless the caller invokes this helper.

use alloc::vec::Vec;

use crate::parse::chunks::{self, Kind, Rules};
use crate::{ByteReader, Error, container, metadata_pack};

fn compressible(tag: [u8; 4]) -> bool {
    matches!(&tag, b"iCCP" | b"eXIf" | b"xMPp")
}

/// Visits complete original chunk byte ranges, under the chunk rules every
/// reader applies (`VALIDATE`: also the caller's lower cap on metadata
/// blocks, raw or compressed). Returns the header's end and the `cSUM`
/// chunk (position, length, payload), which is never visited.
fn chunks<const VALIDATE: bool>(
    file: &[u8],
    max_block_bytes: usize,
    mut visit: impl FnMut([u8; 4], &[u8], &[u8]) -> Result<(), Error>,
) -> Result<(usize, Option<(usize, usize, &[u8])>), Error> {
    let mut reader = ByteReader::new(file);
    let hp = crate::parse::header::read_versioned(&mut reader, u64::MAX)?;
    let mixed_pages = hp.mixed_pages();
    let crate::parse::header::ParsedHeader {
        header,
        declared,
        checksum: announced,
        ..
    } = hp;
    let head_end = reader.pos;
    let v4_session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default()
            .with_output_limits(crate::Limits::NONE)
            .for_structure()
            .with_max_input_bytes(file.len() as u64),
    );
    let mut rules = Rules::new(mixed_pages, true)
        .with_sequence_items(declared.contains(crate::parse::header::Feature::SequenceItems));
    let mut checksum = None;
    // A rule or parse error with the table read before it: a mismatching
    // piece under the bytes read explains it (SPEC 1.4).
    let walked = (|| -> Result<(), Error> {
        while reader.remaining() != 0 {
            let start = reader.pos;
            let (tag, len) = chunks::read_head(&mut reader)?;
            if chunks::is_plane_chunk(&tag) {
                return Err(Error::Unsupported(
                    "metadata remux requires rebuilding plane indexes",
                ));
            }
            let kind = rules.head(tag, len)?;
            let body = match len {
                Some(n) => chunks::payload(&mut reader, n)?,
                None => {
                    let size = if header.version == 4 {
                        crate::v4::file::open_data_len(
                            &header,
                            declared,
                            &file[reader.pos..],
                            &v4_session,
                        )?
                    } else {
                        crate::v1::payload_len(&header, declared, &file[reader.pos..])?
                    };
                    reader.bytes(size)?
                }
            };
            if kind == Kind::Checksum {
                // Checked by the caller, never visited: the remuxed file gets a
                // new table.
                checksum = Some((start, reader.pos - start, body));
                continue;
            }
            if tag == chunks::MCMP {
                rules.identity(tag, body)?;
                // The record under the caller's cap, once (the writing pass
                // copies records the validating pass checked): its declared
                // length before its syntax (limits first); the cap is at most
                // the decoders' own.
                if VALIDATE {
                    crate::metadata_pack::check(&body[4..], max_block_bytes)?;
                }
            } else if kind.has_body_rules(&tag) {
                rules.body(tag, body)?;
            }
            if VALIDATE && tag != chunks::MCMP && compressible(tag) && body.len() > max_block_bytes
            {
                return Err(Error::TooLarge);
            }
            visit(tag, body, &file[start..reader.pos])?;
        }
        rules.finish()
    })();
    if let Err(e) = walked {
        if header.version == 4 {
            return Err(e);
        }
        let announced = announced.map(|a| a.announced);
        let end = reader.pos;
        return Err(crate::checksum::explain_in_memory(
            file, announced, checksum, end, e,
        ));
    }
    Ok((head_end, checksum))
}

/// Compresses eligible ICC, Exif and XMP metadata using the format's fixed
/// dictionary, keeping the complete original chunk when compression would not
/// reduce its serialized size. Pixel data, headers, chunk order and all other
/// chunks retain their exact bytes. Already compressed metadata is validated
/// and retained byte-for-byte; nested images and frames are not rewritten.
///
/// This is an explicit encoder-side remux: it allocates a new file buffer, with
/// additional workspace bounded by one metadata block and the packing table.
/// `max_block_bytes` applies to each eligible decoded block and must be at most
/// 16 MiB, matching decoder limits. A lower caller limit is honored for both raw
/// and previously compressed metadata. Other opaque chunks are copied without
/// decoding them. This validates container structure, not pixel entropy content.
///
/// Files carrying plane records/indexes/locators are rejected because moving
/// chunks would invalidate their absolute offsets. Compress metadata before
/// constructing a plane set. No codec dictionary is downloaded or embedded.
///
/// A `cSUM` file checksum is verified, then recomputed over the new bytes
/// (before `DATA`); a checksum algorithm this library does not know is
/// refused with [`Error::Unsupported`] rather than dropped.
pub fn compress_metadata(file: &[u8], max_block_bytes: usize) -> Result<Vec<u8>, Error> {
    if max_block_bytes > crate::parse::chunks::MAX_PACKED_METADATA {
        return Err(Error::InvalidInput(
            "metadata block limit exceeds decoder cap",
        ));
    }
    let (head_end, chunk) = chunks::<true>(file, max_block_bytes, |_, _, _| Ok(()))?;
    let announced = crate::parse::header::read_versioned(&mut ByteReader::new(file), u64::MAX)?
        .checksum
        .map(|c| c.announced);
    let pieces = crate::checksum::pieces(file, announced, chunk)?;
    if let Some(p) = &pieces {
        p.verify_all(1)?;
    }
    let checksum =
        container::ChecksumChunk::rewritable(chunk.map(|(at, len, _)| container::ChecksumChunk {
            at,
            len,
            pieces,
        }))?;
    let mut out = Vec::new();
    out.try_reserve_exact(file.len())
        .map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(&file[..head_end]);
    chunks::<false>(file, max_block_bytes, |tag, body, original| {
        if compressible(tag) {
            let packed = metadata_pack::pack(body)?;
            let body_len = 4usize.checked_add(packed.len()).ok_or(Error::TooLarge)?;
            let framed_len = body_len.checked_add(1).ok_or(Error::TooLarge)?;
            let chunk_len = 4usize
                .checked_add(crate::varint_len(framed_len as u64))
                .and_then(|n| n.checked_add(body_len))
                .ok_or(Error::TooLarge)?;
            if packed.first() == Some(&1) && chunk_len < original.len() {
                out.extend_from_slice(&crate::parse::chunks::MCMP);
                crate::put_varint(&mut out, framed_len as u64);
                out.extend_from_slice(&tag);
                out.extend_from_slice(&packed);
                return Ok(());
            }
        }
        out.extend_from_slice(original);
        Ok(())
    })?;
    // Chunk lengths before DATA changed: refresh the DATA locator (before
    // the checksum, which covers it).
    let out = container::locate(out)?;
    if checksum {
        return crate::checksum::refinish(out);
    }
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::Metadata;
    use crate::{Channels, Coding, DecodeSpeed, EncodeConfig, ImageInfo, SampleType};
    use alloc::vec;

    #[test]
    fn v4_remux_preserves_pixel_objects_and_refinishes_encoded_checksums() {
        let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73) as u8)
            .collect();
        let metadata = Metadata {
            icc: Some(vec![0x55; 4096]),
            exif: Some(vec![0x33; 2048]),
            xmp: Some(vec![0x22; 2048]),
            ..Default::default()
        };
        for lossy in [false, true] {
            for checksum in [false, true] {
                let config = if lossy {
                    crate::ImiEncoderConfig::lossy(71.)
                } else {
                    crate::ImiEncoderConfig::lossless()
                }
                .with_checksum(checksum);
                let file = config
                    .encode_samples(crate::ImageRef::packed(&pixels, &info), &metadata)
                    .unwrap();
                let expected = crate::decode(&file).unwrap();
                let packed = compress_metadata(&file, 1 << 20).unwrap();
                assert!(
                    packed.len() < file.len(),
                    "compressible metadata must be packed"
                );
                assert_eq!(crate::header(&packed).unwrap().version, 4);
                assert_eq!(crate::decode(&packed).unwrap(), expected);
                let session = crate::decode_resources::DecodeSession::new(Default::default());
                let before = crate::v4::file::parse_file(&file, &session).unwrap();
                let after = crate::v4::file::parse_file(&packed, &session).unwrap();
                assert_eq!(after.metadata().unwrap(), before.metadata().unwrap());
                assert_eq!(
                    before.data, after.data,
                    "all pixel objects and local digests stay exact"
                );
                assert_eq!(compress_metadata(&packed, 1 << 20).unwrap(), packed);
            }
        }
    }

    fn image(metadata: &Metadata) -> Vec<u8> {
        crate::encode(
            &[17, 29, 41],
            3,
            &ImageInfo::new(1, 1, Channels::Rgb, SampleType::U8),
            metadata,
            &EncodeConfig {
                threads: 1,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn xmp() -> Vec<u8> {
        b"<rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"><xmp:CreatorTool>fixture</xmp:CreatorTool></rdf:Description>".repeat(32)
    }

    #[test]
    fn saves_complete_chunks_preserving_pixels_metadata_and_order() {
        let metadata = Metadata {
            icc: Some(b"acspmntrRGB XYZ rXYZgXYZbXYZrTRCgTRCbTRC".repeat(64)),
            exif: Some(vec![0; 4096]),
            xmp: Some(xmp()),
            chunks: vec![crate::Chunk {
                tag: *b"tESt",
                data: vec![1, 2, 3, 4],
            }],
            ..Default::default()
        };
        let original = image(&metadata);
        let packed = compress_metadata(&original, 16384).unwrap();
        assert!(packed.len() < original.len());
        assert_eq!(
            crate::decode(&packed).unwrap(),
            crate::decode(&original).unwrap()
        );
        let before = crate::v1::parse_header(&original).unwrap();
        let after = crate::v1::parse_header(&packed).unwrap();
        assert_eq!(before.0, after.0);
        assert_eq!(before.1, after.1);
        assert_eq!(before.2, after.2);
        let mut compressed_tags = Vec::new();
        chunks::<true>(&packed, 16384, |tag, body, _| {
            if tag == crate::parse::chunks::MCMP {
                compressed_tags.push(body[..4].to_vec());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            compressed_tags,
            [b"iCCP".to_vec(), b"eXIf".to_vec(), b"xMPp".to_vec()]
        );
        assert_eq!(compress_metadata(&packed, 16384).unwrap(), packed);
    }

    #[test]
    fn no_savings_retains_every_original_byte() {
        let original = image(&Metadata {
            icc: Some(vec![17, 29, 41]),
            exif: Some(vec![]),
            xmp: Some(vec![255; 5]),
            ..Default::default()
        });
        assert_eq!(compress_metadata(&original, 8).unwrap(), original);
        let plain = image(&Metadata::default());
        assert_eq!(compress_metadata(&plain, 0).unwrap(), plain);
    }

    #[test]
    fn open_data_and_trailing_metadata_keep_pixel_framing_exact() {
        let baseline = image(&Metadata::default());
        let (info, _, data) = crate::v1::parse_header(&baseline).unwrap();
        let mut original = Vec::new();
        container::write_head(
            &mut original,
            &info,
            &Metadata::default(),
            Coding::Lossless,
            None,
            DecodeSpeed::Balanced,
        )
        .unwrap();
        original.extend_from_slice(&crate::parse::chunks::DATA);
        original.push(0);
        original.extend_from_slice(data);
        container::put_chunk(&mut original, b"xMPp", &xmp());
        container::put_chunk(&mut original, b"tESt", &[9, 8, 7]);
        let packed = compress_metadata(&original, 16384).unwrap();
        assert_eq!(
            crate::decode(&packed).unwrap(),
            crate::decode(&original).unwrap()
        );
        let mut before = Vec::new();
        let mut after = Vec::new();
        chunks::<true>(&original, 16384, |tag, _, raw| {
            if tag == crate::parse::chunks::DATA {
                before.extend_from_slice(raw);
            }
            Ok(())
        })
        .unwrap();
        chunks::<true>(&packed, 16384, |tag, _, raw| {
            if tag == crate::parse::chunks::DATA {
                after.extend_from_slice(raw);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(before, after);
        assert_eq!(after[4], 0);
    }

    #[test]
    fn rejects_indexed_files_limits_duplicates_and_invalid_packing() {
        let baseline = image(&Metadata::default());
        for tag in [b"pDAT", b"pIDX", b"sLOC"] {
            let mut file = baseline.clone();
            container::put_chunk(&mut file, tag, &[]);
            assert!(matches!(
                compress_metadata(&file, 16384),
                Err(Error::Unsupported(_))
            ));
        }
        let original = image(&Metadata {
            xmp: Some(xmp()),
            ..Default::default()
        });
        assert_eq!(compress_metadata(&original, 16), Err(Error::TooLarge));
        let packed = compress_metadata(&original, 16384).unwrap();
        assert_eq!(compress_metadata(&packed, 16), Err(Error::TooLarge));
        assert!(matches!(
            compress_metadata(&baseline, crate::parse::chunks::MAX_PACKED_METADATA + 1),
            Err(Error::InvalidInput(_))
        ));
        let mut duplicate = original.clone();
        // Duplicate identity wins before trying to unpack invalid bytes.
        container::put_chunk(&mut duplicate, &crate::parse::chunks::MCMP, b"xMPpINVALID");
        assert_eq!(
            compress_metadata(&duplicate, 16384),
            Err(Error::Corrupt("repeated chunk"))
        );
        for body in [
            &b""[..],
            &b"DATA\x00\x00"[..],
            &b"mCMP\x00\x00"[..],
            &b"xMPpINVALID"[..],
        ] {
            let mut file = baseline.clone();
            container::put_chunk(&mut file, &crate::parse::chunks::MCMP, body);
            assert!(compress_metadata(&file, 16384).is_err());
        }
    }

    #[test]
    fn validates_chunk_controls_without_copying_opaque_payloads() {
        let baseline = image(&Metadata::default());
        for tag in [b"DATA", b"aNIM", b"ABCD", b"mDCv", b"cLLI", b"t1St"] {
            let mut file = baseline.clone();
            container::put_chunk(&mut file, tag, &[]);
            assert!(compress_metadata(&file, 16384).is_err(), "{tag:?}");
        }
        let mut file = baseline.clone();
        file.extend_from_slice(b"xMPp\x00");
        assert!(compress_metadata(&file, 16384).is_err());
        let mut file = baseline.clone();
        file.extend_from_slice(b"xMPp\x05\x01");
        assert!(compress_metadata(&file, 16384).is_err());
        assert!(compress_metadata(&baseline[..baseline.len() - 1], 16384).is_err());
    }
}
