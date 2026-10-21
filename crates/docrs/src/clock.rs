//! Timestamps for revision and comment stamping (upstream `_clock.py` and
//! `ST_DateTime`).
//!
//! OOXML dates are written as UTC `YYYY-MM-DDTHH:MM:SSZ` with whole seconds,
//! exactly as upstream serializes them. Operations take `Option<Timestamp>`;
//! `None` means [`Timestamp::now`]. Upstream interprets *naive* Python
//! datetimes in the local zone; this API has no naive form — callers supply
//! an instant (deliberate, documented difference).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Result};

/// A UTC instant with whole-second precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    secs: i64,
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

impl Timestamp {
    /// From seconds since the Unix epoch.
    pub fn from_unix(secs: i64) -> Self {
        Timestamp { secs }
    }

    /// Seconds since the Unix epoch.
    pub fn unix(self) -> i64 {
        self.secs
    }

    /// Current time truncated to whole seconds (upstream `_clock.now`).
    pub fn now() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Timestamp { secs }
    }

    /// From UTC calendar fields.
    pub fn from_utc(
        year: i64,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Result<Self> {
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return Err(Error::invalid_argument(
                "invalid_datetime",
                "calendar field out of range",
            ));
        }
        let days = days_from_civil(year, month as i64, day as i64);
        let (y, m, d) = civil_from_days(days);
        if (y, m, d) != (year, month as i64, day as i64) {
            return Err(Error::invalid_argument(
                "invalid_datetime",
                "day does not exist in that month",
            ));
        }
        Ok(Timestamp {
            secs: days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64,
        })
    }

    /// Parse an ISO 8601 instant: `YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)`.
    /// Fractional seconds are truncated (OOXML precision).
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || {
            Error::invalid_argument(
                "invalid_datetime",
                format!("not an ISO 8601 instant with zone: {text:?}"),
            )
        };
        let (date, rest) = text.split_once('T').ok_or_else(bad)?;
        let mut date_parts = date.splitn(3, '-');
        let year: i64 = date_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let month: u32 = date_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let day: u32 = date_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let (time, offset_secs) = if let Some(time) = rest.strip_suffix('Z') {
            (time, 0i64)
        } else {
            let split = rest.rfind(['+', '-']).ok_or_else(bad)?;
            let (time, zone) = rest.split_at(split);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (h, m) = zone[1..].split_once(':').ok_or_else(bad)?;
            let h: i64 = h.parse().map_err(|_| bad())?;
            let m: i64 = m.parse().map_err(|_| bad())?;
            (time, sign * (h * 3600 + m * 60))
        };
        let time = time.split('.').next().unwrap_or(time);
        let mut time_parts = time.splitn(3, ':');
        let hour: u32 = time_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let minute: u32 = time_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let second: u32 = time_parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(bad)?;
        let local = Self::from_utc(year, month, day, hour, minute, second)?;
        Ok(Timestamp {
            secs: local.secs - offset_secs,
        })
    }

    /// `ST_DateTime` serialization: `YYYY-MM-DDTHH:MM:SSZ` (UTC).
    pub fn to_xml(self) -> String {
        let days = self.secs.div_euclid(86_400);
        let rem = self.secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_like_st_datetime() {
        let ts = Timestamp::parse("2024-01-02T03:04:05Z").unwrap();
        assert_eq!(ts.to_xml(), "2024-01-02T03:04:05Z");
        assert_eq!(
            Timestamp::parse("2024-01-02T03:04:05.000123+05:00")
                .unwrap()
                .to_xml(),
            "2024-01-01T22:04:05Z"
        );
        assert_eq!(Timestamp::from_unix(0).to_xml(), "1970-01-01T00:00:00Z");
        assert_eq!(
            Timestamp::parse("2000-02-29T23:59:59Z").unwrap().to_xml(),
            "2000-02-29T23:59:59Z"
        );
        assert!(Timestamp::parse("2001-02-29T00:00:00Z").is_err());
        assert!(Timestamp::parse("2024-01-02T03:04:05").is_err());
    }
}
