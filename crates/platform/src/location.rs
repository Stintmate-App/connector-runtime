//! Where the connector's data lives: `<workspace>\connectors\<id>\`.
//!
//! The data folder is resolved once per process, at start, through the workspace resolver. With
//! no workspace chosen it is an error (`workspace_missing`, detail `no workspace chosen`): the
//! connector keeps no data next to its code. A process that starts another connector process
//! passes [`Location::child_args`], so parent and child never disagree.

use crate::spec::ConnectorSpec;
use crate::workspace::{self, Code, Probes, Source, WorkspaceError};
use std::path::PathBuf;

/// Exit status of an entry point whose workspace failed to resolve (nothing written).
pub const WORKSPACE_ERROR_EXIT: i32 = 3;
/// The resolved data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// `<workspace>\connectors\<id>`.
    pub data_dir: PathBuf,
    /// Where the workspace came from.
    pub source: Source,
    /// The workspace folder.
    pub workspace: PathBuf,
    /// Inside a cloud-synced folder (allowed).
    pub synced: bool,
}

impl Location {
    /// Arguments for a child connector process: the resolved workspace, explicitly.
    pub fn child_args(&self) -> Vec<String> {
        let mut args = vec![
            "--workspace".to_string(),
            self.workspace.display().to_string(),
        ];
        if self.synced {
            args.push("--allow-synced".into());
        }
        args
    }

    /// One line for the log and the control app.
    pub fn describe(&self) -> String {
        format!(
            "workspace: {} (source {})",
            self.workspace.display(),
            self.source.as_str()
        )
    }

    /// Log the location once at start (plus a warning when synced).
    pub fn log(&self) {
        tracing::info!("{}", self.describe());
        if self.synced {
            tracing::warn!(
                "workspace {} is inside a cloud-synced folder (allowed): sync clients may lock or duplicate files",
                self.workspace.display()
            );
        }
    }
}

/// Resolve the data folder of the connector `spec`. Nothing is created.
pub fn resolve(
    spec: &ConnectorSpec,
    workspace_arg: Option<&str>,
    allow_synced: bool,
    probes: &dyn Probes,
) -> Result<Location, WorkspaceError> {
    let res = workspace::resolve(workspace_arg, allow_synced, probes)?;
    let Some(path) = res.path else {
        return Err(WorkspaceError::new(
            Code::WorkspaceMissing,
            Source::Pointer,
            "no workspace chosen",
        ));
    };
    let workspace = PathBuf::from(path);
    Ok(Location {
        data_dir: workspace.join("connectors").join(spec.id),
        source: res.source,
        workspace,
        synced: res.synced,
    })
}

/// The command-line options every connector process takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceArgs {
    /// `--workspace DIR` / `--workspace=DIR`.
    pub workspace: Option<String>,
    /// `--allow-synced`.
    pub allow_synced: bool,
    /// Every other argument, in order.
    pub rest: Vec<String>,
}

impl WorkspaceArgs {
    /// Split `--workspace` and `--allow-synced` from the other arguments (program name
    /// excluded). A trailing `--workspace` with no value is kept in `rest`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut out = Self::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            if a == "--workspace" {
                match it.next() {
                    Some(v) => out.workspace = Some(v),
                    None => out.rest.push(a),
                }
            } else if let Some(v) = a.strip_prefix("--workspace=") {
                out.workspace = Some(v.to_string());
            } else if a == "--allow-synced" {
                out.allow_synced = true;
            } else {
                out.rest.push(a);
            }
        }
        out
    }
}
