//! The C ABI a JUCE plug-in links instead of talking to a separate process.
//!
//! IMPORTANT: every entry point parses attacker-supplied audio. A panic
//! unwinding across an `extern "C"` boundary is undefined behaviour, so each
//! body is wrapped in [`std::panic::catch_unwind`] and reported as an error
//! code. No entry point may be called from the audio thread: they all do
//! filesystem I/O.
//!
//! Ownership: every `*mut c_char` this library hands out was allocated by it and
//! MUST be returned to [`apw_string_free`]. Nothing else may free it.
//!
//! Linking: `libapw_engine.a` needs no `-framework` and no C++ runtime. The
//! workspace builds `c2pa` with `rust_native_crypto` instead of the OpenSSL
//! backend, so the archive is self-contained; `cc probe.c libapw_engine.a`
//! links and runs as-is on both slices.

// TODO: `apw_verify_file` takes no detached manifest, so an AIFF asset (which
// cannot embed one) always grades `nothing_found` here. Add a sidecar parameter
// when the plug-in needs to verify a container that carries its claim beside it.

use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use apw_cli::state_of_file;
use apw_core::VerificationState;

/// The file was graded and carries a trusted claim.
pub const APW_STATE_VERIFIED: c_int = 0;
/// A claim was found, but the asset's bytes no longer match its hard binding.
pub const APW_STATE_REGISTERED_BUT_CHANGED: c_int = 1;
/// A claim was found and its signer does not chain to a supplied anchor.
pub const APW_STATE_MARK_FOUND_CLAIM_NOT_TRUSTED: c_int = 2;
/// No provenance data was found.
///
/// IMPORTANT: this is the absence of a record, never evidence that the asset is
/// synthetic, machine-generated, or untrustworthy.
pub const APW_STATE_NOTHING_FOUND: c_int = 3;

/// A null, non-UTF-8, or empty path argument.
pub const APW_ERR_INVALID_ARGUMENT: c_int = -1;
/// The file could not be read or is not a container this engine parses.
pub const APW_ERR_UNREADABLE: c_int = -2;
/// The engine failed internally. The out-parameter carries the reason.
pub const APW_ERR_INTERNAL: c_int = -3;

const NO_ANCHOR_SCOPE: &str = "no_trust_anchors_supplied";
const CALLER_ANCHOR_SCOPE: &str = "caller_supplied_anchor_list";

/// The engine version, as a static NUL-terminated string.
///
/// The returned pointer is static and MUST NOT be passed to [`apw_string_free`].
#[no_mangle]
pub extern "C" fn apw_engine_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Grade one file into one of the four provenance states.
///
/// `path_utf8` is a NUL-terminated UTF-8 path. `trust_anchors_pem` is either
/// NULL (no anchors, so no claim can reach `verified`) or a NUL-terminated PEM
/// bundle. On return, `out_json` receives an owned NUL-terminated JSON record
/// describing the outcome; the caller frees it with [`apw_string_free`].
///
/// Returns one of the `APW_STATE_*` codes, or a negative `APW_ERR_*` code. On a
/// negative return `out_json` still receives a record when one could be built.
///
/// # Safety
///
/// `path_utf8` and, when non-null, `trust_anchors_pem` must point to
/// NUL-terminated C strings that stay valid for the duration of the call.
/// `out_json`, when non-null, must point to one writable `*mut c_char`.
#[no_mangle]
pub unsafe extern "C" fn apw_verify_file(
    path_utf8: *const c_char,
    trust_anchors_pem: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    if !out_json.is_null() {
        // SAFETY: the caller contract requires one writable slot here.
        unsafe { *out_json = core::ptr::null_mut() };
    }
    // SAFETY: the caller contract requires NUL-terminated strings; both reads
    // happen before any unwind-capable code runs.
    let path = match unsafe { borrow(path_utf8) } {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => return APW_ERR_INVALID_ARGUMENT,
    };
    let anchors = unsafe { borrow(trust_anchors_pem) };
    if !trust_anchors_pem.is_null() && anchors.is_none() {
        return APW_ERR_INVALID_ARGUMENT;
    }

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let scope = if anchors.is_some() {
            CALLER_ANCHOR_SCOPE
        } else {
            NO_ANCHOR_SCOPE
        };
        state_of_file(&path, anchors.as_deref(), scope, None)
    }));

    let (code, record) = match outcome {
        Ok(Ok(verified)) => (
            state_code(verified.state),
            verified.record.to_string(),
        ),
        Ok(Err(error)) => (
            APW_ERR_UNREADABLE,
            error_record(APW_ERR_UNREADABLE, &apw_cli::one_line(&error)),
        ),
        // A panic here means an engine defect, not a verdict about the file. It
        // must never be reported as `nothing_found`, which a reader could take
        // as a statement about the asset.
        Err(_) => (
            APW_ERR_INTERNAL,
            error_record(APW_ERR_INTERNAL, "the verification engine panicked"),
        ),
    };

    if !out_json.is_null() {
        if let Ok(owned) = CString::new(record) {
            // SAFETY: the caller contract requires one writable slot here, and
            // ownership of the allocation transfers with the write.
            unsafe { *out_json = owned.into_raw() };
        }
    }
    code
}

/// Free a string this library returned through an out-parameter.
///
/// # Safety
///
/// `value` must be either NULL or a pointer this library handed out and that has
/// not already been freed.
#[no_mangle]
pub unsafe extern "C" fn apw_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    // SAFETY: the caller contract restricts `value` to a pointer produced by
    // `CString::into_raw` in this library and not yet freed.
    drop(unsafe { CString::from_raw(value) });
}

fn state_code(state: VerificationState) -> c_int {
    match state {
        VerificationState::Verified => APW_STATE_VERIFIED,
        VerificationState::RegisteredButChanged => APW_STATE_REGISTERED_BUT_CHANGED,
        VerificationState::MarkFoundClaimNotTrusted => APW_STATE_MARK_FOUND_CLAIM_NOT_TRUSTED,
        VerificationState::NothingFound => APW_STATE_NOTHING_FOUND,
    }
}

fn error_record(code: c_int, detail: &str) -> String {
    serde_json::json!({
        "state": Option::<&str>::None,
        "error_code": code,
        "detail": detail,
    })
    .to_string()
}

/// # Safety
///
/// `raw` must be NULL or point to a NUL-terminated string valid for the call.
unsafe fn borrow(raw: *const c_char) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: delegated to this function's own contract.
    unsafe { CStr::from_ptr(raw) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::io::Write;

    /// The FFI boundary must not report an unparseable file as `nothing_found`:
    /// that code is a statement about provenance data, and a caller could read it
    /// as a statement about the asset.
    #[test]
    fn an_unparseable_file_is_an_error_not_nothing_found() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("not-audio.wav");
        let mut file = std::fs::File::create(&path)?;
        file.write_all(b"this is not a RIFF container")?;
        drop(file);

        let c_path = CString::new(path.to_string_lossy().as_ref())?;
        let mut out: *mut c_char = core::ptr::null_mut();
        let code = unsafe { apw_verify_file(c_path.as_ptr(), core::ptr::null(), &mut out) };

        assert_eq!(code, APW_ERR_UNREADABLE);
        assert!(!out.is_null());
        let json = unsafe { CStr::from_ptr(out) }.to_str()?.to_owned();
        unsafe { apw_string_free(out) };
        assert!(json.contains("\"error_code\":-2"), "{json}");
        assert!(!json.contains("nothing_found"), "{json}");
        Ok(())
    }

    #[test]
    fn a_null_path_is_rejected_before_any_io() {
        let mut out: *mut c_char = core::ptr::null_mut();
        let code = unsafe { apw_verify_file(core::ptr::null(), core::ptr::null(), &mut out) };
        assert_eq!(code, APW_ERR_INVALID_ARGUMENT);
        assert!(out.is_null());
    }
}
