use apw_core::{VerificationState, NOTHING_FOUND_NORMATIVE_NOTE};
use apw_provenance::{expand_user, LocalReferenceProvider, ProvenanceProvider};
use serde_json::Value;

use crate::cli::VerifyArgs;
use crate::error::{CliError, Result};
use crate::verify_state::state_of_file;

/// A locally issued root proves that a claim chains to a key this machine holds.
/// It is never an external trust list, a registry, or a verified creator.
const LOCAL_ROOT_SCOPE: &str = "self_issued_local_root_only";
const CALLER_ANCHOR_SCOPE: &str = "caller_supplied_anchor_list";
const NO_ANCHOR_SCOPE: &str = "no_trust_anchors_supplied";

/// Exit codes the demo scripts branch on: 0 verified, 3 graded as anything else,
/// 1 an input the verifier could not grade at all.
pub const EXIT_VERIFIED: i32 = 0;
pub const EXIT_NOT_VERIFIED: i32 = 3;

pub fn run(args: &VerifyArgs) -> Result<i32> {
    if !args.target.exists() {
        return Err(CliError::io(
            "verify",
            &args.target,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        ));
    }
    // The local manifest verifier grades an evidence hash chain, a coverage
    // cross-check and a C2PA claim cross-check that this engine does not
    // implement. Grading a manifest on the subset it can check would emit a
    // clean result from checks that never ran.
    if args
        .target
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        return Err(CliError::ManifestVerifierUnported {
            path: args.target.clone(),
        });
    }

    let (anchors, anchor_scope) = trust_anchors(args)?;
    let sidecar = match &args.sidecar {
        Some(path) => {
            Some(std::fs::read(path).map_err(|source| CliError::io("read", path, source))?)
        }
        None => None,
    };

    let verified = state_of_file(
        &args.target,
        anchors.as_deref(),
        anchor_scope,
        sidecar.as_deref(),
    )?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&verified.record)?);
    } else {
        print_summary(&verified.record, verified.state);
    }

    Ok(if verified.state == VerificationState::Verified {
        EXIT_VERIFIED
    } else {
        EXIT_NOT_VERIFIED
    })
}

fn print_summary(record: &Value, state: VerificationState) {
    let field = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    println!("STATE:  {}", state.as_str());
    println!("FILE:   {}", field("file"));
    println!("DETAIL: {}", field("detail"));
    println!("TRUST:  {}", field("trust_anchor_scope"));
    println!("SIGNER: not_established (a signing key is not a verified identity)");
    let codes: Vec<&str> = record
        .get("failure_codes")
        .and_then(Value::as_array)
        .map(|codes| codes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !codes.is_empty() {
        println!("CODES:  {}", codes.join(", "));
    }
    if state == VerificationState::NothingFound {
        println!("NOTE:   {NOTHING_FOUND_NORMATIVE_NOTE}");
    }
}

fn trust_anchors(args: &VerifyArgs) -> Result<(Option<String>, &'static str)> {
    if let Some(path) = &args.trust_anchor {
        let pem =
            std::fs::read_to_string(path).map_err(|source| CliError::io("read", path, source))?;
        return Ok((Some(pem), CALLER_ANCHOR_SCOPE));
    }
    if args.trust_local_root {
        let store = expand_user(&args.provenance_store);
        let provider = LocalReferenceProvider::new(&store)?;
        let material = provider.issue_signing_material()?;
        let pem = String::from_utf8(material.trust_anchor_pem)
            .map_err(|_| CliError::usage("the local trust anchor PEM is not valid UTF-8"))?;
        return Ok((Some(pem), LOCAL_ROOT_SCOPE));
    }
    Ok((None, NO_ANCHOR_SCOPE))
}
