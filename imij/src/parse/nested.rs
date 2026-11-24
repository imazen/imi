//! Complete files a decoder holds: the outer file and its nested ones
//! (frames and pages after the first). One policy parses them all (SPEC
//! 1.4 "Nested files").

use crate::container::{self, File};
use crate::parse::limits::Unrestricted;
use crate::{Error, Limits};

/// How a decoder parses a complete file. With `verify`, the file's own
/// `cSUM` table is checked when it carries one, whether or not the outer
/// file has one; without, nested and outer tables are skipped alike.
/// `limits`, when given, are checked right after each file's header.
#[derive(Clone, Copy)]
pub(crate) struct NestedPolicy {
    pub(crate) verify: bool,
    pub(crate) limits: Option<Limits>,
}

impl NestedPolicy {
    pub(crate) fn parse_admitted<'a>(
        &self,
        file: &'a [u8],
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<File<'a>, Error> {
        if crate::parse::header::header_version(file).ok() == Some(4) {
            return crate::v4::admitted::parse(file, admission, self.verify, self.limits);
        }
        if let Some(limits) = &self.limits {
            let header = crate::parse::header::read_versioned(
                &mut crate::ByteReader::new(file),
                admission.v4_resume().header_limit(),
            )?;
            crate::parse::limits::check_limits(&header.header.info, limits)?;
        }
        let length = |h: &crate::Header, d, body: &'a [u8]| {
            crate::v1::payload_len_checked(h, d, body, admission)
        };
        container::parse_checked(
            file,
            false,
            &length,
            admission,
            crate::checksum::Verify::from_flag(self.verify, 1),
        )
    }

    pub(crate) fn parse_decodable_admitted<'a>(
        &self,
        file: &'a [u8],
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<File<'a>, Error> {
        let hp = crate::parse::header::read_versioned(
            &mut crate::ByteReader::new(file),
            admission.v4_resume().header_limit(),
        )?;
        crate::parse::header::check_decodable(&hp.header, hp.declared)?;
        self.parse_admitted(file, admission)
    }
    /// Parses `file`: header, chunks (checksum per the policy) and `DATA`
    /// framing.
    pub(crate) fn parse<'a>(&self, file: &'a [u8]) -> Result<File<'a>, Error> {
        self.parse_admitted(file, &Unrestricted)
    }

    /// [`Self::parse`] after checking that this build decodes what the
    /// header declares (SPEC 0: from the header, before the chunks).
    pub(crate) fn parse_decodable<'a>(&self, file: &'a [u8]) -> Result<File<'a>, Error> {
        let hp = crate::parse::header::read_versioned(&mut crate::ByteReader::new(file), u64::MAX)?;
        crate::parse::header::check_decodable(&hp.header, hp.declared)?;
        self.parse(file)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, EncodeConfig, ImageInfo, Metadata, SampleType};

    fn file(checksum: bool) -> alloc::vec::Vec<u8> {
        let info = ImageInfo::new(6, 5, Channels::Rgb, SampleType::U8);
        let px: alloc::vec::Vec<u8> = (0..info.row_bytes() * 5).map(|i| i as u8).collect();
        let f = crate::encode(
            &px,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &EncodeConfig::default(),
        )
        .unwrap();
        if checksum {
            crate::checksum::finish_file(f, Some(crate::HashAlgorithm::DEFAULT)).unwrap()
        } else {
            f
        }
    }

    #[test]
    fn nested_policy_checks_each_files_own_table_and_limits() {
        let plain = file(false);
        let summed = file(true);
        assert!(summed.len() > plain.len(), "finish_file added a cSUM chunk");
        // Damage a covered byte (the last one, in DATA).
        let mut bad = summed.clone();
        let (at, _) = {
            let f = container::parse(&summed, false, &crate::v1::payload_len).unwrap();
            let start = f.data.as_ptr() as usize - summed.as_ptr() as usize;
            (start + f.data.len() - 1, ())
        };
        bad[at] ^= 1;
        let on = NestedPolicy {
            verify: true,
            limits: None,
        };
        let off = NestedPolicy {
            verify: false,
            limits: None,
        };
        assert!(on.parse(&plain).is_ok());
        assert!(on.parse(&summed).is_ok());
        assert_eq!(on.parse(&bad).err(), Some(crate::checksum::MISMATCH));
        // Without verification the table is skipped like an unknown chunk.
        assert!(off.parse(&bad).is_ok());
        // Limits apply right after the header, before the table.
        let tight = NestedPolicy {
            verify: true,
            limits: Some(Limits {
                max_pixels: 29,
                max_bytes: u64::MAX,
                verify_checksum: true,
            }),
        };
        assert_eq!(tight.parse(&bad).err(), Some(Error::TooLarge));
        let roomy = NestedPolicy {
            verify: true,
            limits: Some(Limits {
                max_pixels: 30,
                max_bytes: u64::MAX,
                verify_checksum: true,
            }),
        };
        assert!(roomy.parse(&summed).is_ok());
        assert!(roomy.parse_decodable(&summed).is_ok());
    }
}
