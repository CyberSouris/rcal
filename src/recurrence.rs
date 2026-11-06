//! Expansion of recurring (`RRULE`) events into concrete occurrences.
//!
//! rcal stores one database row per recurring series (the series master). The
//! views call [`expand`] to turn that row into the occurrences that fall into
//! the date range they are rendering. Supported rule parts:
//!
//! `FREQ` (DAILY/WEEKLY/MONTHLY/YEARLY), `INTERVAL`, `COUNT`, `UNTIL`,
//! `BYDAY` (including ordinals such as `-1FR`), `BYMONTHDAY`, `BYMONTH`, and
//! `WKST`. Unsupported parts (e.g. `BYSETPOS`, `BYWEEKNO`) are ignored.
//!
//! Expansion works on the stored UTC instants, so a recurring timed event can
//! shift by an hour across a DST transition; the original TZID is not stored.

use chrono::{DateTime, Datelike, Days, Duration, Local, Months, NaiveDate, NaiveTime, Utc, Weekday};
use std::collections::{BTreeSet, HashSet};

use crate::ical::CalendarEvent;

/// Safety cap so a malformed or very old rule cannot make a view hang.
const MAX_OCCURRENCES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug)]
struct Rule {
    freq: Freq,
    interval: u32,
    count: Option<u32>,
    until: Option<DateTime<Utc>>,
    /// `(ordinal, weekday)`; the ordinal is `None` when the rule lists plain
    /// weekdays (e.g. `BYDAY=MO,WE`).
    byday: Vec<(Option<i32>, Weekday)>,
    bymonthday: Vec<i32>,
    bymonth: Vec<u32>,
    wkst: Weekday,
}

impl Rule {
    fn parse(value: &str) -> Option<Self> {
        let mut freq = None;
        let mut interval = 1;
        let mut count = None;
        let mut until = None;
        let mut byday = Vec::new();
        let mut bymonthday = Vec::new();
        let mut bymonth = Vec::new();
        let mut wkst = Weekday::Mon;

        for part in value.split(';') {
            let (key, val) = part.split_once('=')?;
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = match val.trim().to_ascii_uppercase().as_str() {
                        "DAILY" => Some(Freq::Daily),
                        "WEEKLY" => Some(Freq::Weekly),
                        "MONTHLY" => Some(Freq::Monthly),
                        "YEARLY" => Some(Freq::Yearly),
                        _ => return None,
                    };
                }
                "INTERVAL" => interval = val.trim().parse::<u32>().ok()?.max(1),
                "COUNT" => count = val.trim().parse().ok(),
                "UNTIL" => until = parse_until(val.trim()),
                "BYDAY" => {
                    for item in val.split(',') {
                        let spec = parse_byday(item.trim())?;
                        byday.push(spec);
                    }
                }
                "BYMONTHDAY" => {
                    for item in val.split(',') {
                        bymonthday.push(item.trim().parse().ok()?);
                    }
                }
                "BYMONTH" => {
                    for item in val.split(',') {
                        bymonth.push(item.trim().parse().ok()?);
                    }
                }
                "WKST" => wkst = parse_weekday(val.trim())?,
                // Unsupported parts are ignored rather than rejecting the
                // whole series.
                _ => {}
            }
        }

        Some(Rule {
            freq: freq?,
            interval,
            count,
            until,
            byday,
            bymonthday,
            bymonth,
            wkst,
        })
    }
}

/// Parse an RRULE `UNTIL` value. A date-only value includes the whole day.
fn parse_until(value: &str) -> Option<DateTime<Utc>> {
    if value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        let time = NaiveTime::from_hms_opt(23, 59, 59)?;
        return Some(DateTime::from_naive_utc_and_offset(date.and_time(time), Utc));
    }
    crate::ical::parse_ical_datetime(value).ok()
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    match value.to_ascii_uppercase().as_str() {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Parse `MO`, `WE`, or an ordinal form such as `2TU` / `-1FR`.
fn parse_byday(value: &str) -> Option<(Option<i32>, Weekday)> {
    let split = value.find(|c: char| c.is_ascii_alphabetic())?;
    let (ordinal, code) = value.split_at(split);
    let weekday = parse_weekday(code)?;
    let ordinal = if ordinal.is_empty() {
        None
    } else {
        let n: i32 = ordinal.parse().ok()?;
        if n == 0 { None } else { Some(n) }
    };
    Some((ordinal, weekday))
}

/// Expand `event` into occurrences whose covered dates overlap `from..=to`.
///
/// Non-recurring events (or events with an unparseable rule) are returned
/// unchanged. Occurrence copies keep the series UID and metadata but get their
/// own `dtstart`/`dtend` and an empty `exdates` list.
pub fn expand(event: &CalendarEvent, from: NaiveDate, to: NaiveDate) -> Vec<CalendarEvent> {
    let Some(raw_rule) = event.recurrence.as_deref() else {
        return vec![event.clone()];
    };
    let Some(rule) = Rule::parse(raw_rule) else {
        return vec![event.clone()];
    };
    let Some(start) = event.dtstart else {
        return vec![event.clone()];
    };

    let duration = event
        .dtend
        .map(|end| end - start)
        .filter(|d| *d >= Duration::zero())
        .unwrap_or_else(Duration::zero);
    // Occurrences that start before `from` can still reach into the range
    // (multi-day events), so keep generating a little past `to`.
    let span_days = duration.num_days().clamp(0, 3660) as u64;
    let limit = to
        .checked_add_days(Days::new(span_days + 2))
        .unwrap_or(to);

    let dates = occurrence_dates(&rule, start.date_naive(), start.time(), limit);
    let mut result = Vec::new();

    for date in dates {
        let occ_start = DateTime::from_naive_utc_and_offset(date.and_time(start.time()), Utc);
        if event.exdates.contains(&occ_start) {
            continue;
        }

        let occ_end = event.dtend.map(|_| occ_start + duration);
        let (first, last) = covered_dates(occ_start, occ_end, event.all_day);
        if first > to || last < from {
            continue;
        }

        let mut occurrence = event.clone();
        occurrence.dtstart = Some(occ_start);
        occurrence.dtend = occ_end;
        occurrence.exdates.clear();
        result.push(occurrence);
    }

    result
}

/// Generate the occurrence start dates of a rule, starting at the series
/// start and stopping at `limit` (or when `COUNT`/`UNTIL` is exhausted).
fn occurrence_dates(
    rule: &Rule,
    start: NaiveDate,
    start_time: NaiveTime,
    limit: NaiveDate,
) -> Vec<NaiveDate> {
    let mut dates = Vec::new();
    let mut generated: u32 = 0;
    let mut period: u32 = 0;

    'periods: loop {
        let Some((anchor, candidates)) = period_candidates(rule, start, period) else {
            break;
        };
        if anchor > limit {
            break;
        }

        let mut candidates = candidates;
        candidates.sort_unstable();
        candidates.dedup();

        for date in candidates {
            if date < start {
                continue;
            }
            let instant = DateTime::<Utc>::from_naive_utc_and_offset(date.and_time(start_time), Utc);
            if rule.until.is_some_and(|until| instant > until) {
                break 'periods;
            }
            generated += 1;
            if rule.count.is_some_and(|count| generated > count) {
                break 'periods;
            }
            if date > limit {
                break 'periods;
            }
            dates.push(date);
            if dates.len() >= MAX_OCCURRENCES {
                break 'periods;
            }
        }

        period += 1;
    }

    dates
}

/// The anchor date and candidate occurrence dates for one interval period.
fn period_candidates(
    rule: &Rule,
    start: NaiveDate,
    period: u32,
) -> Option<(NaiveDate, Vec<NaiveDate>)> {
    match rule.freq {
        Freq::Daily => {
            let date = start.checked_add_days(Days::new(
                u64::from(period) * u64::from(rule.interval),
            ))?;
            let matches = date_matches_filters(rule, date);
            Some((date, if matches { vec![date] } else { Vec::new() }))
        }
        Freq::Weekly => {
            let first_week = week_start(start, rule.wkst);
            let anchor = first_week.checked_add_days(Days::new(
                u64::from(period) * 7 * u64::from(rule.interval),
            ))?;
            let weekdays: HashSet<Weekday> = if rule.byday.is_empty() {
                std::iter::once(start.weekday()).collect()
            } else {
                rule.byday.iter().map(|(_, weekday)| *weekday).collect()
            };
            let mut dates = Vec::new();
            for offset in 0..7 {
                let date = anchor + Days::new(offset);
                if weekdays.contains(&date.weekday()) && date_matches_filters(rule, date) {
                    dates.push(date);
                }
            }
            Some((anchor, dates))
        }
        Freq::Monthly => {
            let (year, month) = add_months(start.year(), start.month(), period, rule.interval)?;
            let anchor = NaiveDate::from_ymd_opt(year, month, 1)?;
            let mut dates = Vec::new();
            if rule.bymonth.is_empty() || rule.bymonth.contains(&month) {
                dates = month_candidates(year, month, rule, start.day());
            }
            Some((anchor, dates))
        }
        Freq::Yearly => {
            let year = start
                .year()
                .checked_add((period as i32).checked_mul(rule.interval as i32)?)?;
            let anchor = NaiveDate::from_ymd_opt(year, 1, 1)?;
            let mut months: Vec<u32> = if rule.bymonth.is_empty() {
                vec![start.month()]
            } else {
                rule.bymonth.clone()
            };
            months.sort_unstable();
            months.dedup();
            let mut dates = Vec::new();
            for month in months {
                dates.extend(month_candidates(year, month, rule, start.day()));
            }
            Some((anchor, dates))
        }
    }
}

fn add_months(year: i32, month: u32, period: u32, interval: u32) -> Option<(i32, u32)> {
    let index = year as i64 * 12 + (month as i64 - 1) + period as i64 * interval as i64;
    let year = i32::try_from(index.div_euclid(12)).ok()?;
    let month = index.rem_euclid(12) as u32 + 1;
    Some((year, month))
}

fn week_start(date: NaiveDate, wkst: Weekday) -> NaiveDate {
    let delta = (date.weekday().num_days_from_monday() + 7 - wkst.num_days_from_monday()) % 7;
    date - Days::new(u64::from(delta))
}

fn date_matches_filters(rule: &Rule, date: NaiveDate) -> bool {
    (rule.bymonth.is_empty() || rule.bymonth.contains(&date.month()))
        && (rule.bymonthday.is_empty() || bymonthday_matches(date, &rule.bymonthday))
        && (rule.byday.is_empty()
            || rule
                .byday
                .iter()
                .any(|(_, weekday)| *weekday == date.weekday()))
}

fn bymonthday_matches(date: NaiveDate, bymonthday: &[i32]) -> bool {
    let last = last_day_of_month(date.year(), date.month());
    bymonthday
        .iter()
        .filter_map(|day| resolve_month_day(*day, last))
        .any(|day| day == date.day())
}

/// Resolve a `BYMONTHDAY` value (`1`..=`31`, or negative from the end) to a
/// day number in a month with `last` days.
fn resolve_month_day(day: i32, last: u32) -> Option<u32> {
    let resolved = if day > 0 {
        day as u32
    } else if day < 0 {
        (last as i32 + 1 + day) as u32
    } else {
        return None;
    };
    (1..=last).contains(&resolved).then_some(resolved)
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(NaiveDate::MIN);
    (first + Months::new(1) - Days::new(1)).day()
}

/// Days in a given month selected by `BYMONTHDAY` and/or `BYDAY`. When both
/// are present the intersection is used; when neither is present the series
/// start day is repeated (skipping months that are too short).
fn month_candidates(year: i32, month: u32, rule: &Rule, start_day: u32) -> Vec<NaiveDate> {
    let last = last_day_of_month(year, month);
    let mut days: BTreeSet<u32> = BTreeSet::new();

    if !rule.bymonthday.is_empty() {
        for day in &rule.bymonthday {
            if let Some(day) = resolve_month_day(*day, last) {
                days.insert(day);
            }
        }
    }

    if !rule.byday.is_empty() {
        let byday_days = byday_days_in_month(year, month, &rule.byday);
        if rule.bymonthday.is_empty() {
            days.extend(byday_days);
        } else {
            days.retain(|day| byday_days.contains(day));
        }
    }

    if rule.bymonthday.is_empty() && rule.byday.is_empty() && start_day <= last {
        days.insert(start_day);
    }

    days.into_iter()
        .filter_map(|day| NaiveDate::from_ymd_opt(year, month, day))
        .collect()
}

fn byday_days_in_month(year: i32, month: u32, byday: &[(Option<i32>, Weekday)]) -> BTreeSet<u32> {
    let last = last_day_of_month(year, month);
    let mut days = BTreeSet::new();

    for (ordinal, weekday) in byday {
        let matching: Vec<u32> = (1..=last)
            .filter(|day| {
                NaiveDate::from_ymd_opt(year, month, *day)
                    .is_some_and(|date| date.weekday() == *weekday)
            })
            .collect();

        match ordinal {
            None => days.extend(matching),
            Some(n) if *n > 0 => {
                if let Some(day) = matching.get((*n - 1) as usize) {
                    days.insert(*day);
                }
            }
            Some(n) => {
                let from_end = (-*n - 1) as usize;
                if let Some(day) = matching.len().checked_sub(from_end + 1).map(|i| matching[i]) {
                    days.insert(day);
                }
            }
        }
    }

    days
}

/// The first and last local date an occurrence covers.
fn covered_dates(
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
    all_day: bool,
) -> (NaiveDate, NaiveDate) {
    let start_date = if all_day {
        start.date_naive()
    } else {
        start.with_timezone(&Local).date_naive()
    };

    let Some(end) = end else {
        return (start_date, start_date);
    };

    if all_day {
        // DTEND is exclusive for all-day events.
        let last = (end.date_naive() - Days::new(1)).max(start_date);
        return (start_date, last);
    }

    let end_local = end.with_timezone(&Local);
    let end_date = end_local.date_naive();
    let ends_at_midnight = end_local.time() == NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let last = if ends_at_midnight && end_date > start_date {
        end_date - Days::new(1)
    } else {
        end_date
    };
    (start_date, last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ical::parse_ical_datetime;

    fn event(summary: &str, rule: &str) -> CalendarEvent {
        CalendarEvent {
            uid: format!("uid-{summary}"),
            summary: summary.to_string(),
            description: None,
            location: None,
            url: None,
            dtstart: Some(parse_ical_datetime("20240115T090000Z").unwrap()),
            dtend: Some(parse_ical_datetime("20240115T100000Z").unwrap()),
            all_day: false,
            status: None,
            recurrence: Some(rule.to_string()),
            exdates: Vec::new(),
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn occurrences(event: &CalendarEvent, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        expand(event, from, to)
            .iter()
            .filter_map(|event| event.dtstart)
            .map(|start| start.date_naive())
            .collect()
    }

    #[test]
    fn test_weekly_count() {
        let dates = occurrences(
            &event("weekly", "FREQ=WEEKLY;COUNT=4"),
            date(2024, 1, 1),
            date(2024, 2, 29),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 22), date(2024, 1, 29), date(2024, 2, 5)]
        );
    }

    #[test]
    fn test_weekly_interval() {
        let dates = occurrences(
            &event("biweekly", "FREQ=WEEKLY;INTERVAL=2;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 29), date(2024, 2, 12)]
        );
    }

    #[test]
    fn test_weekly_byday() {
        let dates = occurrences(
            &event("mwf", "FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=4"),
            date(2024, 1, 1),
            date(2024, 2, 29),
        );
        assert_eq!(
            dates,
            vec![
                date(2024, 1, 15),
                date(2024, 1, 17),
                date(2024, 1, 19),
                date(2024, 1, 22),
            ]
        );
    }

    #[test]
    fn test_until_is_inclusive() {
        let dates = occurrences(
            &event("until", "FREQ=WEEKLY;UNTIL=20240129T090000Z"),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 22), date(2024, 1, 29)]
        );
    }

    #[test]
    fn test_monthly_bymonthday() {
        let dates = occurrences(
            &event("monthly", "FREQ=MONTHLY;BYMONTHDAY=15;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 6, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 2, 15), date(2024, 3, 15)]
        );
    }

    #[test]
    fn test_monthly_last_friday() {
        let dates = occurrences(
            &event("last-friday", "FREQ=MONTHLY;BYDAY=-1FR;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 6, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 26), date(2024, 2, 23), date(2024, 3, 29)]
        );
    }

    #[test]
    fn test_yearly() {
        let dates = occurrences(
            &event("yearly", "FREQ=YEARLY;COUNT=3"),
            date(2024, 1, 1),
            date(2028, 1, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2025, 1, 15), date(2026, 1, 15)]
        );
    }

    #[test]
    fn test_yearly_bymonth() {
        let dates = occurrences(
            &event("yearly-months", "FREQ=YEARLY;BYMONTH=1,7;COUNT=4"),
            date(2024, 1, 1),
            date(2025, 12, 31),
        );
        assert_eq!(
            dates,
            vec![
                date(2024, 1, 15),
                date(2024, 7, 15),
                date(2025, 1, 15),
                date(2025, 7, 15),
            ]
        );
    }

    #[test]
    fn test_daily_interval() {
        let dates = occurrences(
            &event("daily", "FREQ=DAILY;INTERVAL=3;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 18), date(2024, 1, 21)]
        );
    }

    #[test]
    fn test_exdate_is_excluded() {
        let mut event = event("excluded", "FREQ=WEEKLY;COUNT=4");
        event.exdates = vec![parse_ical_datetime("20240122T090000Z").unwrap()];
        let dates = occurrences(&event, date(2024, 1, 1), date(2024, 3, 1));
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 29), date(2024, 2, 5)]
        );
    }

    #[test]
    fn test_expansion_is_limited_to_requested_range() {
        let dates = occurrences(
            &event("weekly", "FREQ=WEEKLY"),
            date(2024, 1, 29),
            date(2024, 2, 12),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 29), date(2024, 2, 5), date(2024, 2, 12)]
        );
    }

    #[test]
    fn test_multi_day_occurrence_overlapping_range() {
        // A Friday-to-Monday event must still appear when only Sunday is
        // rendered.
        let mut event = event("weekend", "FREQ=WEEKLY");
        event.dtstart = Some(parse_ical_datetime("20240119T180000Z").unwrap());
        event.dtend = Some(parse_ical_datetime("20240122T090000Z").unwrap());
        let dates = occurrences(&event, date(2024, 1, 21), date(2024, 1, 21));
        assert_eq!(dates, vec![date(2024, 1, 19)]);
    }

    #[test]
    fn test_all_day_event_keeps_day_duration() {
        let mut event = event("all-day", "FREQ=DAILY;COUNT=2");
        event.all_day = true;
        event.dtstart = Some(parse_ical_datetime("20240115").unwrap());
        event.dtend = Some(parse_ical_datetime("20240116").unwrap());

        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 16));
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].dtstart, Some(parse_ical_datetime("20240115").unwrap()));
        assert_eq!(expanded[0].dtend, Some(parse_ical_datetime("20240116").unwrap()));
        assert_eq!(expanded[1].dtstart, Some(parse_ical_datetime("20240116").unwrap()));
        assert_eq!(expanded[1].dtend, Some(parse_ical_datetime("20240117").unwrap()));
    }

    #[test]
    fn test_non_recurring_event_is_returned_unchanged() {
        let mut event = event("plain", "FREQ=WEEKLY");
        event.recurrence = None;
        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 15));
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].dtstart, event.dtstart);
    }

    #[test]
    fn test_unparseable_rule_falls_back_to_master() {
        let event = event("broken", "FREQ=SOMETIMES");
        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 15));
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].dtstart, event.dtstart);
    }

    #[test]
    fn test_count_includes_occurrences_before_the_range() {
        // COUNT is counted from the series start, not from the view range.
        let dates = occurrences(
            &event("weekly", "FREQ=WEEKLY;COUNT=3"),
            date(2024, 1, 29),
            date(2024, 3, 1),
        );
        assert_eq!(dates, vec![date(2024, 1, 29)]);
    }

    #[test]
    fn test_monthly_negative_bymonthday() {
        let dates = occurrences(
            &event("last-day", "FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 6, 1),
        );
        // 2024 is a leap year, so February's last day is the 29th.
        assert_eq!(
            dates,
            vec![date(2024, 1, 31), date(2024, 2, 29), date(2024, 3, 31)]
        );
    }

    #[test]
    fn test_monthly_bymonthday_and_byday_intersection() {
        let dates = occurrences(
            &event(
                "intersection",
                "FREQ=MONTHLY;BYMONTHDAY=1,5,10;BYDAY=FR;COUNT=2",
            ),
            date(2024, 1, 1),
            date(2024, 5, 1),
        );
        // Only the 1st/5th/10th that also fall on a Friday are kept; dates
        // before the series start (Jan 15) do not count towards COUNT.
        assert_eq!(dates, vec![date(2024, 3, 1), date(2024, 4, 5)]);
    }

    #[test]
    fn test_monthly_positive_ordinal_byday() {
        let dates = occurrences(
            &event("second-friday", "FREQ=MONTHLY;BYDAY=2FR;COUNT=3"),
            date(2024, 1, 1),
            date(2024, 6, 1),
        );
        // January's second Friday (the 12th) precedes the series start.
        assert_eq!(
            dates,
            vec![date(2024, 2, 9), date(2024, 3, 8), date(2024, 4, 12)]
        );
    }

    #[test]
    fn test_monthly_bymonth_skips_other_months() {
        let dates = occurrences(
            &event("january-only", "FREQ=MONTHLY;BYMONTH=1;COUNT=2"),
            date(2024, 1, 1),
            date(2025, 3, 1),
        );
        assert_eq!(dates, vec![date(2024, 1, 15), date(2025, 1, 15)]);
    }

    #[test]
    fn test_monthly_rolls_over_the_year() {
        let mut event = event("december", "FREQ=MONTHLY;COUNT=3");
        event.dtstart = Some(parse_ical_datetime("20241215T090000Z").unwrap());
        event.dtend = Some(parse_ical_datetime("20241215T100000Z").unwrap());
        let dates = occurrences(&event, date(2024, 12, 1), date(2025, 3, 1));
        assert_eq!(
            dates,
            vec![date(2024, 12, 15), date(2025, 1, 15), date(2025, 2, 15)]
        );
    }

    #[test]
    fn test_weekly_wkst_changes_interval_grouping() {
        let rule = "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,SU;COUNT=4";
        let monday_first = occurrences(
            &event("wkst-monday", &format!("{rule};WKST=MO")),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );
        let sunday_first = occurrences(
            &event("wkst-sunday", &format!("{rule};WKST=SU")),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );

        assert_eq!(
            monday_first,
            vec![
                date(2024, 1, 15),
                date(2024, 1, 21),
                date(2024, 1, 29),
                date(2024, 2, 4),
            ]
        );
        assert_eq!(
            sunday_first,
            vec![
                date(2024, 1, 15),
                date(2024, 1, 28),
                date(2024, 1, 29),
                date(2024, 2, 11),
            ]
        );
    }

    #[test]
    fn test_until_date_only_includes_whole_day() {
        let dates = occurrences(
            &event("until-date", "FREQ=DAILY;UNTIL=20240117"),
            date(2024, 1, 1),
            date(2024, 1, 31),
        );
        assert_eq!(
            dates,
            vec![date(2024, 1, 15), date(2024, 1, 16), date(2024, 1, 17)]
        );
    }

    #[test]
    fn test_parse_weekday_and_byday_helpers() {
        for (code, weekday) in [
            ("MO", Weekday::Mon),
            ("TU", Weekday::Tue),
            ("WE", Weekday::Wed),
            ("TH", Weekday::Thu),
            ("FR", Weekday::Fri),
            ("SA", Weekday::Sat),
            ("SU", Weekday::Sun),
        ] {
            assert_eq!(parse_weekday(code), Some(weekday));
            assert_eq!(parse_weekday(&code.to_ascii_lowercase()), Some(weekday));
            assert_eq!(parse_byday(code), Some((None, weekday)));
        }
        assert_eq!(parse_weekday("XX"), None);
        assert_eq!(parse_byday("0SU"), Some((None, Weekday::Sun)));
        assert_eq!(parse_byday("-1FR"), Some((Some(-1), Weekday::Fri)));
        assert_eq!(parse_byday("XX"), None);
        assert_eq!(parse_byday("-1"), None);
    }

    #[test]
    fn test_rule_parsing_is_case_insensitive_and_ignores_unknown_parts() {
        let dates = occurrences(
            &event(
                "lowercase",
                "freq=weekly;count=2;BYSETPOS=1;byday=mo",
            ),
            date(2024, 1, 1),
            date(2024, 3, 1),
        );
        assert_eq!(dates, vec![date(2024, 1, 15), date(2024, 1, 22)]);
    }

    #[test]
    fn test_recurring_event_without_dtstart_is_returned_unchanged() {
        let mut event = event("no-start", "FREQ=WEEKLY");
        event.dtstart = None;
        event.dtend = None;
        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 15));
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].dtstart, None);
    }

    #[test]
    fn test_occurrence_without_dtend_stays_open_ended() {
        let mut event = event("open", "FREQ=DAILY;COUNT=2");
        event.dtend = None;
        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 16));
        assert_eq!(expanded.len(), 2);
        assert!(expanded.iter().all(|occurrence| occurrence.dtend.is_none()));
    }

    #[test]
    fn test_negative_duration_is_clamped_to_zero() {
        let mut event = event("backwards", "FREQ=DAILY;COUNT=2");
        event.dtstart = Some(parse_ical_datetime("20240115T100000Z").unwrap());
        event.dtend = Some(parse_ical_datetime("20240115T090000Z").unwrap());
        let expanded = expand(&event, date(2024, 1, 15), date(2024, 1, 16));
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].dtend, expanded[0].dtstart);
    }
}
