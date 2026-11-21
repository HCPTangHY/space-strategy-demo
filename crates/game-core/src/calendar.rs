//! Simulation calendar, speed tiers, combat subtick marker, and tick step counters.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const SUBTICKS_PER_DAY: u64 = 10;
pub const DAYS_PER_MONTH: u64 = 30;
pub const MONTHS_PER_YEAR: u64 = 12;
pub const DAYS_PER_YEAR: u64 = DAYS_PER_MONTH * MONTHS_PER_YEAR; // 360

/// Scenario calendar definition. Internal simulation only stores elapsed subticks from zero;
/// display dates are derived from this configuration and skip year 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarConfig {
    pub epoch_name: String,
    pub start_year: i32,
}

impl CalendarConfig {
    pub fn new(epoch_name: impl Into<String>, start_year: i32) -> Result<Self, &'static str> {
        if start_year == 0 {
            return Err("历法起始年份不能为 0（无公元 0 年）。");
        }
        Ok(Self {
            epoch_name: epoch_name.into(),
            start_year,
        })
    }
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            epoch_name: "星历".into(),
            start_year: 1,
        }
    }
}

/// Speed tiers: Pause (0) + 1x..4x.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum GameSpeed {
    Paused = 0,
    Speed1 = 1,
    Speed2 = 2,
    Speed3 = 3,
    Speed4 = 4,
}

impl GameSpeed {
    pub fn day_interval(self) -> Option<Duration> {
        match self {
            Self::Paused => None,
            Self::Speed1 => Some(Duration::from_millis(2000)),
            Self::Speed2 => Some(Duration::from_millis(1000)),
            Self::Speed3 => Some(Duration::from_millis(250)),
            // ponytail: 60 days/sec ceiling prevents empty worlds from skipping millennia in 1s;
            // once late-game daily simulation exceeds 16.67ms, it degrades naturally to zero-delay.
            Self::Speed4 => Some(Duration::from_nanos(16_666_667)),
        }
    }
}

impl From<GameSpeed> for u8 {
    fn from(speed: GameSpeed) -> Self {
        speed as u8
    }
}

impl TryFrom<u8> for GameSpeed {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Paused),
            1 => Ok(Self::Speed1),
            2 => Ok(Self::Speed2),
            3 => Ok(Self::Speed3),
            4 => Ok(Self::Speed4),
            other => Err(format!("无效的速度档位：{other}（仅支持 0..=4）")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InCombat {
    pub subticks_fought: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickCounters {
    pub combat_subticks: u64,
    pub daily_ticks: u64,
    pub monthly_ticks: u64,
    pub yearly_ticks: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepOutcome {
    pub day_advanced: bool,
    pub month_advanced: bool,
    pub year_advanced: bool,
}
