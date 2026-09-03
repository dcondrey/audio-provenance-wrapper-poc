use std::sync::OnceLock;
use std::time::Instant;

/// Process-relative monotonic milliseconds, the daemon clock the correlation
/// engine and every `daemon_*_monotonic_ms` field are expressed in.
pub fn monotonic_millis() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// `bytes * 2` lowercase hex characters from the OS CSPRNG. Falls back to the
/// monotonic clock only if the CSPRNG is unavailable, which cannot make an
/// identifier meaningful but must not abort a capture session.
pub fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes.min(32)];
    if getrandom::getrandom(&mut buffer).is_err() {
        let fallback = monotonic_millis().to_le_bytes();
        for (slot, byte) in buffer.iter_mut().zip(fallback.iter().cycle()) {
            *slot = *byte;
        }
    }
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// CPython `round(value, digits)`: round-half-to-even on the exact binary value.
/// Rust's float formatter rounds the same way, so formatting and re-parsing
/// reproduces it; `format!("{:.3}")` on a non-finite value would render "NaN",
/// so those are returned unchanged for the canonicaliser to reject.
pub fn python_round(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.digits$}").parse::<f64>().unwrap_or(value)
}
