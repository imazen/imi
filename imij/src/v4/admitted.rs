//! Shared request accounting when an existing consumer admits a V4 image.
use crate::container::File;
use crate::parse::limits::Admission;
use crate::{Error, Limits};

pub(crate) fn parse<'a>(
    data: &'a [u8],
    admission: &impl Admission,
    verify: bool,
    limits: Option<Limits>,
) -> Result<File<'a>, Error> {
    parse_with_policy(data, admission, verify, limits, false)
}

pub(crate) fn parse_structure<'a>(
    data: &'a [u8],
    admission: &impl Admission,
    verify: bool,
) -> Result<File<'a>, Error> {
    parse_with_policy(data, admission, verify, None, true)
}

fn parse_with_policy<'a>(
    data: &'a [u8],
    admission: &impl Admission,
    verify: bool,
    limits: Option<Limits>,
    structure: bool,
) -> Result<File<'a>, Error> {
    admission.check()?;
    let mut resume = admission.v4_resume();
    resume.nested_policy(verify, limits);
    if structure {
        resume.for_structure();
    }
    let session = resume.session(admission.v4_cancel());
    let result = (|| {
        // Existing complete-input consumers own transport accounting. Parsing
        // still charges nested headers, models, units, work and cancellation.
        let _input = session.v4_precharged()?;
        super::file::parse_file(data, &session)
    })();
    let committed = admission.v4_commit(session.v4_resume());
    result.and_then(|file| committed.map(|_| file))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_frame(
    file: &File<'_>,
    output: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    inherited: Option<&crate::ImageInfo>,
    admission: &impl Admission,
    check: &(impl Fn() -> Result<(), Error> + Sync),
) -> Result<crate::ImageInfo, Error> {
    check()?;
    let resume = admission.v4_resume();
    let cancel = admission.v4_cancel();
    let session = resume.session(|| cancel() || check().is_err());
    let result = (|| {
        let _input = session.v4_precharged()?;
        let mut info = file.header.info.clone();
        if file.header.coding == crate::Coding::Lossy
            && let Some(parent) = inherited
        {
            info.cicp = parent.cicp;
            info.alpha_mode = parent.alpha_mode;
        }
        session.v4_output(&info)?;
        let required = info
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("frame output stride/extent"))?;
        if output.len() < required {
            return Err(Error::InvalidInput("frame output stride/extent"));
        }
        // FrameDecoder owns the outer sequence's controls and bitmap table.
        // This view validates/reconstructs exactly the selected image's objects.
        let image = super::decode::ImageFile::from_parsed(
            file.image_view(),
            file.encoded_len,
            &session,
            inherited,
        )?;
        if !inter {
            super::decode::run_file(image, output, stride, &session, None, threads)?;
        } else {
            image.visit(|event| {
                let work = match event {
                    super::decode::ImageEvent::Group(work) => work,
                    super::decode::ImageEvent::Tile(tile) => {
                        tile.verify(&info, output, stride, &session)?;
                        return Ok(());
                    }
                };
                let rect = work.rect()?;
                let at = (rect.y() as usize)
                    .checked_mul(stride)
                    .and_then(|n| n.checked_add(rect.x() as usize * info.bytes_per_pixel()))
                    .ok_or(Error::TooLarge)?;
                work.decode_reference(&info, &mut output[at..], stride, &session)
            })?;
        }
        // The frame driver handles whole-canvas checksums after compositing;
        // a nested image's own checksum is still checked here, as in V2/V3.
        file.pixelsum
            .verify_checked(0, &info, output, stride, || session.adapter_work(64 * 1024))?;
        session.adapter_work(0)?;
        Ok(info)
    })();
    let committed = admission.v4_commit(session.v4_resume());
    result.and_then(|info| committed.map(|_| info))
}
