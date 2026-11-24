use imij::planes::{ReadAt, SliceReader};
use imij_set::{
    AccessDecision, EntryMetadata, EvaluationPurpose, EvaluationRun, HeldOutAccess, Reader,
    ScopeAccess, Summary, Value, Writer,
};
use std::{cell::Cell, io, rc::Rc};

const POLICY: &str =
    "CSIQ revision 2, test. Final evaluation only unless the owner approves an override.";
const RUN: EvaluationRun<'static> = EvaluationRun {
    actor: "reviewer",
    run_id: "run-1",
    frozen_revision: "frozen-model",
    reason: "measure held-out agreement",
    purpose: EvaluationPurpose::FinalEvaluation,
};
const PIXELS: &[u8] = &[13, 31, 201, 70, 40, 91];
fn encoded_image() -> Vec<u8> {
    let info = imij::ImageInfo::new(2, 1, imij::Channels::Rgb, imij::SampleType::U8);
    imij::ImiEncoderConfig::lossless()
        .encode_samples(
            imij::ImageRef::new(PIXELS, 6, &info),
            &imij::Metadata::default(),
        )
        .unwrap()
}
const SCORES: &[u8] = br#"{"human":0.75,"metric":0.8}"#;

fn nested() -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    writer
        .insert_stream(1, 0, Summary::default(), |out| {
            out.write_all(&encoded_image())
        })
        .unwrap();
    writer
        .insert_stream(2, 0, Summary::default(), |out| out.write_all(SCORES))
        .unwrap();
    writer.finish().unwrap().0
}

fn archive(governed: bool) -> Vec<u8> {
    let scope = nested();
    let mut writer = Writer::new(Vec::new());
    if governed {
        writer
            .insert_governed_scope(1, POLICY, |out| out.write_all(&scope))
            .unwrap();
    } else {
        writer
            .insert_stream(1, 0, Summary::default(), |out| out.write_all(&scope))
            .unwrap();
    }
    writer
        .insert_stream(2, 0, Summary::default(), |out| out.write_all(b"public"))
        .unwrap();
    writer
        .insert_stream(3, 0, Summary::default(), |out| out.write_all(&[0; 65536]))
        .unwrap();
    writer.finish().unwrap().0
}

struct Gate {
    authorized: Rc<Cell<bool>>,
    order: Vec<&'static str>,
    fail: Option<&'static str>,
    purpose: EvaluationPurpose,
}

impl Gate {
    fn step(&mut self, name: &'static str, access: &ScopeAccess<'_>) -> io::Result<()> {
        assert_eq!(access.split_policy, POLICY);
        assert_eq!(access.run.run_id, RUN.run_id);
        assert_eq!(access.run.frozen_revision, RUN.frozen_revision);
        assert_eq!(access.run.reason, RUN.reason);
        assert_eq!(access.run.purpose, self.purpose);
        assert_eq!(access.entry_key, 1);
        assert_eq!(access.archive_generation, 1);
        assert_eq!(
            access.entry_digest,
            imij_set::HashAlgorithm::DEFAULT.digest(&nested())
        );
        assert!(!self.authorized.get());
        self.order.push(name);
        if self.fail == Some(name) {
            return Err(io::Error::other(name));
        }
        Ok(())
    }
}

impl HeldOutAccess for Gate {
    fn attest_that_encoded_images_and_decoded_pixels_stay_only_in_memory(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<()> {
        self.step("images", access)
    }

    fn attest_that_human_ratings_and_other_associated_scores_stay_only_in_memory(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<()> {
        self.step("scores", access)
    }

    fn obtain_interactive_user_approval_for_this_held_out_run(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<String> {
        self.step("approval", access)?;
        Ok("interactive-approval-1".into())
    }

    fn append_scope_access_decision_to_the_durable_audit_log(
        &mut self,
        access: &ScopeAccess<'_>,
        decision: AccessDecision<'_>,
    ) -> io::Result<String> {
        match decision {
            AccessDecision::Denied => {
                self.step("denied", access)?;
                Ok(String::new())
            }
            AccessDecision::Granted { approval_id } => {
                assert_eq!(approval_id, "interactive-approval-1");
                self.step("grant", access)?;
                self.authorized.set(true);
                Ok("receipt-1".into())
            }
        }
    }
}

// The outer directory is distant from the scope. Every read touching the
// scope must wait for the acknowledged audit grant.
struct WatchedSource<'a> {
    bytes: SliceReader<'a>,
    scope_end: u64,
    authorized: Rc<Cell<bool>>,
}

impl ReadAt for WatchedSource<'_> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        self.bytes.len()
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        if offset < self.scope_end && !out.is_empty() {
            assert!(self.authorized.get(), "scope was read before audit");
        }
        self.bytes.read_exact_at(offset, out)
    }
}

#[test]
fn images_and_scores_require_both_promises_approval_and_audit() {
    let bytes = archive(true);
    for (fail, expected) in [
        (Some("images"), &["images", "denied"][..]),
        (Some("scores"), &["images", "scores", "denied"][..]),
        (
            Some("approval"),
            &["images", "scores", "approval", "denied"][..],
        ),
        (
            Some("grant"),
            &["images", "scores", "approval", "grant"][..],
        ),
        (None, &["images", "scores", "approval", "grant"][..]),
    ] {
        let authorized = Rc::new(Cell::new(false));
        let mut reader = Reader::open(WatchedSource {
            bytes: SliceReader::new(&bytes),
            scope_end: nested().len() as u64,
            authorized: Rc::clone(&authorized),
        })
        .unwrap();
        let mut gate = Gate {
            fail,
            authorized,
            order: Vec::new(),
            purpose: RUN.purpose,
        };
        let result = reader.open_governed_scope(1, RUN, &mut gate);
        assert_eq!(&gate.order, expected);
        if fail.is_some() {
            assert!(result.is_err());
            continue;
        }
        let mut scope = result.unwrap();
        assert_eq!(scope.audit_receipt(), Some("receipt-1"));
        let image = scope.get(1).unwrap().unwrap();
        let scores = scope.get(2).unwrap().unwrap();
        assert_eq!(
            scope.read_entry_bounded(&image, 1024).unwrap(),
            encoded_image()
        );
        {
            let source = scope.entry_source(&image).unwrap();
            let mut region = imij::region::RegionReader::open(source).unwrap();
            let mut pixels = [0; 6];
            region
                .decode_region_threads(imij::planes::Rect::new(0, 0, 2, 1), &mut pixels, 6, 1)
                .unwrap();
            assert_eq!(pixels, PIXELS);
        }
        assert_eq!(scope.read_entry_bounded(&scores, 100).unwrap(), SCORES);
        let mut out = [0; 5];
        scope
            .entry_source(&scores)
            .unwrap()
            .read_exact_at(0, &mut out)
            .unwrap();
        assert_eq!(&out, &SCORES[..5]);
        let mut exported = Vec::new();
        assert_eq!(
            scope.copy_to(&scores, &mut exported).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(exported.is_empty());
    }
}

#[test]
fn protected_payloads_cannot_use_public_entry_paths_or_foreign_descriptors() {
    let bytes = archive(true);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let scope = reader.get(1).unwrap().unwrap();
    assert_eq!(scope.access_policy(), Some((1, POLICY)));
    let entries = reader.get_many(&[1, 2]).unwrap();
    assert_eq!(entries[0].as_ref(), Some(&scope));
    let from_listing = reader
        .listing()
        .unwrap()
        .next(&mut reader)
        .unwrap()
        .unwrap();
    assert_eq!(from_listing, scope);
    let mut resumed = reader.listing_from(1).unwrap();
    let resumed_scope = resumed.next(&mut reader).unwrap().unwrap();
    assert_eq!(resumed_scope, scope);
    assert!(reader.entry_source(&resumed_scope).is_err());
    assert!(reader.entry_source(&scope).is_err());
    assert_eq!(
        reader
            .read_entry_bounded(&scope, usize::MAX)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    let mut exported = Vec::new();
    assert_eq!(
        reader.copy_to(&scope, &mut exported).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(exported.is_empty());
    let plain = archive(false);
    let mut other = Reader::open(SliceReader::new(&plain)).unwrap();
    let foreign = other.get(1).unwrap().unwrap();
    assert_eq!(foreign.offset(), scope.offset());
    assert_eq!(foreign.checksum(), scope.checksum());
    assert!(reader.entry_source(&foreign).is_err());
    let same_archive = Reader::open(SliceReader::new(&bytes)).unwrap();
    let public = reader.get(2).unwrap().unwrap();
    assert_eq!(reader.read_entry_bounded(&public, 20).unwrap(), b"public");
    let mut reopened = same_archive;
    assert_eq!(reopened.read_entry_bounded(&public, 20).unwrap(), b"public");
}

#[test]
fn unknown_profiles_remain_discoverable_and_fail_closed() {
    let mut writer = Writer::new(Vec::new());
    let mut metadata = EntryMetadata::default();
    metadata
        .attributes
        .insert("imij.access-profile".into(), Value::Unsigned(42));
    metadata
        .attributes
        .insert("imij.split-policy".into(), Value::Text(POLICY.into()));
    writer
        .insert_stream_with_metadata(1, 0, Summary::default(), &metadata, |out| {
            out.write_all(&nested())
        })
        .unwrap();
    let bytes = writer.finish().unwrap().0;
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let entry = reader.get(1).unwrap().unwrap();
    assert_eq!(entry.access_policy(), Some((42, POLICY)));
    let mut gate = Gate {
        authorized: Rc::new(Cell::new(false)),
        order: Vec::new(),
        fail: None,
        purpose: RUN.purpose,
    };
    let error = reader.open_governed_scope(1, RUN, &mut gate).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(gate.order.is_empty());
    assert!(reader.entry_source(&entry).is_err());
}

#[test]
fn training_override_is_present_at_approval_and_grant() {
    let bytes = archive(true);
    let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
    let run = EvaluationRun {
        purpose: EvaluationPurpose::TrainingOrTuningOverride,
        ..RUN
    };
    let mut gate = Gate {
        authorized: Rc::new(Cell::new(false)),
        order: Vec::new(),
        fail: None,
        purpose: run.purpose,
    };
    let scope = reader.open_governed_scope(1, run, &mut gate).unwrap();
    assert_eq!(scope.audit_receipt(), Some("receipt-1"));
    assert_eq!(gate.order, ["images", "scores", "approval", "grant"]);
}
