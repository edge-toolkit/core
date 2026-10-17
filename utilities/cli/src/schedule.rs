//! When a scheduled agent's runner starts, as a scenario states it and as each deployment format is held to it.
//!
//! A schedule turns an agent from a long-running peer into a job: its runner is started on each tick and runs its
//! module to completion, which is what a runner already does on its own. Three formats carry it -- a Kubernetes
//! `CronJob`, an Ofelia job beside the compose stack, and process-compose's own `schedule:` -- and all three parse
//! the expression with the same robfig cron grammar. Validation therefore admits only what that grammar accepts, so an
//! expression cannot pass here and then be rejected, or read differently, by one of them.

use croner::parser::{CronParser, Seconds, Year};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::error::CliError;
use crate::input::{ClusterInput, OutputType};

/// The `@` shorthands robfig's standard parser expands, and so the only ones every scheduler here understands.
///
/// `@every <duration>` is left out on purpose: Kubernetes rejects it, so it has no `CronJob` to render as.
const MACROS: [&str; 7] = [
    "@yearly",
    "@annually",
    "@monthly",
    "@weekly",
    "@daily",
    "@midnight",
    "@hourly",
];

/// Month names a month field may use in place of its number.
const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Weekday names a day-of-week field may use in place of its number.
const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Index of the month field in a five-field expression.
const MONTH_FIELD: usize = 3;

/// Index of the day-of-week field in a five-field expression.
const WEEKDAY_FIELD: usize = 4;

/// The highest day of week robfig's grammar takes, Saturday.
const MAX_WEEKDAY: u8 = 6;

/// When an agent's runner is started, from its `schedule:`.
///
/// Unset -- an empty `cron` -- is an agent that runs continuously, which is every agent a scenario wrote before
/// this field existed. Concurrency is not configurable: a tick that arrives while the previous run is still going is
/// skipped, which is the one policy every scheduler here can honour.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Schedule {
    /// A five-field cron expression, or one of the `@hourly`-style shorthands, from `cron:`.
    #[serde(deserialize_with = "written_cron")]
    pub cron: String,
    /// The IANA time zone the expression is read in, from `time_zone:`; empty reads it in the host's own zone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time_zone: String,
}

impl Schedule {
    /// Whether the agent has no schedule, and so runs continuously.
    #[must_use]
    pub const fn is_unset(&self) -> bool {
        self.cron.is_empty()
    }
}

/// Read a `cron:` that has to say something, since a `schedule:` written out with an empty one is a mistake.
///
/// Refused here rather than read as empty, because empty is how [`Schedule::is_unset`] tells an agent with no
/// `schedule:` at all, which runs continuously -- an agent meant to run on a schedule would otherwise run nonstop.
fn written_cron<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let cron = String::deserialize(deserializer)?;
    if cron.trim().is_empty() {
        return Err(de::Error::custom(
            "`cron` is empty; leave `schedule:` out for an agent that runs continuously",
        ));
    }
    Ok(cron)
}

/// Reject a schedule that some scheduler here would refuse or read differently, naming the agent and why.
pub(crate) fn validate_schedule(agent: &str, schedule: &Schedule) -> Result<(), CliError> {
    let invalid = |reason: String| invalid_schedule(agent, schedule, reason);
    let cron = schedule.cron.trim();
    if cron.starts_with('@') {
        if !MACROS.contains(&cron) {
            return Err(invalid(format!(
                "the only shorthands accepted are {}",
                MACROS.join(", ")
            )));
        }
    } else {
        let fields: Vec<&str> = cron.split_whitespace().collect();
        if fields.len() != WEEKDAY_FIELD + 1 {
            return Err(invalid(format!(
                "it has {} fields, and a schedule takes exactly five: minute, hour, day of month, month, day of week",
                fields.len()
            )));
        }
        for (index, field) in fields.iter().enumerate() {
            standard_field(agent, schedule, index, field)?;
        }
    }
    let parser = CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build();
    if let Err(error) = parser.parse(cron) {
        return Err(invalid(error.to_string()));
    }
    if schedule.time_zone.is_empty() {
        return Ok(());
    }
    // The zone database matches names without regard to case, but the schedulers handed the name read it as a file
    // path -- Kubernetes among them -- so a name spelled any other way than the database's own is refused too.
    let Ok(zone) = jiff::tz::TimeZone::get(&schedule.time_zone) else {
        return Err(CliError::InvalidScheduleTimeZone {
            agent: agent.to_owned(),
            time_zone: schedule.time_zone.clone(),
        });
    };
    match zone.iana_name() {
        Some(canonical) if canonical != schedule.time_zone => Err(CliError::NonCanonicalScheduleTimeZone {
            agent: agent.to_owned(),
            time_zone: schedule.time_zone.clone(),
            canonical: canonical.to_owned(),
        }),
        _ => Ok(()),
    }
}

/// Render how the compose deployment runs its scheduled agents, or nothing for any other format or an unscheduled one.
///
/// Compose is the one format whose schedule differs visibly from a `CronJob`, and the one that asks for a privilege, so
/// both are said where the operator starting it will read them.
pub(crate) fn schedule_note(output_type: OutputType, cluster: &ClusterInput) -> &'static str {
    let scheduled = cluster.agents.iter().any(|agent| !agent.schedule.is_unset());
    if !scheduled || !matches!(output_type, OutputType::DockerCompose) {
        return "";
    }
    concat!(
        "\n### Scheduled Agents With Docker Compose\n\n",
        "`docker compose up` runs each scheduled agent's runner once, as it does every service. The\n",
        "`scheduler` service, Ofelia, then starts that same container again on each tick of its schedule,\n",
        "skipping a tick while the previous run is still going. It mounts the Docker socket to do so, which\n",
        "gives it the Docker daemon's privileges.\n"
    )
}

/// Check one field uses only the standard grammar: numbers, `*`, ranges, steps, lists, and names where they belong.
///
/// The cron parser also takes Quartz's `L`, `W`, `#` and `?`, which robfig's standard parser does not, so a field using
/// them would generate a `CronJob` the API server rejects. It also reads a day of week of `7` as Sunday, where robfig's
/// days run `0` to `6`; a step after `/` is a count rather than a day, so `*/7` is left alone.
fn standard_field(agent: &str, schedule: &Schedule, index: usize, field: &str) -> Result<(), CliError> {
    let names: &[&str] = match index {
        MONTH_FIELD => &MONTH_NAMES,
        WEEKDAY_FIELD => &WEEKDAY_NAMES,
        _ => &[],
    };
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    for item in field.split(',') {
        // An item without a step steps by one, which needs no checking.
        let (range, step) = item.split_once('/').unwrap_or((item, "1"));
        let mut segments: Vec<&str> = range.split('-').collect();
        segments.push(step);
        for (position, part) in segments.iter().enumerate() {
            let is_step = position == segments.len().saturating_sub(1);
            let is_name = !is_step && names.contains(&part.to_ascii_uppercase().as_str());
            if !(digits(part) || is_name || (*part == "*" && !is_step)) {
                let reason = format!("{part:?} is not a number, `*` or a name this field accepts");
                return Err(invalid_schedule(agent, schedule, reason));
            }
            if index == WEEKDAY_FIELD && !is_step && part.parse::<u8>().is_ok_and(|day| day > MAX_WEEKDAY) {
                let reason = format!("day of week {part} is past {MAX_WEEKDAY}; Sunday is 0");
                return Err(invalid_schedule(agent, schedule, reason));
            }
        }
    }
    Ok(())
}

/// The error rejecting `agent`'s `schedule` for `reason`.
fn invalid_schedule(agent: &str, schedule: &Schedule, reason: String) -> CliError {
    CliError::InvalidSchedule {
        agent: agent.to_owned(),
        value: schedule.cron.clone(),
        reason,
    }
}
