//! Frozen-source HDR code-grid study; no timing or SDR perceptual metrics.
//! `hdr_precision_probe SOURCE_DIRECTORY NEW_OUTPUT_DIRECTORY`
#![forbid(unsafe_code)]
use imij::{
    Channels, Cicp, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const SIDE: usize = 512;
const SOURCES: [(&str, &str); 4] = [
    (
        "1228",
        "ae1e0531b643751dabf548754a0cbce80f6415ba5c97f0934fe4c357434706a7",
    ),
    (
        "1492",
        "8053759201e3b3ffbf342f6d564d6268ae88b210bf3eca81908731aa29e4dc07",
    ),
    (
        "1503",
        "7a058c47247dc9fa0dd204999959fdf66ce018f624fe9875da4b0c84ab5edfae",
    ),
    (
        "1546",
        "4471a1b3c264dd78fe917552a90d1223831e974198454540d8ed3b4103e1148a",
    ),
];

fn need(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn save(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn hash(bytes: &[u8]) -> Result<String> {
    // Hashing is outside any measurement (this probe has no timing command).
    // Reuse the system digest tool instead of another crate or SHA implementation.
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("hash stdin missing")?
        .write_all(bytes)?;
    let result = child.wait_with_output()?;
    need(result.status.success(), "sha256sum failed")?;
    let text = String::from_utf8(result.stdout)?;
    let digest = text.split_whitespace().next().ok_or("hash missing")?;
    need(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid hash",
    )?;
    Ok(digest.to_owned())
}
fn round_even(numerator: u64, denominator: u64) -> u16 {
    let q = numerator / denominator;
    let r = numerator % denominator;
    (q + u64::from(r * 2 > denominator || (r * 2 == denominator && q & 1 != 0))) as u16
}
fn grid(value: u16, bits: u8) -> u16 {
    round_even(u64::from(value) * ((1u64 << bits) - 1), 65535)
}
fn scaled(value: u16, bits: u8) -> u16 {
    round_even(u64::from(value) * 65535, (1u64 << bits) - 1)
}
fn bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|v| v.to_ne_bytes()).collect()
}
fn words(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_ne_bytes)
        .collect()
}
fn errors(source: &[u16], actual: &[u16]) -> Value {
    assert_eq!(source.len(), actual.len());
    let mut absolute = 0u64;
    let mut squared = 0u64;
    let mut maximum = 0u16;
    let mut changed = 0usize;
    let mut unit = 0usize;
    for (&a, &b) in source.iter().zip(actual) {
        let e = a.abs_diff(b);
        absolute += u64::from(e);
        squared += u64::from(e) * u64::from(e);
        maximum = maximum.max(e);
        changed += usize::from(e != 0);
        unit += usize::from(e == 1);
    }
    json!({"components":source.len(),"changed_components":changed,"one_unit_errors":unit,
        "max_abs":maximum,"mae":absolute as f64/source.len() as f64,
        "rmse":(squared as f64/source.len() as f64).sqrt()})
}

struct Cell<'a> {
    image: &'a str,
    bits: u8,
    embedding: &'a str,
    info: &'a ImageInfo,
    input: &'a [u8],
    reference: &'a [u16],
    input_hash: &'a str,
    directory: &'a Path,
}
impl Cell<'_> {
    fn run(&self, quality: Option<f32>, color: LossyColor) -> Result<Value> {
        let color_name = if quality.is_none() {
            "lossless"
        } else if color == LossyColor::Xyb {
            "xyb"
        } else {
            "ycocg"
        };
        let name = format!(
            "{}-n{}-{}-{color_name}-{}",
            self.image,
            self.bits,
            self.embedding,
            quality.map_or("exact".to_string(), |q| format!("q{q}"))
        );
        let file = if let Some(q) = quality {
            let config = imij::ImiEncoderConfig::lossy(q)
                .tune_lossy(|t| t.color = color)
                .with_threads(1)
                .with_effort(10.0)
                .tune_lossy(|t| t.entropy = LossyEntropy::RunLevel)
                .tune_lossy(|t| t.compact_streams = true);
            config.encode_samples(
                imij::ImageRef::new(self.input, self.info.row_bytes(), self.info),
                &Metadata::default(),
            )?
        } else {
            imij::ImiEncoderConfig::default()
                .with_effort(10.0)
                .encode_samples(
                    imij::ImageRef::new(self.input, self.info.row_bytes(), self.info),
                    &Metadata::default(),
                )?
        };
        let (decoded_info, decoded) = imij::decode(&file)?;
        need(decoded_info == *self.info, "decoded precision/CICP differs")?;
        if quality.is_none() {
            need(decoded == self.input, "lossless output differs")?;
        }
        let samples = words(&decoded);
        let output_codes = if self.embedding == "scaled16" {
            samples
                .iter()
                .map(|&v| grid(v, self.bits))
                .collect::<Vec<_>>()
        } else {
            need(
                samples.iter().all(|&v| u32::from(v) < (1u32 << self.bits)),
                "decoded value exceeds declared range",
            )?;
            samples
        };
        if quality.is_none() {
            need(
                output_codes == self.reference,
                "lossless grid inverse differs",
            )?;
        }
        let path = self.directory.join(format!("{name}.imi"));
        fs::write(&path, &file)?;
        Ok(
            json!({"case_id":name,"image":self.image,"bits":self.bits,"embedding":self.embedding,
            "lossless":quality.is_none(),"quality":quality,"color":color_name,"effort":10,
            "threads":1,"entropy":"runlevel for lossy, default lossless","compact_streams":quality.is_some(),
            "width":SIDE,"height":SIDE,"channels":3,"alpha":"not present",
            "input_sha256":self.input_hash,"reference_codes_sha256":hash(&bytes(self.reference))?,
            "encoded":path,"encoded_bytes":file.len(),"bpp":file.len() as f64*8.0/(SIDE*SIDE) as f64,
            "encoded_sha256":hash(&file)?,"decoded_sha256":hash(&decoded)?,
            "decoded_codes_sha256":hash(&bytes(&output_codes))?,"error_code_grid":errors(self.reference,&output_codes)}),
        )
    }
}

fn run(input: &Path, output: &Path) -> Result<()> {
    need(!output.exists(), "output directory must be new")?;
    fs::create_dir_all(output)?;
    let protocol = json!({"version":1,"sources":SOURCES,"crop":"centered512x512, floor offsets, no resize or orientation change",
        "bits":[8,9,10,11,12,13,14,15,16],"grid":"nearest-even integer source16*maxN/65535; original16 unchanged",
        "embeddings":["right: U16 storage, declared N bits","scaled16: nearest-even codeN*65535/maxN, declared16bits"],
        "qualities":[30,60,90,100,116],"colors":["xyb","ycocg"],"lossless_both_embeddings":true,
        "effort":10,"threads":1,"lossy_entropy":"runlevel","compact_streams":true,
        "metrics":"MAE/RMSE/maxabs and exactness in source N-bit PQ codegrid; not perceptual metrics or BD-rate",
        "metadata":"Source cICP retained exactly; other source metadata recorded in source receipts, not copied into test files",
        "timing":false,"selection":"four existing exposed HDR photos; controlled regrids are derived data, not native camera precisions"});
    save(&output.join("protocol.json"), &protocol)?;
    let mut all_rows = Vec::new();
    let mut source_records = Vec::new();
    for (image, expected) in SOURCES {
        let path = input.join(format!("{image}.hdr.png"));
        let png = fs::read(&path)?;
        need(hash(&png)? == expected, "source PNG changed")?;
        let decoded = zenpng::decode(
            &png,
            &zenpng::PngDecodeConfig::default(),
            &enough::Unstoppable,
        )?;
        need(
            decoded.info.bit_depth == 16 && decoded.info.color_type == 2 && !decoded.info.has_alpha,
            "source must be native RGB16 without alpha",
        )?;
        let (width, height) = (decoded.info.width as usize, decoded.info.height as usize);
        need(
            width >= SIDE && height >= SIDE,
            "source smaller than declared crop",
        )?;
        let c = decoded.info.cicp.ok_or("source cICP missing")?;
        need(
            c.transfer_characteristics == 16 && c.matrix_coefficients == 0 && c.full_range,
            "source is not full-range RGB PQ",
        )?;
        let cicp = Cicp {
            color_primaries: c.color_primaries,
            transfer_characteristics: c.transfer_characteristics,
            matrix_coefficients: c.matrix_coefficients,
            full_range: c.full_range,
        };
        let packed = decoded.pixels.copy_to_contiguous_bytes();
        need(
            packed.len() == width * height * 6,
            "unexpected native pixel layout",
        )?;
        let (x, y) = ((width - SIDE) / 2, (height - SIDE) / 2);
        let mut crop = Vec::with_capacity(SIDE * SIDE * 6);
        for row in y..y + SIDE {
            crop.extend_from_slice(&packed[(row * width + x) * 6..(row * width + x + SIDE) * 6]);
        }
        let original = words(&crop);
        let folder = output.join(image);
        fs::create_dir(&folder)?;
        fs::write(folder.join("source16.rgb16ne"), &crop)?;
        let record = json!({"image":image,"path":path,"png_sha256":expected,"width":width,"height":height,
            "png_bit_depth":16,"png_color_type":2,"has_alpha":false,"crop":{"x":x,"y":y,"width":SIDE,"height":SIDE},
            "source16_sha256":hash(&crop)?,"storage":"RGB U16 native-endian","cicp":[c.color_primaries,c.transfer_characteristics,c.matrix_coefficients,u8::from(c.full_range)],
            "source_gamma":decoded.info.source_gamma,"sbit":format!("{:?}",decoded.info.significant_bits),
            "content_light_level":format!("{:?}",decoded.info.content_light_level),
            "icc_sha256":decoded.info.icc_profile.as_deref().map(hash).transpose()?,
            "exif_sha256":decoded.info.exif.as_deref().map(hash).transpose()?,
            "xmp_sha256":decoded.info.xmp.as_deref().map(hash).transpose()?});
        save(&folder.join("source.json"), &record)?;
        source_records.push(record);
        for bits in 8..=16u8 {
            let codes: Vec<u16> = original.iter().map(|&v| grid(v, bits)).collect();
            let expanded: Vec<u16> = codes.iter().map(|&v| scaled(v, bits)).collect();
            need(
                expanded
                    .iter()
                    .map(|&v| grid(v, bits))
                    .eq(codes.iter().copied()),
                "embedding not invertible",
            )?;
            if bits == 16 {
                need(codes == original, "16bit control changed")?;
            }
            for (embedding, values, declared) in
                [("right", &codes, bits), ("scaled16", &expanded, 16)]
            {
                let input = bytes(values);
                let input_hash = hash(&input)?;
                fs::write(folder.join(format!("n{bits}-{embedding}.rgb16ne")), &input)?;
                let mut info =
                    ImageInfo::new(SIDE as u32, SIDE as u32, Channels::Rgb, SampleType::U16)
                        .with_significant_bits(&[declared; 3])?;
                info.cicp = cicp;
                let cell = Cell {
                    image,
                    bits,
                    embedding,
                    info: &info,
                    input: &input,
                    reference: &codes,
                    input_hash: &input_hash,
                    directory: &folder,
                };
                let mut rows = vec![cell.run(None, LossyColor::Xyb)?];
                for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                    for quality in [30.0, 60.0, 90.0, 100.0, 116.0] {
                        rows.push(cell.run(Some(quality), color)?);
                    }
                }
                for row in &mut rows {
                    row["controlled_regrid_error_original16"] = errors(&original, &expanded);
                }
                all_rows.extend(rows);
            }
            save(&output.join("results.json"), &json!(all_rows))?;
            eprintln!("{image} bits{bits} complete: {} cells", all_rows.len());
        }
    }
    save(&output.join("sources.json"), &json!(source_records))?;
    need(all_rows.len() == 792, "incomplete grid")?;
    save(
        &output.join("complete.json"),
        &json!({"rows":all_rows.len(),"lossy":720,"lossless":72,
        "results_sha256":hash(&fs::read(output.join("results.json"))?)?}),
    )?;
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    need(
        args.len() == 3,
        "usage: hdr_precision_probe SOURCE_DIRECTORY NEW_OUTPUT_DIRECTORY",
    )?;
    run(Path::new(&args[1]), Path::new(&args[2]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_even_ties_and_all_controlled_grids() {
        assert_eq!(round_even(1, 2), 0);
        assert_eq!(round_even(3, 2), 2);
        assert_eq!(round_even(5, 2), 2);
        assert_eq!(round_even(7, 2), 4);
        for bits in 1..=16 {
            for code in 0..(1u32 << bits) {
                assert_eq!(grid(scaled(code as u16, bits), bits), code as u16);
            }
        }
        for code in 0..=u16::MAX {
            assert_eq!(grid(code, 16), code);
        }
    }
}
