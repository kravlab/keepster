//! `explain`: whether a path is captured, and which rule at which level decided it.

use axum::Json;
use axum::extract::{Path, Query, State};
use keepster_api::{
    ConfigLevel, ConfigWarning, Decision, Explanation, Origin, Problem, RuleEffect, RuleReference,
};
use keepster_app::config::{self, OnDiskKind};
use keepster_core::{Kind, Level, Rule, RuleOrigin, Verdict};

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};
use crate::snapshots::parse_path;

/// Which path to explain.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ExplainParameters {
    /// The path, relative to the Root and `/`-separated. It need not exist.
    path: Option<String>,
}

/// Whether a path is captured, and which rule at which level decided it.
///
/// The answer is computed from the configuration and the Working Tree as they are now — the same
/// inputs the next capture reads — so it is the answer to "would the next Snapshot hold this file". An
/// excluded ancestor is named as such, because pruning wins over negation and a person reading only
/// the path's own rule would conclude the opposite (ADR 0007).
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/explain",
    operation_id = "explain_path",
    tag = "rules",
    params(RootAddress, ExplainParameters),
    responses(
        (status = 200, description = "Whether the path is captured, and why.", body = Explanation),
        (status = 400, description = "The path is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or its configuration cannot be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn explain(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    Query(parameters): Query<ExplainParameters>,
) -> Result<Json<Explanation>, ProblemResponse> {
    let id = roots::parse_id(&id)?;
    let path = parse_path(parameters.path.as_deref().unwrap_or_default())?;
    // Resolved while the Registry is held, as everywhere: resolving may write into the Root's store
    // directory, which an erasing deregistration must not be removing at the same moment.
    let (root, loaded) = {
        let registry = state.registry();
        let root = roots::record(&registry, id)?;
        let loaded = state.effective(id, &root.path)?;
        (root, loaded)
    };

    let policy = config::policy_for_path(&loaded.policy, &root.path, &path);
    let on_disk = config::observe(&root.path, &path);
    let kind = match on_disk.kind {
        Some(OnDiskKind::Directory) => Kind::Directory,
        // A path that is not there is explained as a file: it has no kind to read, and a file is what
        // a person asking about one usually means.
        _ => Kind::File,
    };
    let explained = policy.explain(&path, kind, on_disk.size);

    let withheld = |reason: &str| (Decision::Withheld, Some(reason.to_owned()));
    let ((decision, reason), rule, pruned_by) = match &explained.verdict {
        // The rules would let it in, but a walk never reads a pipe or a device: saying "captured" here
        // would be the one answer `explain` exists to prevent. A link is captured — as a link, or
        // followed — like a file (ticket 49).
        Verdict::Captured if on_disk.kind == Some(OnDiskKind::Special) => {
            (withheld("special_file"), None, None)
        }
        Verdict::Captured => (
            (Decision::Captured, None),
            explained.opted_in.or(explained.own_rule),
            None,
        ),
        Verdict::Reserved => (withheld("reserved_name"), None, None),
        Verdict::Fenced(reason) => (withheld(reason.as_str()), None, None),
        Verdict::Excluded(rule) => ((Decision::Excluded, None), Some(*rule), None),
        Verdict::Pruned { ancestor, rule } => (
            (Decision::Pruned, None),
            Some(*rule),
            Some(ancestor.as_str().to_owned()),
        ),
        Verdict::Secret(rule) => ((Decision::Secret, None), Some(*rule), None),
        Verdict::TooLarge { rule, .. } => (withheld("too_large"), *rule, None),
    };

    Ok(Json(Explanation {
        path: path.as_str().to_owned(),
        captured: decision == Decision::Captured,
        decision,
        reason,
        rule: rule.map(reference),
        pruned_by,
        own_rule: explained.own_rule.map(reference),
        on_disk: on_disk.kind.map(|kind| match kind {
            OnDiskKind::File => keepster_api::OnDiskKind::File,
            OnDiskKind::Directory => keepster_api::OnDiskKind::Dir,
            OnDiskKind::Symlink => keepster_api::OnDiskKind::Symlink,
            OnDiskKind::Special => keepster_api::OnDiskKind::Special,
        }),
        size: on_disk.size,
        max_size: explained.max_size,
        max_size_origin: origin(explained.max_size_origin),
        warnings: loaded.warnings.into_iter().map(warning).collect(),
    }))
}

/// A rule as `explain` names it.
pub(crate) fn reference(rule: &Rule) -> RuleReference {
    RuleReference {
        pattern: rule.text.clone(),
        effect: if rule.negated {
            RuleEffect::Include
        } else {
            RuleEffect::Exclude
        },
        origin: origin(&rule.origin),
    }
}

/// A configuration warning as the wire spells it.
pub(crate) fn warning(warning: keepster_app::config::ConfigWarning) -> ConfigWarning {
    ConfigWarning {
        code: warning.code.as_str().to_owned(),
        message: warning.message,
    }
}

/// Where a rule or a value was set, as the wire spells it.
pub(crate) fn origin(origin: &RuleOrigin) -> Origin {
    Origin {
        level: match origin.level {
            Level::Default => ConfigLevel::Default,
            Level::IgnoreFile => ConfigLevel::IgnoreFile,
            Level::Global => ConfigLevel::Global,
            Level::GlobalPattern => ConfigLevel::GlobalPattern,
            Level::GlobalRoot => ConfigLevel::GlobalRoot,
            Level::Root => ConfigLevel::Root,
            Level::Forget => ConfigLevel::Forget,
        },
        source: origin.source.clone(),
        line: origin.line,
    }
}
