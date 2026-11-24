//! Application-mediated access to held-out datasets.
use crate::{Value, wire::bad};
use std::io;

pub(crate) const PROFILE: &str = "imij.access-profile";
pub(crate) const POLICY: &str = "imij.split-policy";
pub(crate) const PLAINTEXT: u64 = 1;
pub(crate) const LEAF_KIND: u8 = 7;

pub(crate) fn declaration(metadata: &crate::EntryMetadata) -> io::Result<Option<(u64, &str)>> {
    match (
        metadata.attributes.get(PROFILE),
        metadata.attributes.get(POLICY),
    ) {
        (None, None) => Ok(None),
        (Some(Value::Unsigned(profile)), Some(Value::Text(policy)))
            if *profile != 0 && !policy.trim().is_empty() =>
        {
            Ok(Some((*profile, policy)))
        }
        _ => Err(bad("invalid governed scope declaration")),
    }
}

pub(crate) fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "governed scope requires audited access",
    )
}

/// Declared use of held-out material.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluationPurpose {
    FinalEvaluation,
    Inspection,
    TrainingOrTuningOverride,
}

/// Caller-supplied run identity. The host verifies these claims when approving.
#[derive(Clone, Copy, Debug)]
pub struct EvaluationRun<'a> {
    pub actor: &'a str,
    pub run_id: &'a str,
    pub frozen_revision: &'a str,
    pub reason: &'a str,
    pub purpose: EvaluationPurpose,
}

/// Exact scope, split policy and run presented to the approver and audit sink.
#[derive(Debug)]
#[non_exhaustive]
pub struct ScopeAccess<'a> {
    pub run: EvaluationRun<'a>,
    pub entry_key: u64,
    pub entry_digest: crate::Digest,
    pub archive_generation: u64,
    pub split_policy: &'a str,
}

/// The authorization outcome. Only a final evaluation can qualify for
/// held-out reporting. A grant records authorization before payload delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessDecision<'a> {
    Denied,
    Granted { approval_id: &'a str },
}

/// Integration contract for a cooperating application. Implementations bind
/// approval and durable log records to the supplied scope and run. A hosted
/// implementation can use a private range origin and a remote audit service.
pub trait HeldOutAccess {
    fn attest_that_encoded_images_and_decoded_pixels_stay_only_in_memory(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<()>;

    fn attest_that_human_ratings_and_other_associated_scores_stay_only_in_memory(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<()>;

    /// Return the interactive approval identifier. The approver must see the
    /// exact split policy and purpose. Overrides require explicit permission.
    fn obtain_interactive_user_approval_for_this_held_out_run(
        &mut self,
        access: &ScopeAccess<'_>,
    ) -> io::Result<String>;

    /// Append the policy, run, outcome and approval identity to a durable log.
    /// A successful return acknowledges the durable append. Return its receipt
    /// ID for a grant. Retried requests should reuse the same audit record.
    fn append_scope_access_decision_to_the_durable_audit_log(
        &mut self,
        access: &ScopeAccess<'_>,
        decision: AccessDecision<'_>,
    ) -> io::Result<String>;
}
