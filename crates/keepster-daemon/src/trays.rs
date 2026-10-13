//! The trays attached (ticket 67): each reports what it can do — draw its icon, show notifications — and
//! `status` lists the reports, so that a tray that cannot reach the person is seen rather than assumed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use keepster_api::{ErrorCode, Problem, Tray, TrayReport};

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// How long a report stands without being renewed. A tray renews it every 30 seconds, so a tray that
/// died — which cannot say so — drops out of `status` after three missed reports.
pub(crate) const EXPIRY: Duration = Duration::from_secs(90);

/// The longest instance name a tray reports under.
const LONGEST_INSTANCE: usize = 64;

/// The reports standing, by instance. Pure: the caller hands in the time.
#[derive(Debug, Default)]
pub(crate) struct Trays {
    /// Each instance's last report, with when it arrived.
    reports: BTreeMap<String, (TrayReport, Instant)>,
}

impl Trays {
    /// `instance` reports `report` at `at`, replacing what it reported before.
    pub(crate) fn report(&mut self, instance: &str, report: TrayReport, at: Instant) {
        self.reports.insert(instance.to_owned(), (report, at));
    }

    /// `instance` is leaving. Withdrawing what was never reported is no error.
    pub(crate) fn withdraw(&mut self, instance: &str) {
        self.reports.remove(instance);
    }

    /// Every report still standing at `at`, by instance; the expired ones are forgotten.
    pub(crate) fn current(&mut self, at: Instant) -> Vec<Tray> {
        self.reports
            .retain(|_, (_, reported)| at.saturating_duration_since(*reported) <= EXPIRY);
        self.reports
            .iter()
            .map(|(instance, (report, _))| Tray {
                instance: instance.clone(),
                report: report.clone(),
            })
            .collect()
    }
}

/// Whether `instance` is a name a tray may report under: 1 to 64 letters, digits, `.`, `_` or `-`, so
/// that it reads safely wherever `status` prints it.
pub(crate) fn valid_instance(instance: &str) -> bool {
    (1..=LONGEST_INSTANCE).contains(&instance.len())
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Report what a tray can do, renewing its report.
#[utoipa::path(
    put,
    path = "/v1/trays/{instance}",
    operation_id = "report_tray",
    tag = "status",
    params(("instance" = String, Path, description = "The name the tray reports under, the same for as long as it runs: 1 to 64 letters, digits, `.`, `_` or `-`.")),
    request_body = TrayReport,
    responses(
        (status = 204, description = "Recorded: `status` lists it under `trays` for 90 seconds, so a tray renews it every 30, and a tray that died drops out by itself."),
        (status = 400, description = "An instance that is not a plain name (`tray_invalid`), or a body that is not this JSON (`body_invalid`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn report(
    State(state): State<DaemonState>,
    Path(instance): Path<String>,
    body: Result<Json<TrayReport>, JsonRejection>,
) -> Result<StatusCode, ProblemResponse> {
    let instance = checked(instance)?;
    let Json(report) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;

    state.trays().report(&instance, report, Instant::now());

    Ok(StatusCode::NO_CONTENT)
}

/// A tray is leaving: withdraw its report.
#[utoipa::path(
    delete,
    path = "/v1/trays/{instance}",
    operation_id = "withdraw_tray",
    tag = "status",
    params(("instance" = String, Path, description = "The name the tray reported under.")),
    responses(
        (status = 204, description = "Withdrawn. Withdrawing a report never made, or one already expired, is no error."),
        (status = 400, description = "An instance that is not a plain name (`tray_invalid`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn withdraw(
    State(state): State<DaemonState>,
    Path(instance): Path<String>,
) -> Result<StatusCode, ProblemResponse> {
    let instance = checked(instance)?;

    state.trays().withdraw(&instance);

    Ok(StatusCode::NO_CONTENT)
}

/// `instance`, or the refusal of one that is not a plain name.
fn checked(instance: String) -> Result<String, ProblemResponse> {
    if valid_instance(&instance) {
        return Ok(instance);
    }

    Err(ProblemResponse::new(
        StatusCode::BAD_REQUEST,
        ErrorCode::TrayInvalid,
        "a tray's instance is 1 to 64 letters, digits, `.`, `_` or `-`",
    )
    .with_detail("instance", instance.as_str()))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use keepster_api::{Reported, Tray, TrayReport};

    use super::{EXPIRY, Trays, valid_instance};

    fn report(notifications: Reported) -> TrayReport {
        TrayReport {
            icon: Reported::Available,
            icon_reason: None,
            notifications,
            notifications_reason: None,
        }
    }

    fn tray(instance: &str, report: TrayReport) -> Tray {
        Tray {
            instance: instance.to_owned(),
            report,
        }
    }

    #[test]
    fn a_report_is_listed_by_its_instance_and_a_later_one_replaces_it() {
        let start = Instant::now();
        let mut trays = Trays::default();

        trays.report("b", report(Reported::Available), start);
        trays.report("a", report(Reported::Available), start);
        trays.report("a", report(Reported::Unavailable), start);

        assert_eq!(
            trays.current(start),
            [
                tray("a", report(Reported::Unavailable)),
                tray("b", report(Reported::Available)),
            ]
        );
    }

    #[test]
    fn a_report_not_renewed_for_ninety_seconds_is_forgotten() {
        let start = Instant::now();
        let mut trays = Trays::default();
        trays.report("a", report(Reported::Available), start);
        trays.report(
            "b",
            report(Reported::Available),
            start + Duration::from_secs(30),
        );

        assert_eq!(trays.current(start + EXPIRY).len(), 2);
        assert_eq!(
            trays.current(start + EXPIRY + Duration::from_secs(1)),
            [tray("b", report(Reported::Available))]
        );
    }

    #[test]
    fn a_withdrawn_tray_is_gone_and_withdrawing_an_unknown_one_is_no_error() {
        let start = Instant::now();
        let mut trays = Trays::default();
        trays.report("a", report(Reported::Available), start);

        trays.withdraw("a");
        trays.withdraw("never");

        assert_eq!(trays.current(start), []);
    }

    #[test]
    fn an_instance_is_a_short_plain_name() {
        for valid in ["1234", "tray-01J9.x_y", &"a".repeat(64)] {
            assert!(valid_instance(valid), "{valid}");
        }
        for invalid in ["", "a b", "a/b", "ä", &"a".repeat(65)] {
            assert!(!valid_instance(invalid), "{invalid}");
        }
    }
}
