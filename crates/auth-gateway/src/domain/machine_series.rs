//! The machines' charts (decision 0072, point 13, step 7): series read from
//! Mimir for Settings → Machines, never stored here and never used to
//! decide. A range is one of three, each with its step, so a series has at
//! most 168 points; the window ends on a step, so that the same minute
//! asks the same question. The use case is `application`'s; the port is
//! here, beside the machines', so that `infrastructure` implements it.

use std::time::{SystemTime, UNIX_EPOCH};

/// The charts' ranges: `1h`, `24h` and `7d` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeriesRange {
    Hour,
    Day,
    Week,
}

impl SeriesRange {
    pub const ALL: [Self; 3] = [Self::Hour, Self::Day, Self::Week];

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }

    /// Seconds between two points: 1 min, 10 min, 1 h.
    pub fn step(self) -> i64 {
        match self {
            Self::Hour => 60,
            Self::Day => 600,
            Self::Week => 3600,
        }
    }

    /// Points a series has at most: 60, 144, 168.
    pub fn points(self) -> usize {
        match self {
            Self::Hour => 60,
            Self::Day => 144,
            Self::Week => 168,
        }
    }

    /// The window ending at the last step at or before `now`.
    pub fn window(self, now: SystemTime) -> SeriesWindow {
        let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
        let step = self.step();
        let end = secs - secs.rem_euclid(step);
        SeriesWindow {
            range: self,
            start: end - (self.points() as i64 - 1) * step,
            end,
        }
    }
}

/// Where a chart's points fall: `start + k × step` up to `end`, Unix
/// seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesWindow {
    pub range: SeriesRange,
    pub start: i64,
    pub end: i64,
}

impl SeriesWindow {
    /// The point of the time `t`, if it is one of the window's.
    pub fn index(&self, t: i64) -> Option<usize> {
        let step = self.range.step();
        let k = (t - self.start).checked_div(step)?;
        let on_step = (t - self.start).rem_euclid(step) == 0;
        (on_step && t >= self.start && t <= self.end).then_some(k as usize)
    }
}

/// One line: a value for each point of the window, `None` where Mimir had
/// none (a gap is missing points, never a value).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The machine (`vm`).
    pub vm: String,
    /// What else names the line: a filesystem's watched path, or a
    /// network's direction (`in`, `out`); empty for CPU and memory.
    pub label: String,
    pub values: Vec<Option<f64>>,
}

/// The five answers, for every machine at once.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineCharts {
    pub window: SeriesWindow,
    /// CPU used, %.
    pub cpu: Vec<Line>,
    /// Memory used, %.
    pub memory: Vec<Line>,
    /// Each watched filesystem's used %, labelled by its path's name.
    pub filesystems: Vec<Line>,
    /// Bytes a second, `in` and `out`.
    pub network: Vec<Line>,
    /// Each machine's last host sample in the range, Unix seconds.
    pub last_sample: Vec<(String, i64)>,
}

/// Mimir did not answer, or not in time, or not as Prometheus does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesUnavailable(pub String);

/// Port: the machines' series, as Mimir holds them.
#[async_trait::async_trait]
pub trait MachineSeries: Send + Sync {
    /// The charts of `range`, its window ending at or before `now`.
    async fn read(
        &self,
        range: SeriesRange,
        now: SystemTime,
    ) -> Result<MachineCharts, SeriesUnavailable>;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_range_is_one_of_three_with_its_step() {
        for r in SeriesRange::ALL {
            assert_eq!(SeriesRange::parse(r.as_str()), Some(r));
        }
        for other in ["", "2h", "24H", "30d", "1h ", "15m"] {
            assert_eq!(SeriesRange::parse(other), None, "{other}");
        }
        let steps: Vec<_> = SeriesRange::ALL.map(|r| (r.step(), r.points())).into();
        assert_eq!(steps, [(60, 60), (600, 144), (3600, 168)]);
    }

    #[test]
    fn a_window_ends_on_its_step_and_holds_its_points() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_123);
        let w = SeriesRange::Day.window(now);
        assert_eq!(w.end, 1_699_999_800);
        assert_eq!((w.end - w.start) / 600 + 1, 144);
        assert_eq!(w.index(w.start), Some(0));
        assert_eq!(w.index(w.end), Some(143));
        assert_eq!(w.index(w.start + 300), None, "between two steps");
        assert_eq!(w.index(w.start - 600), None);
        assert_eq!(w.index(w.end + 600), None);
        // Within the same step, the same window.
        let later = SeriesRange::Day.window(now + Duration::from_secs(60));
        assert_eq!(later, w);
    }
}
