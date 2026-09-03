//! The UTC calendar date, with no clock dependency beyond the one call that reads it.

use std::time::{SystemTime, UNIX_EPOCH};

use audio_provenance_registry::SignedAt;
use audio_provenance_trust::Instant;

use crate::error::CliError;

/// A manifest carries a calendar date; a registry record carries an RFC 3339 UTC instant. Midnight
/// UTC is the only instant a bare date supports, so the promotion is explicit rather than a
/// reformat that could invent a time of day.
pub fn registry_signed_at(signed_at: &str) -> Result<SignedAt, CliError> {
    match SignedAt::parse(signed_at) {
        Ok(parsed) => Ok(parsed),
        Err(_) => Ok(SignedAt::parse(&format!("{signed_at}T00:00:00Z"))?),
    }
}

pub fn today_utc() -> Result<String, CliError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CliError::usage("system clock is before 1970; pass --signed-at=<YYYY-MM-DD>"))?
        .as_secs();
    let seconds = i64::try_from(seconds).map_err(|_| {
        CliError::usage("system clock is unreadable; pass --signed-at=<YYYY-MM-DD>")
    })?;
    // The civil-calendar conversion lives in `audio-provenance-trust`, where a validity window already
    // depends on it being right and its tests already pin the epoch, a leap day and a non-leap
    // century. A second copy here would be a second thing to get wrong.
    let instant = Instant::from_unix_seconds(seconds).map_err(|_| {
        CliError::usage("system clock is out of range; pass --signed-at=<YYYY-MM-DD>")
    })?;
    Ok(instant.date().to_string())
}
