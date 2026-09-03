//! A deliberately narrow instant: exactly `YYYY-MM-DDTHH:MM:SSZ`, 20 ASCII bytes.
//!
//! RFC 3339 permits fractional seconds and non-UTC offsets, both of which break the property this
//! type exists for: at fixed width with no optional parts, byte ordering IS chronological ordering,
//! so a validity-window comparison is a string comparison and cannot carry an arithmetic bug.
//! `audio_provenance_registry::SignedAt` accepts the wider grammar and its derived `Ord` therefore sorts
//! `...:00.5Z` before `...:00Z`; nothing in this crate may depend on that ordering.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::TrustError;

pub const INSTANT_LEN: usize = 20;

const SECONDS_PER_DAY: i64 = 86_400;

/// A UTC instant at second precision.
///
/// IMPORTANT: no constructor reads a clock. The evaluation instant is always supplied by the
/// caller, matching the registry's rule, so every chain decision in this crate is reproducible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(String);

impl Instant {
    pub fn parse(text: &str) -> Result<Self, TrustError> {
        let bytes = text.as_bytes();
        if bytes.len() != INSTANT_LEN {
            return Err(TrustError::Instant {
                found: truncated(text),
            });
        }
        let at = |index: usize| bytes.get(index).copied().unwrap_or(0);
        if at(4) != b'-'
            || at(7) != b'-'
            || at(10) != b'T'
            || at(13) != b':'
            || at(16) != b':'
            || at(19) != b'Z'
        {
            return Err(TrustError::Instant {
                found: truncated(text),
            });
        }
        let field = |range: core::ops::Range<usize>| -> Option<u32> {
            let part = text.get(range)?;
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse::<u32>().ok()
        };
        let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
            field(0..4),
            field(5..7),
            field(8..10),
            field(11..13),
            field(14..16),
            field(17..19),
        ) else {
            return Err(TrustError::Instant {
                found: truncated(text),
            });
        };
        if year == 0
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            // No leap second. A leap second is not representable as a distinct instant here and
            // accepting :60 would let two documents claim the same ordering position.
            || second > 59
        {
            return Err(TrustError::Instant {
                found: truncated(text),
            });
        }
        Ok(Self(text.to_string()))
    }

    /// The clock-free conversion callers use to turn a host clock into an evaluation instant.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, TrustError> {
        let days = seconds.div_euclid(SECONDS_PER_DAY);
        let rest = seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        if !(1..=9999).contains(&year) {
            return Err(TrustError::Instant {
                found: seconds.to_string(),
            });
        }
        Self::parse(&format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rest / 3600,
            (rest % 3600) / 60,
            rest % 60
        ))
    }

    /// Seconds since the epoch. The inverse of [`Self::from_unix_seconds`], and the only reason it
    /// exists is that a validity window is issued as "now plus N days".
    pub fn unix_seconds(&self) -> i64 {
        let field = |range: core::ops::Range<usize>| -> i64 {
            // Every field was range-checked by `parse`, which is the only constructor.
            self.0
                .get(range)
                .and_then(|part| part.parse::<i64>().ok())
                .unwrap_or(0)
        };
        let (year, month, day) = (field(0..4), field(5..7), field(8..10));
        let (hour, minute, second) = (field(11..13), field(14..16), field(17..19));
        days_from_civil(year, month, day) * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second
    }

    /// This instant advanced by `days`, for issuing a validity window.
    pub fn plus_days(&self, days: i64) -> Result<Self, TrustError> {
        let seconds = days
            .checked_mul(SECONDS_PER_DAY)
            .and_then(|span| self.unix_seconds().checked_add(span))
            .ok_or_else(|| TrustError::Instant {
                found: format!("{self} plus {days} days"),
            })?;
        Self::from_unix_seconds(seconds)
    }

    /// Promotes a bare `YYYY-MM-DD` to midnight UTC, and accepts a full instant unchanged. A
    /// manifest carries a calendar date while this crate compares instants; the widening is
    /// explicit so no path can invent a time of day silently.
    pub fn parse_date_or_instant(text: &str) -> Result<Self, TrustError> {
        if text.len() == 10 {
            return Self::parse(&format!("{text}T00:00:00Z"));
        }
        Self::parse(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn date(&self) -> &str {
        self.0.get(0..10).unwrap_or(&self.0)
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Instant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Instant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A half-open window `[not_before, not_after)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub not_before: Instant,
    pub not_after: Instant,
}

impl Window {
    pub fn new(not_before: Instant, not_after: Instant) -> Result<Self, TrustError> {
        if not_after <= not_before {
            return Err(TrustError::EmptyWindow {
                not_before: not_before.as_str().to_string(),
                not_after: not_after.as_str().to_string(),
            });
        }
        Ok(Self {
            not_before,
            not_after,
        })
    }

    pub fn contains(&self, at: &Instant) -> bool {
        *at >= self.not_before && *at < self.not_after
    }
}

const fn is_leap(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Hinnant's `days_from_civil`, the inverse of [`civil_from_days`] over the same era shift.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Hinnant's `civil_from_days`, era-shifted to 0000-03-01.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

fn truncated(text: &str) -> String {
    text.chars().take(INSTANT_LEN + 8).collect()
}

#[cfg(test)]
mod tests {
    use super::{Instant, Window};

    /// The window comparison is a byte comparison, so it is only correct while the grammar stays
    /// fixed-width. Every rejection below is a value that would have widened it.
    #[test]
    fn rejects_every_grammar_that_would_break_byte_ordering() {
        for text in [
            "2026-08-31T00:00:00.5Z",
            "2026-08-31T00:00:00+01:00",
            "2026-08-31T00:00:00",
            "2026-08-31 00:00:00Z",
            "2026-08-31T00:00:60Z",
            "2026-02-30T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-08-31T24:00:00Z",
            "",
        ] {
            assert!(Instant::parse(text).is_err(), "accepted {text:?}");
        }
        assert!(Instant::parse("2024-02-29T23:59:59Z").is_ok());
    }

    /// Expected values come from Python's `datetime.utcfromtimestamp` and `calendar.timegm`, not
    /// from this code. The round trip has to be exact in both directions or an issued validity
    /// window silently drifts by a day.
    #[test]
    fn converts_unix_seconds_to_the_calendar_and_back() {
        for (seconds, expected) in [
            (0, "1970-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_756_598_400, "2025-08-31T00:00:00Z"),
            (1_788_134_400, "2026-08-31T00:00:00Z"),
            (-1, "1969-12-31T23:59:59Z"),
            (-2_208_988_800, "1900-01-01T00:00:00Z"),
        ] {
            let instant = Instant::from_unix_seconds(seconds).unwrap();
            assert_eq!(instant.as_str(), expected, "second {seconds}");
            assert_eq!(instant.unix_seconds(), seconds, "{expected}");
        }
        assert_eq!(
            Instant::parse("2026-02-27T06:30:00Z")
                .unwrap()
                .plus_days(2)
                .unwrap()
                .as_str(),
            "2026-03-01T06:30:00Z"
        );
    }

    #[test]
    fn window_is_half_open_at_both_ends() {
        let window = Window::new(
            Instant::parse("2026-01-01T00:00:00Z").unwrap(),
            Instant::parse("2027-01-01T00:00:00Z").unwrap(),
        )
        .unwrap();
        assert!(!window.contains(&Instant::parse("2025-12-31T23:59:59Z").unwrap()));
        assert!(window.contains(&Instant::parse("2026-01-01T00:00:00Z").unwrap()));
        assert!(window.contains(&Instant::parse("2026-12-31T23:59:59Z").unwrap()));
        assert!(!window.contains(&Instant::parse("2027-01-01T00:00:00Z").unwrap()));
    }
}
