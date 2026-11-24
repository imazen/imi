//! PNG import color policy over zenpng's parsed metadata.
//!
//! Color policy and rendered-image transcoding use zenpng's public decoder.
//! Animation frames are retained as composited canvases with rational timing.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod export;
pub use export::{ExportConfig, ExportJob, Exporter};
mod transcode;
pub use transcode::{
    TranscodeConfig, TranscodeError, TranscodeJob, TranscodeOutput, Transcoder, transcode,
};

use imij::{Cicp, Error, Metadata};

/// `Metadata::chunks` marker recording where the source PNG's first `IDAT`
/// fell: entries before it were `BeforeIdat`, entries after `AfterIdat`.
/// `idat` is not a legal PNG chunk name (the reserved third-letter bit is
/// clear), so it can never collide with a preserved chunk; it is
/// IMI-internal and never emitted to PNG output.
pub(crate) const PNG_IDAT_BOUNDARY: [u8; 4] = *b"idat";

/// Resolve PNG precedence and construct active IMI color/HDR metadata.
///
/// Gamma 2.2 is intentionally normalized to sRGB's transfer curve. Missing
/// gamma/primaries assume sRGB/D65 BT.709. This is an import compatibility
/// policy, not a mathematical equivalence or a pixel conversion.
///
/// Legacy gAMA accepts 45454/45455 (gamma 2.2) and 100000 (linear). Legacy cHRM
/// accepts D65 BT.709, Display P3 and BT.2020, within one 1/100000 coordinate
/// unit. Other legacy descriptions return Unsupported. Higher-priority color
/// descriptions bypass that fallback entirely.
///
/// Superseded ICC bytes are not installed as active IMI metadata, because
/// IMI gives active ICC precedence over CICP. Historical color carriers and
/// precedence are retained separately in [`Metadata::color_origin`]. Unknown
/// safe-to-copy ancillary chunks are retained verbatim in
/// [`Metadata::chunks`]; unsafe-to-copy unknowns and deprecated gAMA/cHRM
/// values are not retained.
pub fn color_metadata(png: &zenpng::PngInfo) -> Result<(Cicp, Metadata), Error> {
    let mut meta = Metadata {
        exif: png.exif.clone(),
        xmp: png.xmp.clone(),
        mastering_display: png.mastering_display_raw,
        content_light_level: png.content_light_level_raw.map(|values| {
            let mut bytes = [0; 8];
            bytes[..4].copy_from_slice(&values[0].to_be_bytes());
            bytes[4..].copy_from_slice(&values[1].to_be_bytes());
            bytes
        }),
        orientation: 1,
        ..Default::default()
    };
    let cicp = if let Some(c) = png.cicp {
        if c.matrix_coefficients != 0 || !c.full_range {
            return Err(Error::Unsupported("PNG CICP matrix/range"));
        }
        Cicp {
            color_primaries: c.color_primaries,
            transfer_characteristics: c.transfer_characteristics,
            matrix_coefficients: 0,
            full_range: true,
        }
    } else if let Some(icc) = &png.icc_profile {
        meta.icc = Some(icc.clone());
        // The active ICC supplies color meaning. This fallback header value
        // is not a claim that the ICC describes sRGB.
        Cicp::SRGB
    } else if let Some(intent) = png.srgb_intent {
        if intent != 0 {
            return Err(Error::Unsupported("PNG sRGB rendering intent"));
        }
        Cicp::SRGB
    } else {
        legacy_cicp(png)?
    };
    let origin = if let Some(c) = png.cicp {
        match &png.icc_profile {
            Some(icc) => zenpixels::ColorOrigin::from_icc_and_cicp(icc.as_slice(), c),
            None => zenpixels::ColorOrigin::from_cicp(c),
        }
    } else if let Some(icc) = &png.icc_profile {
        zenpixels::ColorOrigin::from_icc(icc.as_slice())
    } else if png.srgb_intent.is_some() {
        // The shared provenance type represents named sRGB as its CICP equivalent.
        zenpixels::ColorOrigin::from_cicp(zenpixels::Cicp::SRGB)
    } else if png.source_gamma.is_some() || png.chromaticities.is_some() {
        zenpixels::ColorOrigin::from_gama_chrm()
    } else {
        zenpixels::ColorOrigin::assumed()
    };
    meta.set_color_origin(Some(&origin))?;
    Ok((cicp, meta))
}

fn legacy_cicp(png: &zenpng::PngInfo) -> Result<Cicp, Error> {
    let transfer = match png.source_gamma {
        None | Some(45454 | 45455) => 13,
        Some(100000) => 8,
        _ => return Err(Error::Unsupported("PNG gamma has no accepted CICP mapping")),
    };
    let primaries = match png.chromaticities {
        None => 1,
        Some(c) => {
            let values = [
                c.white_x, c.white_y, c.red_x, c.red_y, c.green_x, c.green_y, c.blue_x, c.blue_y,
            ];
            // cHRM coordinates in PNG's 1/100000 units; CICP primaries 1/12/9.
            let known = [
                (1, [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000]),
                (12, [31270, 32900, 68000, 32000, 26500, 69000, 15000, 6000]),
                (9, [31270, 32900, 70800, 29200, 17000, 79700, 13100, 4600]),
            ];
            known
                .into_iter()
                .find(|(_, reference)| {
                    values
                        .iter()
                        .zip(reference)
                        .all(|(a, b)| a.abs_diff(*b) <= 1)
                })
                .map(|(code, _)| code)
                .ok_or(Error::Unsupported(
                    "PNG chromaticities have no accepted CICP mapping",
                ))?
        }
    };
    Ok(Cicp {
        color_primaries: primaries,
        transfer_characteristics: transfer,
        ..Cicp::SRGB
    })
}
