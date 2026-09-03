use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;

/// `datetime.fromtimestamp(t, utc).isoformat().replace("+00:00","Z")`: the
/// microsecond field is present only when non-zero, and then always 6 digits.
/// `2025-08-31T00:26:40Z` versus `2025-08-31T00:26:40.500000Z`.
pub fn utc_timestamp(at: Option<SystemTime>) -> String {
    let (seconds, microseconds) = split_epoch(at.unwrap_or_else(SystemTime::now));
    let Some(moment) = OffsetDateTime::from_unix_timestamp(seconds).ok() else {
        return String::from("1970-01-01T00:00:00Z");
    };
    let date = format_date_time(&moment);
    if microseconds == 0 {
        format!("{date}Z")
    } else {
        format!("{date}.{microseconds:06}Z")
    }
}

/// `time.strftime("%Y-%m-%dT%H:%M:%SZ", gmtime())`: whole seconds, no fraction.
/// Used only for `export.exported_at`.
pub fn utc_timestamp_seconds(at: Option<SystemTime>) -> String {
    let (seconds, _) = split_epoch_truncating(at.unwrap_or_else(SystemTime::now));
    let Some(moment) = OffsetDateTime::from_unix_timestamp(seconds).ok() else {
        return String::from("1970-01-01T00:00:00Z");
    };
    format!("{}Z", format_date_time(&moment))
}

fn format_date_time(moment: &OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        moment.year(),
        u8::from(moment.month()),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second()
    )
}

fn split_epoch_truncating(at: SystemTime) -> (i64, u32) {
    match at.duration_since(UNIX_EPOCH) {
        Ok(delta) => (
            i64::try_from(delta.as_secs()).unwrap_or(i64::MAX),
            delta.subsec_nanos(),
        ),
        Err(err) => {
            let delta = err.duration();
            let seconds = i64::try_from(delta.as_secs()).unwrap_or(i64::MAX);
            if delta.subsec_nanos() == 0 {
                (-seconds, 0)
            } else {
                (-seconds - 1, 1_000_000_000 - delta.subsec_nanos())
            }
        }
    }
}

fn split_epoch(at: SystemTime) -> (i64, u32) {
    let (seconds, nanos) = split_epoch_truncating(at);
    let micros = (u64::from(nanos) + 500) / 1000;
    if micros >= 1_000_000 {
        (seconds.saturating_add(1), 0)
    } else {
        (seconds, u32::try_from(micros).unwrap_or(0))
    }
}
