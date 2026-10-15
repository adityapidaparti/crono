//! Core domain types: meals, amounts, and day specifiers.
//!
//! Everything here is pure and total — parsers return errors, never panic —
//! and is covered by property tests in `tests/properties.rs`.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Diary group a food entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Meal {
    Breakfast,
    Lunch,
    Dinner,
    Snacks,
    Uncategorized,
}

impl Meal {
    pub const ALL: [Meal; 5] = [
        Meal::Breakfast,
        Meal::Lunch,
        Meal::Dinner,
        Meal::Snacks,
        Meal::Uncategorized,
    ];

    /// Cronometer diary group number: 0 = uncategorized/auto, 1–4 = B/L/D/S.
    pub fn group_number(&self) -> u32 {
        match self {
            Meal::Uncategorized => 0,
            Meal::Breakfast => 1,
            Meal::Lunch => 2,
            Meal::Dinner => 3,
            Meal::Snacks => 4,
        }
    }

    pub fn from_group_number(n: u32) -> Meal {
        match n {
            1 => Meal::Breakfast,
            2 => Meal::Lunch,
            3 => Meal::Dinner,
            4 => Meal::Snacks,
            _ => Meal::Uncategorized,
        }
    }

    /// The mobile API packs the diary group into the serving's `order` field:
    /// `order = (group << 16) | 1`.
    pub fn pack_order(&self) -> i64 {
        ((self.group_number() as i64) << 16) | 1
    }

    pub fn from_order(order: i64) -> Meal {
        Meal::from_group_number(((order >> 16) & 0xFFFF) as u32)
    }

    /// The app's "auto" meal resolution by local hour: 04–09 breakfast,
    /// 10–13 lunch, 14–20 dinner, otherwise snacks.
    pub fn from_hour(hour: u32) -> Meal {
        match hour {
            4..=9 => Meal::Breakfast,
            10..=13 => Meal::Lunch,
            14..=20 => Meal::Dinner,
            _ => Meal::Snacks,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Meal::Breakfast => "breakfast",
            Meal::Lunch => "lunch",
            Meal::Dinner => "dinner",
            Meal::Snacks => "snacks",
            Meal::Uncategorized => "uncategorized",
        }
    }
}

impl fmt::Display for Meal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Meal {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "b" | "breakfast" => Ok(Meal::Breakfast),
            "l" | "lunch" => Ok(Meal::Lunch),
            "d" | "dinner" => Ok(Meal::Dinner),
            "s" | "snack" | "snacks" => Ok(Meal::Snacks),
            "u" | "none" | "uncategorized" => Ok(Meal::Uncategorized),
            other => Err(format!(
                "unknown meal '{other}' (expected breakfast|lunch|dinner|snacks|uncategorized or b|l|d|s|u)"
            )),
        }
    }
}

/// A quantity of food: a number plus an optional unit word.
///
/// The unit is matched against the food's available measures at log time;
/// `None` means "use the food's default measure".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Amount {
    pub value: f64,
    pub unit: Option<String>,
}

impl Amount {
    pub fn new(value: f64, unit: Option<&str>) -> Self {
        Amount {
            value,
            unit: unit.map(|u| u.trim().to_ascii_lowercase()),
        }
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.unit {
            Some(u) => write!(f, "{} {}", self.value, u),
            None => write!(f, "{}", self.value),
        }
    }
}

impl FromStr for Amount {
    type Err = String;

    /// Accepts `"120g"`, `"1.5 cup"`, `"5 oz"`, `"2"`, `"0.5 sandwich"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty amount".into());
        }
        let num_end = s
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
            .map(|(i, c)| i + c.len_utf8())
            .last()
            .ok_or_else(|| format!("amount '{s}' must start with a number"))?;
        let (num, rest) = s.split_at(num_end);
        let value: f64 = num
            .parse()
            .map_err(|_| format!("bad number '{num}' in amount '{s}'"))?;
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("amount must be a positive finite number, got '{num}'"));
        }
        let unit = rest.trim();
        Ok(Amount::new(value, (!unit.is_empty()).then_some(unit)))
    }
}

/// A day specifier: `today`, `yesterday`, `tomorrow`, `-N` (N days ago),
/// or an ISO date `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaySpec {
    Today,
    Offset(i64),
    Date(NaiveDate),
}

impl DaySpec {
    /// Resolve against a reference "today" (injected for testability).
    pub fn resolve(&self, today: NaiveDate) -> NaiveDate {
        match self {
            DaySpec::Today => today,
            DaySpec::Offset(n) => today + Duration::days(*n),
            DaySpec::Date(d) => *d,
        }
    }
}

impl FromStr for DaySpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "today" | "" => return Ok(DaySpec::Today),
            "yesterday" => return Ok(DaySpec::Offset(-1)),
            "tomorrow" => return Ok(DaySpec::Offset(1)),
            _ => {}
        }
        if let Some(rest) = s.strip_prefix('-') {
            if let Ok(n) = rest.parse::<i64>() {
                return Ok(DaySpec::Offset(-n));
            }
        }
        NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(DaySpec::Date)
            .map_err(|_| format!("unknown day '{s}' (expected today|yesterday|tomorrow|-N|YYYY-MM-DD)"))
    }
}

/// Format a date the way the mobile API expects: `YYYY-M-D`, month and day
/// NOT zero-padded (e.g. `2026-8-5`).
pub fn cron_day(d: NaiveDate) -> String {
    format!("{}-{}-{}", d.year(), d.month(), d.day())
}

/// Format a time the way the mobile API expects: `H:M:S`, non-padded.
pub fn cron_time(t: NaiveTime) -> String {
    format!("{}:{}:{}", t.hour(), t.minute(), t.second())
}

/// Parse a date from an API response. Responses are inconsistent: requests
/// use non-padded `YYYY-M-D` but some responses return zero-padded ISO, so
/// accept both.
pub fn parse_cron_day(s: &str) -> Option<NaiveDate> {
    let mut parts = s.trim().splitn(3, '-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

impl fmt::Display for DaySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DaySpec::Today => f.write_str("today"),
            DaySpec::Offset(-1) => f.write_str("yesterday"),
            DaySpec::Offset(1) => f.write_str("tomorrow"),
            DaySpec::Offset(n) if *n < 0 => write!(f, "-{}", -n),
            DaySpec::Offset(n) => write!(f, "+{n}"),
            DaySpec::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
        }
    }
}
