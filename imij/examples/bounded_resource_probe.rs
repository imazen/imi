//! Fresh-process heap/RSS probe; no timing or in-process parity measurement.
//! bounded_resource_probe FILE WORKING_BUDGET_BYTES legacy|bounded
use imij::decode_resources::{DecodePolicy, DecodeSession, DecodeUsage};
use imij::{Error, ImageInfo};
use std::{env, fs};

fn main() -> Result<(), Error> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 4 {
        return Err(Error::InvalidInput(
            "expected FILE WORKING_BUDGET_BYTES legacy|bounded",
        ));
    }
    let budget: u64 = args[2]
        .parse()
        .map_err(|_| Error::InvalidInput("invalid working budget"))?;
    let file = fs::read(&args[1]).map_err(|_| Error::Read)?;
    let (info, output, usage): (ImageInfo, Vec<u8>, DecodeUsage) = match args[3].as_str() {
        "legacy" => {
            let (info, output) = imij::decode(&file)?;
            (info, output, DecodeUsage::default())
        }
        "bounded" => {
            let mut session =
                DecodeSession::new(DecodePolicy::default().with_max_working_bytes(budget));
            let (info, output) = session.decode(&file)?;
            (info, output, session.usage())
        }
        _ => return Err(Error::InvalidInput("mode must be legacy or bounded")),
    };
    let hash = output.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    println!(
        "mode={} width={} height={} input_bytes={} output_bytes={} output_fnv1a64={hash:016x} working_budget={} peak_planned_working_bytes={} charged_input_bytes={} chunks={} metadata_bytes={} units={} work_units={}",
        args[3],
        info.width,
        info.height,
        file.len(),
        output.len(),
        budget,
        usage.peak_planned_working_bytes,
        usage.input_bytes,
        usage.chunks,
        usage.metadata_bytes,
        usage.units,
        usage.work_units
    );
    Ok(())
}
