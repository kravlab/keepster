//! The directories on the daemon's machine, listed so that a web client can draw a picker for where an
//! Export writes (ticket 58, ticket 19): a browser can name no path of its own.
//!
//! Not confined to any subtree: a client holding the API's credential can already register `/` and read
//! the disk through `entries`, so a limit here would protect nothing and only tax an external volume.
//! Directories only, never followed through a link, and no sizes.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::Query;
use axum::extract::rejection::QueryRejection;
use axum::http::StatusCode;
use keepster_api::{DirectoryListing, ErrorCode, Problem, Subdirectory};

use crate::problem::ProblemResponse;

/// Which directory.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct FsParameters {
    /// The directory, absolute; the home directory when absent.
    path: Option<String>,
}

/// List the directories in one directory.
#[utoipa::path(
    get,
    path = "/v1/fs",
    operation_id = "list_directories",
    tag = "roots",
    params(FsParameters),
    responses(
        (status = 200, description = "Every directory in it, by name, hidden ones marked so a picker can leave them out until asked, and each said to be listable or not. Files, links and sizes are left out.", body = DirectoryListing),
        (status = 400, description = "`path` is not absolute.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "Nothing is at `path`.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "`path` is not a directory (`not_a_directory`), or cannot be listed (`directory_unreadable`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn list(
    parameters: Result<Query<FsParameters>, QueryRejection>,
) -> Result<Json<DirectoryListing>, ProblemResponse> {
    let Query(parameters) = parameters.map_err(|rejection| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            rejection.body_text(),
        )
    })?;
    let path = match parameters.path.as_deref() {
        Some(path) => crate::roots::absolute_query_path(path)?,
        None => std::env::home_dir().ok_or_else(|| {
            ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::PathInvalid,
                "there is no home directory to start from; give `path`",
            )
        })?,
    };

    tokio::task::spawn_blocking(move || listing(&path))
        .await
        .map_err(|error| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::DirectoryUnreadable,
                error.to_string(),
            )
        })?
        .map(Json)
}

/// The directories in `path`.
fn listing(path: &Path) -> Result<DirectoryListing, ProblemResponse> {
    let detail = |problem: ProblemResponse| problem.with_detail("path", path.to_string_lossy());
    let metadata = std::fs::metadata(path).map_err(|error| {
        detail(ProblemResponse::new(
            StatusCode::NOT_FOUND,
            ErrorCode::PathNotFound,
            format!("{}: {error}", path.display()),
        ))
    })?;
    if !metadata.is_dir() {
        return Err(detail(ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::NotADirectory,
            format!("{} is not a directory", path.display()),
        )));
    }
    let entries = std::fs::read_dir(path).map_err(|error| {
        detail(ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::DirectoryUnreadable,
            format!("{} cannot be listed: {error}", path.display()),
        ))
    })?;

    let mut directories: Vec<Subdirectory> = entries
        .filter_map(Result::ok)
        // Not followed: a link is not a directory here, whatever it leads to.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            // A name the wire cannot carry could not be asked for again either.
            let name = entry.file_name().into_string().ok()?;
            let full = entry.path();
            Some(Subdirectory {
                hidden: hidden(&name, &entry),
                readable: std::fs::read_dir(&full).is_ok(),
                path: full.to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();
    directories.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(DirectoryListing {
        path: path.to_string_lossy().into_owned(),
        parent: path
            .parent()
            .map(PathBuf::from)
            .map(|parent| parent.to_string_lossy().into_owned()),
        directories,
    })
}

/// Whether a directory is hidden: a dot name anywhere, and the hidden attribute on Windows.
fn hidden(name: &str, entry: &std::fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if entry
            .metadata()
            .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = entry;

    name.starts_with('.')
}
