//! Rendering only. Every fact printed here was decided by the SDK.

use audio_provenance_core::{ProofLevel, VerificationStatus};
use genotrace::MatchBasis;
use genotrace::{InspectReport, RecoveryStep, StepOutcome, VerifyResult};

use crate::style::{Style, Tone};

const INDENT: &str = "  ";
const LABEL_WIDTH: usize = 11;
const VALUE_WIDTH: usize = 27;
const WRAP_COLUMN: usize = 78;

/// Audio Provenance's published position on a file that carries no recovered provenance. It is printed
/// verbatim, because the failure mode this guards against is a reader treating exit 3 as an
/// accusation.
pub const NOT_FOUND_NOTE: &str = "no manifest was recovered for this file. A missing mark is never proof of synthetic origin; plenty of legitimate recordings carry no mark, simply because they were never registered.";

/// What the identity slot says when nothing external vouched for the signer.
pub const IDENTITY_NOT_ESTABLISHED: &str = "not_established";

pub fn verify_result(result: &VerifyResult, name: &str, style: Style, verbose: bool) -> String {
    let mut out = String::new();
    let status = result.status.as_str();
    out.push_str(&style.paint(status_tone(result.status), &format!("{status:<10}")));
    out.push(' ');
    out.push_str(name);
    // The status word is a fixed vocabulary of four and CI keys on the exit code, so the honesty
    // about HOW it verified rides beside the name rather than inside the word.
    if verified_by_inference(result) {
        out.push_str(&style.paint(Tone::Warn, "  (inferred)"));
    }
    // IMPORTANT: the status word is one of four and `not_found` is among them, so an unexhausted
    // search prints the same headline as a search that finished and found nothing. Exit code 4
    // separates them for a script; this separates them for a reader.
    if result.incomplete {
        out.push_str(&style.paint(Tone::Warn, "  (incomplete)"));
    }
    out.push('\n');

    out.push_str(&row(
        "identity",
        &identity_value(result),
        &parenthesised(&identity_note(result)),
        style,
    ));
    out.push_str(&row(
        "signed",
        result.signed_at.as_deref().unwrap_or("-"),
        "",
        style,
    ));
    out.push_str(&row(
        "match",
        &match_value(result),
        &parenthesised(&match_note(result)),
        style,
    ));
    if let Some(registry) = &result.registry {
        out.push_str(&row(
            "registry",
            &format!("{} ({})", registry.name, registry.kind),
            &match result.method {
                Some(method) => format!("recovered via {}", method.as_str()),
                None => "no candidate recovered".to_string(),
            },
            style,
        ));
    }

    for note in notes(result) {
        out.push_str(&wrapped_note(&note, style));
    }

    if verbose {
        out.push_str(&trace(&result.trace, style));
        for discarded in &result.recovery.discarded {
            out.push_str(&wrapped_note(
                &format!(
                    "discarded {} candidate on {}: {}",
                    discarded.reason,
                    discarded.method.as_str(),
                    discarded.detail
                ),
                style,
            ));
        }
        for finding in &result.findings {
            if finding.code == TRUST_REFUSAL_CODE {
                continue;
            }
            out.push_str(&wrapped_note(
                &format!(
                    "{} {} at {}: {}",
                    finding.severity, finding.code, finding.path, finding.message
                ),
                style,
            ));
        }
    }
    out
}

pub fn inspect_report(report: &InspectReport, name: &str, style: Style) -> String {
    let mut out = String::new();
    out.push_str(&style.paint(Tone::Neutral, &format!("{:<10}", "inspected")));
    out.push(' ');
    out.push_str(name);
    out.push('\n');
    out.push_str(&row(
        "container",
        report.container,
        &format!("{} bytes", report.content_bytes),
        style,
    ));
    out.push_str(&row(
        "audio",
        &format!("{} Hz, {} ch", report.sample_rate, report.channels),
        &format!("{:.3} s", report.duration_seconds),
        style,
    ));
    out.push_str(&row("sha256", &report.content_sha256, "content", style));
    out.push_str(&row(
        "pcm sha256",
        &report.decoded_audio_sha256,
        "decoded audio",
        style,
    ));
    out.push_str(&row(
        "manifest",
        report.manifest_schema.unwrap_or("-"),
        &match &report.signer_id {
            Some(signer) => format!("signer {signer}"),
            None => String::new(),
        },
        style,
    ));
    out.push_str(&trace(&report.trace, style));
    out.push_str(&wrapped_note(
        "no verdict and no trust evaluation: inspect reports what was found, not what it means.",
        style,
    ));
    if report.incomplete {
        out.push_str(&wrapped_note(
            "a recovery step could not run, so this report is incomplete.",
            style,
        ));
    }
    out
}

fn status_tone(status: VerificationStatus) -> Tone {
    match status {
        VerificationStatus::Verified => Tone::Good,
        VerificationStatus::Changed => Tone::Bad,
        VerificationStatus::Untrusted => Tone::Warn,
        // IMPORTANT: neutral, never Bad. Colouring an unregistered file as a failure is the exact
        // implication the published position forbids.
        VerificationStatus::NotFound => Tone::Neutral,
    }
}

/// The finding `genotrace` emits when a configured trust store actively refused a signer, as
/// opposed to simply not covering it.
pub const TRUST_REFUSAL_CODE: &str = "trust_anchor_rejected";

/// The store's own sentence about why it refused, when it refused.
fn trust_refusal(result: &VerifyResult) -> Option<&str> {
    result
        .findings
        .iter()
        .find(|finding| finding.code == TRUST_REFUSAL_CODE)
        .map(|finding| finding.message.as_str())
}

/// A name is printed only alongside the authority that vouches for it. A name with no authority is
/// a self-asserted identity, which is the exact claim the prior art forbids, so it renders as
/// not_established rather than as a name.
fn identity_value(result: &VerifyResult) -> String {
    match (&result.identity, &result.identity_authority) {
        (Some(identity), Some(_)) => identity.clone(),
        _ => IDENTITY_NOT_ESTABLISHED.to_string(),
    }
}

fn identity_note(result: &VerifyResult) -> String {
    match (&result.identity, &result.identity_authority) {
        (Some(_), Some(authority)) => format!(
            "{} via {authority}",
            ProofLevel::ExternallyVerified.as_str()
        ),
        // Unreachable by construction in the SDK, which writes both fields from one verdict, but
        // rendering a bare name with no authority is the claim the prior art forbids, so it is
        // never printed even if the pair somehow arrives split.
        (Some(_), None) => format!(
            "{}: no authority named",
            result.identity_proof_level.as_str()
        ),
        (None, _) => format!(
            "{}: {}",
            result.identity_proof_level.as_str(),
            match (
                trust_refusal(result),
                result.signature.as_ref().map(|signature| signature.valid),
            ) {
                // IMPORTANT: a store that refused this signer made a statement. Rendering it as
                // the generic key-possession line makes a published revocation indistinguishable
                // from a key the store never heard of, which is the whole point of publishing one.
                (Some(_), _) => "the trust store refused this signer",
                (None, Some(true)) => "a valid signature proves key possession, not identity",
                (None, Some(false)) => "the signature did not verify",
                (None, None) => "nothing signed was recovered",
            }
        ),
    }
}

fn match_value(result: &VerifyResult) -> String {
    if result.match_basis == MatchBasis::None {
        return "-".to_string();
    }
    format!("{:.2}", result.r#match)
}

/// A `verified` that rests on the soft binding rather than on a digest that recomputed.
///
/// IMPORTANT: keyed on the basis, not on the reason string, so a future reason code cannot silently
/// drop the qualifier off a result that still needs it.
fn verified_by_inference(result: &VerifyResult) -> bool {
    result.status == VerificationStatus::Verified
        && result.match_basis == MatchBasis::MarkAndFingerprint
}

fn match_note(result: &VerifyResult) -> String {
    match result.match_basis {
        MatchBasis::None => result.binding.detail.clone(),
        MatchBasis::HardExact => match result.reason {
            "hard_binding_decoded_audio_only" => {
                "hard binding, decoded audio; container bytes differ".to_string()
            }
            _ => "hard binding, exact".to_string(),
        },
        MatchBasis::Genomark | MatchBasis::Fingerprint | MatchBasis::MarkAndFingerprint => format!(
            "{}, {}",
            result.binding.detail,
            result.binding.proof_level.as_str()
        ),
    }
}

fn notes(result: &VerifyResult) -> Vec<String> {
    let mut notes = Vec::new();
    // Not gated behind --verbose. A revocation is the strongest negative statement the identity
    // layer can make, and an operator who publishes one has to be able to see that it was applied.
    if let Some(refusal) = trust_refusal(result) {
        notes.push(refusal.to_string());
    }
    // IMPORTANT: the reassurance asserts the search was exhausted and the file legitimately
    // carries no mark. Under `incomplete` a rung never ran, so nothing of the sort was
    // established; the operational note below is then the only true one.
    if result.status == VerificationStatus::NotFound && !result.incomplete {
        notes.push(NOT_FOUND_NOTE.to_string());
    }
    if verified_by_inference(result) {
        notes.push(
            "verified (inferred, soft binding: the exact bytes changed, the recording is the same). The signed hard binding does not recompute over these bytes, so this is NOT an exact-bytes verification; a GenoMark payload naming this record decoded out of the audio and the record's own signed reference constellation matched it. That is the honest reading of a lossy path, not evidence the file is byte-identical to what was signed.".to_string(),
        );
    }
    if matches!(
        result.match_basis,
        MatchBasis::Genomark | MatchBasis::Fingerprint | MatchBasis::MarkAndFingerprint
    ) {
        let side = if result.r#match >= result.binding.threshold {
            "above"
        } else {
            "below"
        };
        notes.push(if result.match_basis == MatchBasis::MarkAndFingerprint {
            format!(
                "the signed hard binding failed and the soft binding answered in its place; alignment against the signed reference constellation is {side} the {:.2} floor.",
                result.binding.threshold
            )
        } else {
            format!(
                "hard binding unavailable (audio re-encoded); soft binding {side} threshold {:.2}.",
                result.binding.threshold
            )
        });
        match (
            result.binding.false_positive_rate_at_match,
            result.binding.false_positive_rate_basis,
        ) {
            (Some(rate), Some(basis)) => notes.push(format!(
                "False-positive rate at this match: {rate:.1e} ({basis})."
            )),
            _ => notes.push(
                "False-positive rate is not measured for this algorithm. A soft binding cannot verify without one; supply a bench null-test report with --null-test.".to_string(),
            ),
        }
    }
    if result.reason == "hard_binding_mismatch" {
        notes.push(
            "the signed hard binding does not recompute over this file, and nothing corroborates it as the same recording: the audio itself changed after signing, or a mark was lifted onto other audio.".to_string(),
        );
    }
    if result.incomplete {
        let stalled: Vec<&str> = result
            .trace
            .iter()
            .filter(|step| step.outcome == StepOutcome::Unavailable)
            .map(|step| step.method.as_str())
            .collect();
        notes.push(if stalled.is_empty() {
            "a recovery step could not run, so the search was not exhausted. This is an operational fault, not a verdict.".to_string()
        } else {
            format!(
                "a recovery step could not run ({}), so the search was not exhausted. This is an operational fault, not a verdict.",
                stalled.join(", ")
            )
        });
    }
    notes
}

fn trace(steps: &[RecoveryStep], style: Style) -> String {
    let mut out = String::new();
    for (position, step) in steps.iter().enumerate() {
        let label = if position == 0 { "trace" } else { "" };
        let tone = match step.outcome {
            StepOutcome::Hit => Tone::Good,
            StepOutcome::Unavailable => Tone::Warn,
            StepOutcome::Miss | StepOutcome::Skipped | StepOutcome::Degraded => Tone::Dim,
        };
        out.push_str(&format!(
            "{INDENT}{label:<5}  {:<26}{}  {:<11}{}\n",
            step.method.as_str(),
            style.paint(tone, &format!("{:<7}", step.outcome.as_str())),
            format!("{:.1}ms", step.duration_ms),
            style.paint(Tone::Dim, &step.detail),
        ));
    }
    out
}

fn parenthesised(note: &str) -> String {
    if note.is_empty() {
        return String::new();
    }
    format!("({note})")
}

fn row(label: &str, value: &str, note: &str, style: Style) -> String {
    if note.is_empty() {
        return format!("{INDENT}{label:<LABEL_WIDTH$}{value}\n");
    }
    // A digest is wider than the column. Padding it to a fixed width would run the note straight
    // into the last hex character, so an over-wide value falls back to a plain two-space gap.
    let padded = if value.chars().count() >= VALUE_WIDTH {
        format!("{value}  ")
    } else {
        format!("{value:<VALUE_WIDTH$}")
    };
    format!(
        "{INDENT}{label:<LABEL_WIDTH$}{padded}{}\n",
        style.paint(Tone::Dim, note)
    )
}

fn wrapped_note(note: &str, style: Style) -> String {
    let prefix = INDENT.len() + LABEL_WIDTH;
    let width = WRAP_COLUMN.saturating_sub(prefix).max(20);
    let mut out = String::new();
    let mut line = String::new();
    let mut first = true;
    for word in note.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push_str(&emit_note_line(&line, first, style));
            line.clear();
            first = false;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(&emit_note_line(&line, first, style));
    }
    out
}

fn emit_note_line(line: &str, first: bool, style: Style) -> String {
    let label = if first { "note" } else { "" };
    format!(
        "{INDENT}{label:<LABEL_WIDTH$}{}\n",
        style.paint(Tone::Dim, line)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use genotrace::result::{FindingReport, MarkRecoveryReport, RecordingAssociationReport};
    use genotrace::{BindingKind, BindingReport, RecordingAssociationStatus, RecoveryReport};

    fn result(status: VerificationStatus, reason: &'static str) -> VerifyResult {
        VerifyResult {
            status,
            reason,
            identity: None,
            identity_proof_level: ProofLevel::UnknownUnobserved,
            identity_authority: None,
            signed_at: None,
            r#match: 0.0,
            match_basis: MatchBasis::None,
            binding: BindingReport {
                kind: BindingKind::None,
                r#match: 0.0,
                proof_level: ProofLevel::UnknownUnobserved,
                threshold: 0.72,
                false_positive_rate_at_match: None,
                false_positive_rate_basis: None,
                expected_sha256: None,
                observed_sha256: "0".repeat(64),
                detail: "no binding evidence recovered".to_string(),
            },
            signature: None,
            trace: Vec::new(),
            recovery: RecoveryReport {
                exhausted: true,
                discarded: Vec::new(),
                diagnostics: Vec::new(),
            },
            mark_recovery: MarkRecoveryReport {
                recovered: false,
                located: false,
                method: None,
                recovered_via_mark: false,
                record_locator_matched: false,
                proof_level: ProofLevel::UnknownUnobserved,
            },
            recording_association: RecordingAssociationReport {
                status: RecordingAssociationStatus::NotEvaluated,
                established: false,
                proof_level: ProofLevel::UnknownUnobserved,
                r#match: 0.0,
            },
            findings: Vec::new(),
            manifest: None,
            registry: None,
            content_sha256: "0".repeat(64),
            content_bytes: 0,
            decoded_audio_sha256: None,
            method: None,
            incomplete: false,
        }
    }

    /// An empty identity slot, or a signer id shown where a vouched-for name belongs, is the exact
    /// misreport the prior art forbids.
    #[test]
    fn absent_identity_is_named_and_never_blank() {
        let rendered = verify_result(
            &result(VerificationStatus::Untrusted, "trust_anchor_unresolved"),
            "demo.wav",
            Style::plain(),
            false,
        );
        assert!(rendered.contains(IDENTITY_NOT_ESTABLISHED), "{rendered}");
        assert!(
            rendered.contains("proves key possession, not identity")
                || rendered.contains("nothing signed was recovered"),
            "{rendered}"
        );
        assert!(!rendered.contains("identity   \n"), "{rendered}");
    }

    /// A store that refused this signer said something specific and retroactive. Rendering that
    /// identically to a key the store never heard of throws away the only signal a revocation has.
    #[test]
    fn a_refused_signer_never_renders_as_an_unknown_one() {
        let unknown = result(VerificationStatus::Untrusted, "trust_anchor_unresolved");
        let mut revoked = result(VerificationStatus::Untrusted, "trust_anchor_rejected");
        revoked.findings.push(FindingReport {
            severity: "warning",
            code: TRUST_REFUSAL_CODE,
            path: "$.portable_signature".to_string(),
            message: "signer_revoked: Signal Room Studios was revoked at 2026-09-01T00:52:39Z \
                      for key_compromise. Revocation is retroactive."
                .to_string(),
        });

        let unknown_out = verify_result(&unknown, "demo.wav", Style::plain(), false);
        let revoked_out = verify_result(&revoked, "demo.wav", Style::plain(), false);

        assert_ne!(unknown_out, revoked_out);
        assert!(
            revoked_out.contains(IDENTITY_NOT_ESTABLISHED),
            "{revoked_out}"
        );
        let flowed = revoked_out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flowed.contains("refused this signer"), "{revoked_out}");
        assert!(flowed.contains("signer_revoked"), "{revoked_out}");
        assert!(
            flowed.contains("Revocation is retroactive"),
            "{revoked_out}"
        );
    }

    /// `not_found` is a neutral result. The vocabulary that would make it read as an accusation is
    /// pinned out.
    #[test]
    fn not_found_reads_as_neutral() {
        let rendered = verify_result(
            &result(VerificationStatus::NotFound, "no_manifest_recovered"),
            "demo.wav",
            Style::plain(),
            false,
        );
        let flowed = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            flowed.contains("A missing mark is never proof of synthetic origin"),
            "{rendered}"
        );
        for forbidden in [
            "fake",
            "suspicious",
            "counterfeit",
            "forged",
            "unverified file",
        ] {
            assert!(
                !flowed.to_lowercase().contains(forbidden),
                "{forbidden:?} in {rendered}"
            );
        }
    }

    /// A registry outage exits 4 while the status field still reads `not_found`. Printing the
    /// reassurance there tells the reader the file legitimately carries no mark, which the run
    /// never established, and contradicts the operational note in the same block.
    #[test]
    fn an_unexhausted_search_never_claims_the_file_carries_no_mark() {
        let mut incomplete = result(VerificationStatus::NotFound, "recovery_incomplete");
        incomplete.incomplete = true;
        incomplete.recovery.exhausted = false;
        let rendered = verify_result(&incomplete, "demo.mp3", Style::plain(), false);
        let flowed = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !flowed.contains("A missing mark is never proof of synthetic origin"),
            "{rendered}"
        );
        assert!(
            flowed.contains("operational fault, not a verdict"),
            "{rendered}"
        );
        assert!(
            flowed.starts_with("not_found demo.mp3 (incomplete)"),
            "the headline must not read as a finished search: {rendered}"
        );
    }
}
