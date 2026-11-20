//! Schedules: when a schedule fires next.
//!
//! - [`next_fire`] / [`next_schedule_fire`]: 5-field cron, or a [`ScheduleTrigger`].
//! - [`ScheduleTrigger`]: Windows-Task-Scheduler-style recurrence.

mod schedule;
mod trigger;

pub use schedule::*;
pub use trigger::*;
