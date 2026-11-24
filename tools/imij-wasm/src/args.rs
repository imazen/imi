//! `encode` argument parsing. Unknown or malformed options are errors; the
//! encoder's own validation reports invalid combinations.

use imij::{
    ChannelSampling, Channels, Cicp, DecodeSpeed, ImiEncoderConfig, LosslessTuning,
    LossyChannelLayout, LossyColor, LossyEntropy, LossyTransform, LossyTuning, SampleType,
};

pub struct RawInput {
    pub w: u32,
    pub h: u32,
    pub channels: Channels,
    pub sample: SampleType,
}

pub enum Expand {
    Shift,
    Replicate,
    Scale,
}

pub struct EncodeArgs {
    pub raw: Option<RawInput>,
    pub channels: Option<Channels>,
    pub sample: Option<SampleType>,
    pub expand: Expand,
    pub sbits: Option<Vec<u8>>,
    pub premultiplied: bool,
    pub cicp: Option<Cicp>,
    /// Center crop to at most this width and height, before conversion.
    pub crop: Option<(u32, u32)>,
    /// The encoder: quick, lossless or lossy, with every option applied.
    pub mode: ImiEncoderConfig,
}

fn channels(s: &str) -> Result<Channels, String> {
    match s {
        "gray" => Ok(Channels::Gray),
        "ga" => Ok(Channels::GrayAlpha),
        "rgb" => Ok(Channels::Rgb),
        "rgba" => Ok(Channels::Rgba),
        _ => Err(format!("bad channels {s:?} (gray|ga|rgb|rgba)")),
    }
}

fn sample(s: &str) -> Result<SampleType, String> {
    match s {
        "u8" => Ok(SampleType::U8),
        "u16" => Ok(SampleType::U16),
        "f16" => Ok(SampleType::F16),
        "u32" => Ok(SampleType::U32),
        "i32" => Ok(SampleType::I32),
        "f32" => Ok(SampleType::F32),
        "i8" => Ok(SampleType::I8),
        "i16" => Ok(SampleType::I16),
        "u64" => Ok(SampleType::U64),
        "i64" => Ok(SampleType::I64),
        "f64" => Ok(SampleType::F64),
        _ => Err(format!(
            "bad sample {s:?} (u8|u16|f16|u32|i32|f32|i8|i16|u64|i64|f64)"
        )),
    }
}

fn f32v(s: &str, name: &str) -> Result<f32, String> {
    s.parse::<f32>()
        .map_err(|_| format!("bad {name} {s:?} (expected a number)"))
}

fn u32v(s: &str, name: &str) -> Result<u32, String> {
    s.parse::<u32>()
        .map_err(|_| format!("bad {name} {s:?} (expected an integer)"))
}

fn onoff(s: &str, name: &str) -> Result<bool, String> {
    match s {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        _ => Err(format!("bad {name} {s:?} (on|off)")),
    }
}

fn parse_column_groups(s: &str) -> Result<Option<u32>, String> {
    match s {
        "auto" => Ok(None),
        "none" | "0" => Ok(Some(0)),
        _ => Ok(Some(u32v(s, "column-groups")?)),
    }
}

fn layout(s: &str) -> Result<LossyChannelLayout, String> {
    let (t, s_) = match s.split_once(',') {
        Some((t, s)) => (t, s),
        None => (s, "full"),
    };
    let transform = match t {
        "dct8" => LossyTransform::Dct8,
        "dct4" => LossyTransform::Dct4,
        "dct2" => LossyTransform::Dct2,
        "adaptive4" => LossyTransform::Adaptive4,
        _ => {
            return Err(format!(
                "bad layout transform {t:?} (dct8|dct4|dct2|adaptive4)"
            ));
        }
    };
    let sampling = match s_ {
        "full" => ChannelSampling::Full,
        "h" => ChannelSampling::HalfHorizontal,
        "v" => ChannelSampling::HalfVertical,
        "both" => ChannelSampling::HalfBoth,
        _ => return Err(format!("bad layout sampling {s_:?} (full|h|v|both)")),
    };
    Ok(LossyChannelLayout::new(transform, sampling))
}

impl EncodeArgs {
    pub fn parse(argv: &[String]) -> Result<Self, String> {
        let mut raw = None;
        let mut ch = None;
        let mut smp = None;
        let mut expand = Expand::Scale;
        let mut sbits = None;
        let mut premultiplied = false;
        let mut cicp = None;
        let mut crop = None;

        let mut ll = LosslessTuning::default();
        let mut ly = LossyTuning::default();
        let mut effort = None;
        let mut column_groups = None;
        let mut unit_ordinals = false;
        let mut quality = None;
        let mut decode_speed = None;
        let mut seen_lossless = false;
        let mut seen_lossy = false;
        let mut seen_quick = false;
        let mut quick_bits = imij::QuickBitSmarts::Off;

        let mut it = argv.iter();
        macro_rules! val {
            ($name:literal) => {
                it.next()
                    .ok_or_else(|| concat!("missing value for ", $name))?
                    .as_str()
            };
        }
        while let Some(a) = it.next() {
            match a.as_str() {
                "--raw" => {
                    let w = u32v(val!("--raw W"), "raw width")?;
                    let h = u32v(val!("--raw H"), "raw height")?;
                    let c = channels(val!("--raw CHANNELS"))?;
                    let s = sample(val!("--raw SAMPLE"))?;
                    raw = Some(RawInput {
                        w,
                        h,
                        channels: c,
                        sample: s,
                    });
                }
                "--channels" => ch = Some(channels(val!("--channels"))?),
                "--sample" => smp = Some(sample(val!("--sample"))?),
                "--expand" => {
                    expand = match val!("--expand") {
                        "shift" => Expand::Shift,
                        "replicate" => Expand::Replicate,
                        "scale" => Expand::Scale,
                        e => return Err(format!("bad expand {e:?} (shift|replicate|scale)")),
                    };
                }
                "--sbits" => {
                    sbits = Some(
                        val!("--sbits")
                            .split(',')
                            .map(|n| {
                                n.trim()
                                    .parse::<u8>()
                                    .map_err(|_| format!("bad --sbits entry {n:?}"))
                            })
                            .collect::<Result<Vec<u8>, _>>()?,
                    );
                }
                "--premultiplied" => premultiplied = true,
                "--crop" => {
                    let v = val!("--crop");
                    let (w, h) = v
                        .split_once('x')
                        .ok_or_else(|| format!("bad --crop {v:?} (WxH)"))?;
                    let (w, h) = (u32v(w, "crop width")?, u32v(h, "crop height")?);
                    if w == 0 || h == 0 {
                        return Err("crop dimensions must be nonzero".into());
                    }
                    crop = Some((w, h));
                }
                "--cicp" => {
                    let v = val!("--cicp");
                    let p: Vec<&str> = v.split(',').collect();
                    if p.len() < 2 || p.len() > 3 {
                        return Err(format!("bad --cicp {v:?} (P,T[,full_range 0|1])"));
                    }
                    cicp = Some(Cicp {
                        color_primaries: u32v(p[0], "cicp primaries")? as u8,
                        transfer_characteristics: u32v(p[1], "cicp transfer")? as u8,
                        matrix_coefficients: 0,
                        full_range: match p.get(2) {
                            Some(r) => onoff(r, "cicp range")?,
                            None => true,
                        },
                    });
                }
                "--quick-bits" => {
                    quick_bits = match val!("--quick-bits") {
                        "off" => imij::QuickBitSmarts::Off,
                        "packed" => imij::QuickBitSmarts::Packed,
                        "packets" => imij::QuickBitSmarts::Packets,
                        _ => return Err("--quick-bits expects off, packed or packets".into()),
                    };
                }
                "--quick" => {
                    seen_quick = true;
                }
                "--lossless" => {
                    if seen_lossy {
                        return Err("--lossless and --lossy are exclusive".into());
                    }
                    seen_lossless = true;
                }
                "--lossy" => {
                    if seen_lossless {
                        return Err("--lossless and --lossy are exclusive".into());
                    }
                    seen_lossy = true;
                }
                // Shared.
                "--effort" => {
                    effort = Some(f32v(val!("--effort"), "effort")?);
                }
                "--column-groups" => {
                    column_groups = Some(parse_column_groups(val!("--column-groups"))?);
                }
                "--unit-ordinals" => {
                    unit_ordinals = onoff(val!("--unit-ordinals"), "unit-ordinals")?;
                }
                // Lossless-only.
                "--stripe-rows" => {
                    ll.stripe_rows = Some(u32v(val!("--stripe-rows"), "stripe-rows")?)
                }
                "--lanes" => ll.lanes = Some(u32v(val!("--lanes"), "lanes")?),
                "--palette" => ll.palette = onoff(val!("--palette"), "palette")?,
                "--decor-mask" => ll.decor_mask = u32v(val!("--decor-mask"), "decor-mask")? as u8,
                "--predictor-mask" => {
                    ll.predictor_mask = u32v(val!("--predictor-mask"), "predictor-mask")? as u8
                }
                "--max-error" => {
                    let v = val!("--max-error");
                    let parts: Vec<&str> = v.split(',').collect();
                    if parts.len() == 1 {
                        ll = ll.with_max_error(u32v(parts[0], "max-error")?);
                    } else if parts.len() <= 4 {
                        let mut errs = [0u32; 4];
                        for (i, p) in parts.iter().enumerate() {
                            errs[i] = u32v(p, "max-error")?;
                        }
                        ll = ll.with_channel_max_errors(errs);
                    } else {
                        return Err(format!("bad --max-error {v:?} (D or D,D,D[,D])"));
                    }
                }
                // Lossy-only.
                "--quality" => quality = Some(f32v(val!("--quality"), "quality")?),
                "--color" => {
                    match val!("--color") {
                        "xyb" => {
                            ly.color = LossyColor::Xyb;
                            ly.color_transform = None;
                        }
                        "ycocg" => {
                            ly.color = LossyColor::YCoCg;
                            ly.color_transform = None;
                        }
                        "identity" | "raw" | "none" => {
                            ly = ly.with_color_transform(Some(imij::ColorTransform::Identity));
                        }
                        c => return Err(format!("bad color {c:?} (xyb|ycocg|identity)")),
                    };
                }
                "--entropy" => {
                    ly.entropy = match val!("--entropy") {
                        "runlevel" => LossyEntropy::RunLevel,
                        "splithuf" => LossyEntropy::SplitHuf,
                        e => {
                            return Err(format!("bad entropy {e:?} (runlevel|splithuf)"));
                        }
                    };
                }
                "--decode-speed" => {
                    decode_speed = Some(match val!("--decode-speed") {
                        "compact" => DecodeSpeed::Compact,
                        "balanced" => DecodeSpeed::Balanced,
                        "fast" => DecodeSpeed::Fast,
                        "fastest" => DecodeSpeed::Fastest,
                        s => {
                            return Err(format!(
                                "bad decode-speed {s:?} (compact|balanced|fast|fastest)"
                            ));
                        }
                    });
                }
                "--compact-streams" => {
                    ly.compact_streams = onoff(val!("--compact-streams"), "compact-streams")?
                }
                "--stripe-block-rows" => {
                    ly.stripe_block_rows = u32v(val!("--stripe-block-rows"), "stripe-block-rows")?
                }
                "--aq-gamma" => ly.aq_gamma = f32v(val!("--aq-gamma"), "aq-gamma")?,
                "--aq-r0" => ly.aq_r0 = f32v(val!("--aq-r0"), "aq-r0")?,
                "--layout-1" => ly.channel_layouts[0] = layout(val!("--layout-1"))?,
                "--layout-2" => ly.channel_layouts[1] = layout(val!("--layout-2"))?,
                "--layout-3" => ly.channel_layouts[2] = layout(val!("--layout-3"))?,
                "--round" => ly.round = f32v(val!("--round"), "round")?,
                "--zero-bias" => {
                    ly.freq_zero_bias = onoff(val!("--zero-bias"), "zero-bias")?;
                }
                "--adaptive-partitions" => {
                    ly.adaptive_partitions =
                        onoff(val!("--adaptive-partitions"), "adaptive-partitions")?;
                }
                "--max-ac-tables" => {
                    ly.max_ac_tables = u32v(val!("--max-ac-tables"), "max-ac-tables")? as usize
                }
                "--channel-mul" => {
                    let v = val!("--channel-mul");
                    let p: Vec<&str> = v.split(',').collect();
                    if p.len() != 3 {
                        return Err(format!("bad --channel-mul {v:?} (F,F,F)"));
                    }
                    ly.channel_mul = Some([
                        f32v(p[0], "channel-mul")?,
                        f32v(p[1], "channel-mul")?,
                        f32v(p[2], "channel-mul")?,
                    ]);
                }
                "--matrix" => ly.matrix = u32v(val!("--matrix"), "matrix")? as u8,
                other => return Err(format!("unknown option {other:?}")),
            }
        }

        if seen_quick && (seen_lossless || seen_lossy) {
            return Err("--quick, --lossless and --lossy are exclusive".into());
        }
        if !seen_quick && quick_bits != imij::QuickBitSmarts::Off {
            return Err("--quick-bits requires --quick".into());
        }
        let mut mode = if seen_quick {
            ImiEncoderConfig::quick()
                .with_quick(quick_bits)
                .tune_lossless(|t| t.stripe_rows = Some(ll.stripe_rows.unwrap_or(64)))
                .with_column_groups(column_groups.flatten().unwrap_or(0))
        } else {
            match (seen_lossless, seen_lossy) {
                (true, false) => ImiEncoderConfig::lossless().with_lossless_tuning(ll),
                (false, true) => ImiEncoderConfig::lossy(quality.unwrap_or(90.0))
                    .with_decode_speed(decode_speed)
                    .with_lossy_tuning(ly),
                _ => {
                    return Err("encode needs exactly one of --quick, --lossless or --lossy".into());
                }
            }
            .with_column_groups(column_groups.flatten())
        };
        if let Some(e) = effort {
            mode = mode.with_effort(e);
        }
        mode = mode.with_unit_ordinals(unit_ordinals);
        Ok(Self {
            raw,
            channels: ch,
            sample: smp,
            expand,
            sbits,
            premultiplied,
            cicp,
            crop,
            mode,
        })
    }
}

#[cfg(test)]
mod quick_bits_tests {
    use super::*;
    #[test]
    fn policies_parse_and_invalid_combinations_fail() {
        let parse = |args: &[&str]| {
            EncodeArgs::parse(&args.iter().map(|v| v.to_string()).collect::<Vec<_>>())
        };
        for (name, expected) in [
            ("off", imij::QuickBitSmarts::Off),
            ("packed", imij::QuickBitSmarts::Packed),
            ("packets", imij::QuickBitSmarts::Packets),
        ] {
            let parsed = parse(&["--quick", "--quick-bits", name]).unwrap();
            assert!(matches!(parsed.mode.coder(), imij::Coder::Quick(b) if b == expected));
        }
        for args in [
            &["--lossless", "--quick-bits", "packed"][..],
            &["--quick", "--quick-bits", "unknown"],
            &["--quick", "--quick-bits"],
            &["--lossless", "--unit-ordinals", "maybe"],
        ] {
            assert!(parse(args).is_err());
        }
        let parsed = parse(&["--lossless", "--unit-ordinals", "on"]).unwrap();
        assert!(parsed.mode.coder().is_lossless() && parsed.mode.unit_ordinals());
        let parsed = parse(&["--lossy", "--unit-ordinals", "on"]).unwrap();
        assert!(
            matches!(parsed.mode.coder(), imij::Coder::Lossy(_)) && parsed.mode.unit_ordinals()
        );
    }
}
