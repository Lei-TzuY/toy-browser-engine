//! End-to-end runtime coordination for Reporting API delivery and retries.
//!
//! The transport scheduler and retry queue intentionally have separate jobs:
//! one owns browser-generated network requests, while the other owns delayed
//! retry eligibility. This coordinator closes the loop without letting either
//! component steal responsibilities from the browser event loop.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::net::{FetchCompletion, FetchError, FetchId, NetworkBackend};
use crate::reporting_delivery::ReportingDeliveryBatch;
use crate::reporting_retry::{
    ReportingRetryDecision, ReportingRetryPolicy, ReportingRetryQueue,
};
use crate::reporting_scheduler::{ReportingDeliveryOutcome, ReportingDeliveryScheduler};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingRuntimeCompletion {
    pub attempt: u32,
    pub outcome: ReportingDeliveryOutcome,
    pub retry: Option<ReportingRetryDecision>,
}

#[derive(Debug, Clone, Copy)]
struct ReportingAttemptState {
    attempt: u32,
    age_ms: u64,
}

/// Parse an HTTP `Retry-After` delta-seconds value into milliseconds.
pub fn retry_after_delta_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(value.parse::<u64>().ok()?.saturating_mul(1_000))
}

/// Parse either Retry-After form into a delay relative to `now_unix_ms`.
///
/// HTTP-date support is intentionally limited to the IMF-fixdate form emitted
/// by modern HTTP senders. Obsolete RFC 850/asctime forms are rejected rather
/// than guessed. Dates in the past map to a zero delay; the retry policy still
/// applies its local exponential-backoff floor afterwards.
pub fn retry_after_delay_ms(value: &str, now_unix_ms: u64) -> Option<u64> {
    if let Some(delay) = retry_after_delta_ms(value) {
        return Some(delay);
    }

    let target_unix_ms = parse_imf_fixdate_unix_ms(value.trim())?;
    Some(target_unix_ms.saturating_sub(now_unix_ms))
}

fn parse_imf_fixdate_unix_ms(value: &str) -> Option<u64> {
    // IMF-fixdate: Sun, 06 Nov 1994 08:49:37 GMT
    let bytes = value.as_bytes();
    if bytes.len() != 29 || &bytes[3..5] != b", " || &bytes[7..8] != b" "
        || &bytes[11..12] != b" " || &bytes[16..17] != b" "
        || &bytes[19..20] != b":" || &bytes[22..23] != b":"
        || &bytes[25..29] != b" GMT"
    {
        return None;
    }

    let weekday = &value[0..3];
    if !matches!(weekday, "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun") {
        return None;
    }

    let day = value[5..7].parse::<u32>().ok()?;
    let month = match &value[8..11] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = value[12..16].parse::<i32>().ok()?;
    let hour = value[17..19].parse::<u32>().ok()?;
    let minute = value[20..22].parse::<u32>().ok()?;
    let second = value[23..25].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 || !valid_day_of_month(year, month, day) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    let seconds = (days as u64)
        .saturating_mul(86_400)
        .saturating_add((hour as u64) * 3_600)
        .saturating_add((minute as u64) * 60)
        .saturating_add(second as u64);
    Some(seconds.saturating_mul(1_000))
}

fn valid_day_of_month(year: i32, month: u32, day: u32) -> bool {
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max).contains(&day)
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month as i32 + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era as i64) * 146_097 + day_of_era as i64 - 719_468
}

fn wall_clock_now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn completion_retry_after_ms(completion: &FetchCompletion, now_unix_ms: u64) -> Option<u64> {
    let response = completion.result.as_ref().ok()?;
    let value = response.headers.get("retry-after")?;
    retry_after_delay_ms(&value, now_unix_ms)
}

/// Owns the live Reporting API scheduler plus delayed retry state.
#[derive(Debug)]
pub struct ReportingDeliveryRuntime {
    scheduler: ReportingDeliveryScheduler,
    retries: ReportingRetryQueue,
    attempts: HashMap<FetchId, ReportingAttemptState>,
}

impl ReportingDeliveryRuntime {
    pub fn new(policy: ReportingRetryPolicy) -> Self {
        Self {
            scheduler: ReportingDeliveryScheduler::new(),
            retries: ReportingRetryQueue::new(policy),
            attempts: HashMap::new(),
        }
    }

    pub fn with_in_flight_limit(mut self, limit: usize) -> Self {
        self.scheduler = self.scheduler.with_limit(limit);
        self
    }

    pub fn in_flight_len(&self) -> usize {
        self.scheduler.len()
    }

    pub fn retry_len(&self) -> usize {
        self.retries.len()
    }

    pub fn is_idle(&self) -> bool {
        self.scheduler.is_empty() && self.retries.is_empty()
    }

    /// Queue a newly-created delivery batch as attempt 1.
    pub fn queue_initial(
        &mut self,
        batch: ReportingDeliveryBatch,
        age_ms: u64,
        user_agent: &str,
    ) -> Result<FetchId, FetchError> {
        let id = self.scheduler.queue(batch, age_ms, user_agent)?;
        self.attempts.insert(id, ReportingAttemptState { attempt: 1, age_ms });
        Ok(id)
    }

    /// Move eligible retries back into the transport scheduler without
    /// exceeding its in-flight limit.
    ///
    /// The caller may supply its current report age, but a retry always uses at
    /// least the minimum age carried by the retry entry. This makes age
    /// monotonic across delivery attempts even if the caller accidentally
    /// reuses a stale age value.
    pub fn queue_ready_retries(
        &mut self,
        now_ms: u64,
        age_ms: u64,
        user_agent: &str,
    ) -> Vec<(FetchId, u32)> {
        let capacity = self.scheduler.limit().saturating_sub(self.scheduler.len());
        let ready = self.retries.drain_ready_up_to(now_ms, capacity);
        let mut queued = Vec::with_capacity(ready.len());

        for entry in ready {
            let retry_age_ms = age_ms.max(entry.minimum_age_ms);
            let id = self
                .scheduler
                .queue(entry.batch, retry_age_ms, user_agent)
                .expect("bounded Reporting API retry must fit scheduler capacity");
            self.attempts.insert(
                id,
                ReportingAttemptState {
                    attempt: entry.attempt,
                    age_ms: retry_age_ms,
                },
            );
            queued.push((id, entry.attempt));
        }
        queued
    }

    pub fn dispatch(&mut self, network: &dyn NetworkBackend) -> usize {
        self.scheduler.dispatch(network)
    }

    pub fn process_completions(
        &mut self,
        completions: Vec<FetchCompletion>,
        now_ms: u64,
    ) -> (Vec<ReportingRuntimeCompletion>, Vec<FetchCompletion>) {
        self.process_completions_at(completions, now_ms, wall_clock_now_unix_ms())
    }

    /// Deterministic form of [`Self::process_completions`] for embedders that
    /// already own a wall clock and for tests. `now_ms` remains the monotonic
    /// scheduler clock; `now_unix_ms` is used only to interpret HTTP-date
    /// Retry-After values.
    pub fn process_completions_at(
        &mut self,
        completions: Vec<FetchCompletion>,
        now_ms: u64,
        now_unix_ms: u64,
    ) -> (Vec<ReportingRuntimeCompletion>, Vec<FetchCompletion>) {
        let retry_after_by_id: HashMap<FetchId, u64> = completions
            .iter()
            .filter_map(|completion| {
                completion_retry_after_ms(completion, now_unix_ms)
                    .map(|delay| (completion.id, delay))
            })
            .collect();

        let (outcomes, unhandled) = self.scheduler.process_completions(completions);
        let mut completed = Vec::with_capacity(outcomes.len());

        for outcome in outcomes {
            let id = match &outcome {
                ReportingDeliveryOutcome::Delivered { id, .. }
                | ReportingDeliveryOutcome::Retryable { id, .. } => *id,
            };
            let state = self.attempts.remove(&id).unwrap_or(ReportingAttemptState {
                attempt: 1,
                age_ms: 0,
            });
            let retry = match &outcome {
                ReportingDeliveryOutcome::Delivered { .. } => None,
                ReportingDeliveryOutcome::Retryable { batch, .. } => Some(
                    self.retries.schedule_failure_with_age_and_minimum_delay(
                        batch.clone(),
                        state.attempt,
                        now_ms,
                        state.age_ms,
                        retry_after_by_id.get(&id).copied().unwrap_or(0),
                    ),
                ),
            };
            completed.push(ReportingRuntimeCompletion {
                attempt: state.attempt,
                outcome,
                retry,
            });
        }

        (completed, unhandled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};
    use crate::net::{FetchResponse, ManualNetwork, Url};
    use crate::reporting_endpoints::ResolvedIntegrityViolationReport;
    use crate::reporting_scheduler::ReportingDeliveryFailure;

    fn batch(endpoint: &str) -> ReportingDeliveryBatch {
        let endpoint_url = Url::parse(endpoint).unwrap();
        ReportingDeliveryBatch {
            endpoint_url: endpoint_url.clone(),
            reports: vec![ResolvedIntegrityViolationReport {
                endpoint_name: "default".into(),
                endpoint_url,
                report: IntegrityViolationReport {
                    report_type: "integrity-violation",
                    endpoint: "default".into(),
                    body: IntegrityViolationReportBody {
                        document_url: "https://example.test/page".into(),
                        blocked_url: "https://cdn.test/app.js".into(),
                        destination: "script".into(),
                        report_only: false,
                    },
                },
            }],
        }
    }

    #[test]
    fn failed_delivery_reenters_scheduler_after_backoff() {
        let network = ManualNetwork::new();
        network.respond_with("https://reports.test/collect", 503, "text/plain", Vec::new());
        network.set_auto_complete(true);

        let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 1000, 3));
        runtime
            .queue_initial(batch("https://reports.test/collect"), 0, "ua")
            .unwrap();
        runtime.dispatch(&network);
        let (completed, unhandled) = runtime.process_completions(network.poll(), 1_000);
        assert!(unhandled.is_empty());
        assert_eq!(completed[0].attempt, 1);
        assert!(matches!(
            completed[0].outcome,
            ReportingDeliveryOutcome::Retryable {
                failure: ReportingDeliveryFailure::HttpStatus(503),
                ..
            }
        ));
        assert_eq!(runtime.retry_len(), 1);
        assert!(runtime.queue_ready_retries(1_099, 0, "ua").is_empty());

        let queued = runtime.queue_ready_retries(1_100, 0, "ua");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1, 2);
        assert_eq!(runtime.retry_len(), 0);
    }

    #[test]
    fn retry_after_delta_parser_is_strict_and_saturating() {
        assert_eq!(retry_after_delta_ms("7"), Some(7_000));
        assert_eq!(retry_after_delta_ms(" 12 "), Some(12_000));
        assert_eq!(retry_after_delta_ms("1.5"), None);
        assert_eq!(retry_after_delta_ms(""), None);
        assert_eq!(retry_after_delta_ms(&u64::MAX.to_string()), Some(u64::MAX));
    }

    #[test]
    fn retry_after_http_date_parser_handles_future_past_and_invalid_dates() {
        let base = parse_imf_fixdate_unix_ms("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
        assert_eq!(retry_after_delay_ms("Sun, 06 Nov 1994 08:49:40 GMT", base), Some(3_000));
        assert_eq!(retry_after_delay_ms("Sun, 06 Nov 1994 08:49:30 GMT", base), Some(0));
        assert_eq!(retry_after_delay_ms("Sun, 31 Feb 1994 08:49:40 GMT", base), None);
        assert_eq!(retry_after_delay_ms("Sunday, 06-Nov-94 08:49:40 GMT", base), None);
    }

    #[test]
    fn retry_after_extends_retry_eligibility() {
        let network = ManualNetwork::new();
        let url = Url::parse("https://reports.test/collect").unwrap();
        let mut response = FetchResponse::synthetic(url, 503, Some("text/plain"), Vec::new());
        response.headers.insert_raw("retry-after", "3");
        network.respond("https://reports.test/collect", response);
        network.set_auto_complete(true);

        let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 3));
        runtime
            .queue_initial(batch("https://reports.test/collect"), 250, "ua")
            .unwrap();
        runtime.dispatch(&network);
        runtime.process_completions(network.poll(), 1_000);

        assert!(runtime.queue_ready_retries(3_999, 0, "ua").is_empty());
        let queued = runtime.queue_ready_retries(4_000, 0, "ua");
        assert_eq!(queued.len(), 1);
        runtime.dispatch(&network);
        let requests = network.requests();
        let retry_body = String::from_utf8(requests[1].body.clone().unwrap()).unwrap();
        assert!(retry_body.contains("\"age\":3250"), "{retry_body}");
    }

    #[test]
    fn retry_after_http_date_extends_retry_eligibility() {
        let network = ManualNetwork::new();
        let url = Url::parse("https://reports.test/collect").unwrap();
        let mut response = FetchResponse::synthetic(url, 503, Some("text/plain"), Vec::new());
        response
            .headers
            .insert_raw("retry-after", "Sun, 06 Nov 1994 08:49:40 GMT");
        network.respond("https://reports.test/collect", response);
        network.set_auto_complete(true);

        let wall_now = parse_imf_fixdate_unix_ms("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
        let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 3));
        runtime
            .queue_initial(batch("https://reports.test/collect"), 250, "ua")
            .unwrap();
        runtime.dispatch(&network);
        runtime.process_completions_at(network.poll(), 1_000, wall_now);

        assert!(runtime.queue_ready_retries(3_999, 0, "ua").is_empty());
        assert_eq!(runtime.queue_ready_retries(4_000, 0, "ua").len(), 1);
    }
}
