//! Timestamps are stored as milliseconds since the Unix epoch (UTC).

use chrono::{DateTime, NaiveDate, SecondsFormat, TimeZone, Utc};
use std::sync::atomic::{AtomicI64, Ordering};

/// Offset added to the system clock, so tests can move time forward.
static OFFSET_MS: AtomicI64 = AtomicI64::new(0);

thread_local! {
    /// The clock offset of the database being used on this thread (see `Db::set_offset`), so
    /// several instances in one process can each have their own clock.
    static LOCAL_OFFSET_MS: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

pub fn now() -> i64 {
    Utc::now().timestamp_millis() + OFFSET_MS.load(Ordering::Relaxed) + LOCAL_OFFSET_MS.with(|o| o.get())
}

/// Runs `f` with `ms` as this thread's local clock offset.
pub fn with_local_offset<T>(ms: i64, f: impl FnOnce() -> T) -> T {
    let prev = LOCAL_OFFSET_MS.with(|o| o.replace(ms));
    let out = f();
    LOCAL_OFFSET_MS.with(|o| o.set(prev));
    out
}

pub fn set_offset(ms: i64) {
    OFFSET_MS.store(ms, Ordering::Relaxed);
}

fn dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

/// REST API format: `2026-11-07T03:03:16.052Z`.
pub fn iso(ms: i64) -> String {
    dt(ms).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Seconds precision with `Z`, used in some webhook payloads: `2026-11-07T03:03:16Z`.
pub fn iso_secs(ms: i64) -> String {
    dt(ms).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Ruby `Time#to_s` for UTC times, used in webhook payloads: `2026-11-07 03:03:16 UTC`.
pub fn ruby_utc(ms: i64) -> String {
    dt(ms).format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// Date in `YYYY-MM-DD` form.
pub fn date(ms: i64) -> String {
    dt(ms).format("%Y-%m-%d").to_string()
}

pub fn today() -> String {
    date(now())
}

pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Parses ISO 8601 timestamps as GitLab accepts them in query parameters.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    // Numeric offsets in other spellings (`+0200`, a space before the offset).
    for f in ["%Y-%m-%d %H:%M:%S %z", "%Y-%m-%d %H:%M:%S%z", "%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%d %H:%M:%S%.f %z"] {
        if let Ok(t) = DateTime::parse_from_str(s, f) {
            return Some(t.timestamp_millis());
        }
    }
    // A trailing `UTC`/`GMT`/`Z` zone name.
    for zone in [" UTC", " GMT", "UTC", "Z"] {
        if let Some(rest) = s.strip_suffix(zone)
            && rest != s
            && let Some(t) = parse_time(rest.trim_end())
        {
            return Some(t);
        }
    }
    for f in ["%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(Utc.from_utc_datetime(&t).timestamp_millis());
        }
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(Utc.from_utc_datetime(&t).timestamp_millis());
        }
    }
    if let Some(d) = parse_date(s) {
        return Some(
            Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?)
                .timestamp_millis(),
        );
    }
    None
}

/// ISO 8601 with milliseconds and a numeric offset, as GitLab renders git dates:
/// `2026-11-07T03:03:16.000+00:00`.
pub fn iso_git(secs: i64, offset_minutes: i32) -> String {
    let tz = chrono::FixedOffset::east_opt(offset_minutes * 60)
        .unwrap_or(chrono::FixedOffset::east_opt(0).unwrap());
    let t = chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_default()
        .with_timezone(&tz);
    t.to_rfc3339_opts(SecondsFormat::Millis, false)
}
