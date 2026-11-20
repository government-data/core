//! How long each stage of an evaluation takes (decision 0031's time per
//! stage, the run log of decision 0026): each lap the time since the one
//! before, in the order the stages ran.

use std::time::Instant;

use quality::worker_api::StageTime;

pub(crate) struct Stages {
    last: Instant,
    times: Vec<StageTime>,
}

impl Stages {
    pub(crate) fn start() -> Self {
        Self {
            last: Instant::now(),
            times: Vec::new(),
        }
    }

    /// Ends `stage` now; the next one starts.
    pub(crate) fn lap(&mut self, stage: &str) {
        let now = Instant::now();
        let ms = i64::try_from(now.duration_since(self.last).as_millis()).unwrap_or(i64::MAX);
        self.times.push(StageTime {
            stage: stage.to_string(),
            ms,
        });
        self.last = now;
    }

    pub(crate) fn done(self) -> Vec<StageTime> {
        self.times
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_lap_is_a_stage_in_the_order_they_ran() {
        let mut stages = Stages::start();
        stages.lap("load");
        std::thread::sleep(std::time::Duration::from_millis(5));
        stages.lap("rules");
        let times = stages.done();
        let names: Vec<&str> = times.iter().map(|t| t.stage.as_str()).collect();
        assert_eq!(names, ["load", "rules"]);
        assert!(times[1].ms >= 5, "{times:?}");
        assert!(times.iter().all(|t| t.ms >= 0));
    }
}
