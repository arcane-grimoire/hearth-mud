//! In-world game clock.
//!
//! The framework keeps a monotonic `game_minute` counter (advanced on the tick
//! heartbeat) and, given a per-game [`ClockConfig`], decomposes it into a
//! calendar [`GameTime`]. Policy — how fast time flows, the calendar shape,
//! month/day names, the epoch — is entirely game config; the framework only
//! keeps the count, exposes it to softcode via `get_time()`, fires the
//! rollover hooks (`on_hour`/`on_day`/`on_dawn`/`on_dusk`), and backs the
//! `game_time_between()` lock predicate. Absent config = no clock, and nothing
//! about the engine changes.

use serde::Deserialize;

/// Per-game clock configuration (`[clock]` in `hearth.toml`). Every field has
/// a default, so `[clock]` with no keys yields a sane 24/30/12 calendar at
/// 1 game-minute per tick.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClockConfig {
    /// Game minutes advanced per tick. May be fractional (e.g. `0.5`); the
    /// engine accumulates the remainder so slow rates still advance.
    pub minutes_per_tick: f64,
    pub hours_per_day: u32,
    pub days_per_month: u32,
    pub months_per_year: u32,
    /// Hour at which `on_dawn` fires and `is_day` becomes true.
    pub dawn_hour: u32,
    /// Hour at which `on_dusk` fires and `is_day` becomes false.
    pub dusk_hour: u32,
    /// Optional month names (index 0 = month 1). Absent → months are numbers.
    pub month_names: Vec<String>,
    /// Optional weekday cycle names. Absent → no weekday in `get_time()`.
    pub day_names: Vec<String>,
    /// The wall-time the counter starts from (displayed time = start + counter).
    pub start: ClockStart,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClockStart {
    pub year: u64,
    pub month: u64,
    pub day: u64,
    pub hour: u64,
    pub minute: u64,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            minutes_per_tick: 1.0,
            hours_per_day: 24,
            days_per_month: 30,
            months_per_year: 12,
            dawn_hour: 6,
            dusk_hour: 20,
            month_names: Vec::new(),
            day_names: Vec::new(),
            start: ClockStart::default(),
        }
    }
}

impl Default for ClockStart {
    fn default() -> Self {
        // Year 1, Month 1, Day 1 at 00:00 — the natural "beginning of time".
        Self { year: 1, month: 1, day: 1, hour: 0, minute: 0 }
    }
}

/// A decomposed in-world time. `year`/`month`/`day` are 1-based (as displayed);
/// `hour`/`minute` are 0-based. `total_minutes` is the absolute minute count
/// from the calendar origin (start included), monotonic across the whole run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameTime {
    pub total_minutes: u64,
    pub minute: u32,
    pub hour: u32,
    pub day: u64,
    pub month: u64,
    pub year: u64,
    /// Absolute day index from the origin — used for `on_day` rollover
    /// detection (unambiguous across month/year boundaries).
    pub abs_day: u64,
    pub weekday: Option<u32>,
    pub month_name: Option<String>,
    pub is_day: bool,
}

impl ClockConfig {
    fn minutes_per_day(&self) -> u64 {
        self.hours_per_day as u64 * 60
    }
    fn minutes_per_month(&self) -> u64 {
        self.days_per_month as u64 * self.minutes_per_day()
    }
    fn minutes_per_year(&self) -> u64 {
        self.months_per_year as u64 * self.minutes_per_month()
    }

    /// Absolute minute offset of the configured `start` epoch.
    fn epoch_minutes(&self) -> u64 {
        self.start.year.saturating_sub(1) * self.minutes_per_year()
            + self.start.month.saturating_sub(1) * self.minutes_per_month()
            + self.start.day.saturating_sub(1) * self.minutes_per_day()
            + self.start.hour * 60
            + self.start.minute
    }

    /// Decompose the `game_minute` counter (minutes since boot/epoch) into a
    /// full calendar time.
    pub fn at(&self, game_minute: u64) -> GameTime {
        let mpd = self.minutes_per_day().max(1);
        let total = self.epoch_minutes() + game_minute;

        let minute = (total % 60) as u32;
        let hour = ((total / 60) % self.hours_per_day.max(1) as u64) as u32;
        let abs_day = total / mpd;
        let day = abs_day % self.days_per_month.max(1) as u64 + 1;
        let abs_month = abs_day / self.days_per_month.max(1) as u64;
        let month = abs_month % self.months_per_year.max(1) as u64 + 1;
        let year = abs_month / self.months_per_year.max(1) as u64 + 1;

        let weekday = if self.day_names.is_empty() {
            None
        } else {
            Some((abs_day % self.day_names.len() as u64) as u32)
        };
        let month_name = self
            .month_names
            .get((month - 1) as usize)
            .cloned();

        // Daytime window. `dawn <= hour < dusk` normally; when dawn >= dusk the
        // window is treated as wrapping past midnight.
        let is_day = if self.dawn_hour <= self.dusk_hour {
            hour >= self.dawn_hour && hour < self.dusk_hour
        } else {
            hour >= self.dawn_hour || hour < self.dusk_hour
        };

        GameTime {
            total_minutes: total,
            minute,
            hour,
            day,
            month,
            year,
            abs_day,
            weekday,
            month_name,
            is_day,
        }
    }

    /// The `get_time()` softcode table for a given counter value.
    pub fn to_json(&self, game_minute: u64) -> serde_json::Value {
        let t = self.at(game_minute);
        let mut obj = serde_json::json!({
            "total_minutes": t.total_minutes,
            "minute": t.minute,
            "hour": t.hour,
            "day": t.day,
            "month": t.month,
            "year": t.year,
            "is_day": t.is_day,
        });
        if let Some(w) = t.weekday {
            obj["weekday"] = serde_json::json!(w);
            if let Some(name) = self.day_names.get(w as usize) {
                obj["day_name"] = serde_json::json!(name);
            }
        }
        if let Some(name) = t.month_name {
            obj["month_name"] = serde_json::json!(name);
        }
        obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ClockConfig {
        ClockConfig::default()
    }

    #[test]
    fn minute_hour_day_rollover() {
        let c = cfg(); // start year1/month1/day1 00:00, 24/30/12
        let t0 = c.at(0);
        assert_eq!((t0.year, t0.month, t0.day, t0.hour, t0.minute), (1, 1, 1, 0, 0));
        assert_eq!(c.at(59).hour, 0);
        assert_eq!(c.at(60).hour, 1); // an hour in
        let t = c.at(90); // 1h30m
        assert_eq!((t.hour, t.minute), (1, 30));
        let d = c.at(24 * 60); // one full day
        assert_eq!((d.day, d.hour), (2, 0));
    }

    #[test]
    fn month_and_year_rollover() {
        let c = cfg();
        let m = c.at(30 * 24 * 60); // 30 days = one month
        assert_eq!((m.year, m.month, m.day), (1, 2, 1));
        let y = c.at(12 * 30 * 24 * 60); // 12 months = one year
        assert_eq!((y.year, y.month, y.day), (2, 1, 1));
    }

    #[test]
    fn is_day_tracks_dawn_dusk() {
        let c = cfg(); // dawn 6, dusk 20
        assert!(!c.at(5 * 60).is_day); // 05:00
        assert!(c.at(6 * 60).is_day); // 06:00 dawn
        assert!(c.at(12 * 60).is_day); // noon
        assert!(!c.at(20 * 60).is_day); // 20:00 dusk
        assert!(!c.at(23 * 60).is_day);
    }

    #[test]
    fn epoch_start_offsets_the_clock() {
        let mut c = cfg();
        c.start = ClockStart { year: 2, month: 3, day: 4, hour: 6, minute: 0 };
        let t = c.at(0);
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute), (2, 3, 4, 6, 0));
        // 60 minutes later it's 07:00 same day.
        assert_eq!(c.at(60).hour, 7);
    }

    #[test]
    fn weekday_and_month_names_optional() {
        let mut c = cfg();
        assert!(c.at(0).weekday.is_none());
        assert!(c.at(0).month_name.is_none());
        c.day_names = vec!["A".into(), "B".into(), "C".into()];
        c.month_names = vec!["Jan".into(), "Feb".into()];
        assert_eq!(c.at(0).weekday, Some(0));
        assert_eq!(c.at(24 * 60).weekday, Some(1)); // next day
        assert_eq!(c.at(0).month_name.as_deref(), Some("Jan"));
    }

    #[test]
    fn to_json_shape() {
        let c = cfg();
        let j = c.to_json(6 * 60 + 30); // 06:30
        assert_eq!(j["hour"], 6);
        assert_eq!(j["minute"], 30);
        assert_eq!(j["is_day"], true);
        assert!(j.get("weekday").is_none()); // no day_names configured
    }
}
