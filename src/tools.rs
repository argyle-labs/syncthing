//! syncthing tool surface.
//!
//!   - `syncthing.ignores.check_pbs`  whether a folder's ignore patterns exclude PBS datastore files
//!   - `syncthing.ignore_perms.sync`  set a folder's `ignorePerms` so chmod failures stop blocking sync
//!   - `syncthing.folders.health`     folders in error or stuck scanning

use plugin_toolkit::client::Client;
use plugin_toolkit::prelude::*;

use crate::api;
use crate::execute;
use crate::ignores::{self, Violation};

#[orca_struct(args)]
pub struct IgnoresCheckPbsArgs {
    /// Syncthing member (host) whose REST API is read.
    #[arg(long)]
    pub member: String,
    /// Folder id holding a PBS datastore.
    #[arg(long)]
    pub folder: String,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoresCheckPbs {
    pub folder: String,
    /// Ignore lines as Syncthing reports them, `#include`s expanded.
    pub patterns: Vec<String>,
    /// Whether every PBS datastore path replicates.
    pub ok: bool,
    pub violations: Vec<Violation>,
}

/// `/rest/db/ignores` — `ignore` is the raw `.stignore`, `expanded` has
/// `#include`s resolved.
#[derive(Debug, serde::Deserialize)]
struct Ignores {
    #[serde(default)]
    ignore: Option<Vec<String>>,
    #[serde(default)]
    expanded: Option<Vec<String>>,
}

/// Unreserved characters pass; everything else is percent-encoded.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}

fn ignores_path(folder: &str) -> String {
    format!("/rest/db/ignores?folder={}", encode(folder))
}

fn evaluate(folder: &str, ig: Ignores) -> IgnoresCheckPbs {
    // `#escape` lives only in the raw file; keep it ahead of the expanded lines.
    let escape: Vec<String> = ig
        .ignore
        .iter()
        .flatten()
        .filter(|l| l.trim_start().starts_with("#escape"))
        .cloned()
        .collect();
    let patterns: Vec<String> = escape
        .into_iter()
        .chain(ig.expanded.or(ig.ignore).unwrap_or_default())
        .collect();
    let violations = ignores::pbs_violations(&patterns.join("\n"));
    IgnoresCheckPbs {
        folder: folder.to_string(),
        ok: violations.is_empty(),
        patterns,
        violations,
    }
}

pub fn check_pbs(base: &str, key: &str, folder: &str) -> Result<IgnoresCheckPbs> {
    let ig: Ignores = api::get_json(&Client::new(), base, Some(key), &ignores_path(folder))
        .map_err(|e| anyhow!(e))?;
    Ok(evaluate(folder, ig))
}

/// Check that a PBS datastore folder's ignore patterns let chunks, indexes,
/// blobs and owner files replicate; a replica missing them cannot restore.
/// Read-only.
#[orca_tool(domain = "syncthing", verb = "ignores.check_pbs", role = "any")]
async fn syncthing_ignores_check_pbs(
    args: IgnoresCheckPbsArgs,
    _ctx: &ToolCtx,
) -> Result<IgnoresCheckPbs> {
    let base = api::base_url(&args.member).map_err(|e| anyhow!(e))?;
    let key = api::api_key(&args.member).map_err(|e| anyhow!(e))?;
    check_pbs(&base, key.expose_secret(), &args.folder)
}

#[orca_struct(args)]
pub struct IgnorePermsSyncArgs {
    /// Syncthing member (host) whose REST API is used.
    #[arg(long)]
    pub member: String,
    /// Folder id whose backing path cannot `chmod` (SMB, NFS with
    /// root_squash, Unraid shfs).
    #[arg(long)]
    pub folder: String,
    /// Report drift and change nothing.
    #[arg(long)]
    #[serde(default)]
    pub dry_run: bool,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnorePermsSync {
    pub folder: String,
    pub path: String,
    /// `ignorePerms` before this call.
    pub ignore_perms: bool,
    /// Current folder errors that are permission-setting failures.
    pub chmod_errors: Vec<String>,
    /// True only when this call wrote the folder config; a dry run never does.
    pub changed: bool,
    pub dry_run: bool,
}

/// `/rest/config/folders/{id}`, the fields read here.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderConfig {
    #[serde(default)]
    path: String,
    #[serde(default)]
    ignore_perms: bool,
}

/// `/rest/folder/errors`.
#[derive(Debug, serde::Deserialize)]
struct FolderErrors {
    #[serde(default)]
    errors: Option<Vec<FolderError>>,
}

#[derive(Debug, serde::Deserialize)]
struct FolderError {
    #[serde(default)]
    path: String,
    #[serde(default)]
    error: String,
}

/// Errors from Syncthing applying file modes on a filesystem that refuses
/// `chmod`; the folder retries with growing backoff and never converges.
fn chmod_errors(errs: FolderErrors) -> Vec<String> {
    errs.errors
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            let msg = e.error.to_ascii_lowercase();
            (msg.contains("chmod") || msg.contains("setting permissions"))
                && msg.contains("operation not permitted")
        })
        .map(|e| format!("{}: {}", e.path, e.error))
        .collect()
}

fn folder_config_path(folder: &str) -> String {
    format!("/rest/config/folders/{}", encode(folder))
}

pub fn ignore_perms_sync(
    base: &str,
    key: &str,
    folder: &str,
    dry_run: bool,
) -> Result<IgnorePermsSync> {
    let client = Client::new();
    let cfg: FolderConfig = api::get_json(&client, base, Some(key), &folder_config_path(folder))
        .map_err(|e| anyhow!(e))?;
    let errs: FolderErrors = api::get_json(
        &client,
        base,
        Some(key),
        &format!("/rest/folder/errors?folder={}", encode(folder)),
    )
    .map_err(|e| anyhow!(e))?;
    let changed = !dry_run && !cfg.ignore_perms;
    if changed {
        api::send_json(
            &client,
            "PATCH",
            base,
            key,
            &folder_config_path(folder),
            &plugin_toolkit::serde_json::json!({ "ignorePerms": true }),
        )
        .map_err(|e| anyhow!(e))?;
        let after: FolderConfig =
            api::get_json(&client, base, Some(key), &folder_config_path(folder))
                .map_err(|e| anyhow!(e))?;
        if !after.ignore_perms {
            bail!("syncthing did not take ignorePerms on folder {folder}");
        }
    }
    Ok(IgnorePermsSync {
        folder: folder.to_string(),
        path: cfg.path,
        ignore_perms: cfg.ignore_perms,
        chmod_errors: chmod_errors(errs),
        changed,
        dry_run,
    })
}

/// **Remediation.** Turn on a folder's Ignore Permissions through the REST
/// config API. Syncthing applies file modes on sync; on a share that refuses
/// `chmod` every such item fails and the folder never converges. Reports the
/// folder's current chmod failures either way. Idempotent; writes only when
/// `ignorePerms` is off, then reads it back.
#[orca_tool(
    domain = "syncthing",
    verb = "ignore_perms.sync",
    role = "admin",
    execute_gated = false
)]
async fn syncthing_ignore_perms_sync(
    args: IgnorePermsSyncArgs,
    ctx: &ToolCtx,
) -> Result<IgnorePermsSync> {
    execute::require_admin("syncthing.ignore_perms.sync", ctx)?;
    let base = api::base_url(&args.member).map_err(|e| anyhow!(e))?;
    let key = api::api_key(&args.member).map_err(|e| anyhow!(e))?;
    ignore_perms_sync(&base, key.expose_secret(), &args.folder, args.dry_run)
}

#[orca_struct(args)]
pub struct FoldersHealthArgs {
    /// Syncthing member (host) whose REST API is read.
    #[arg(long)]
    pub member: String,
    /// Seconds a folder may sit in a scanning state before it counts as stalled.
    #[arg(long, default_value_t = 3600)]
    #[serde(default = "default_scan_stall_secs")]
    pub scan_stall_secs: i64,
}

fn default_scan_stall_secs() -> i64 {
    3600
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderHealth {
    pub folder: String,
    pub label: String,
    pub state: String,
    pub state_age_secs: Option<i64>,
    pub error: Option<String>,
    pub errors: i64,
    pub pull_errors: i64,
    pub problems: Vec<String>,
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldersHealth {
    pub member: String,
    pub ok: bool,
    pub folders: Vec<FolderHealth>,
}

#[derive(Debug, serde::Deserialize)]
struct FolderRef {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    paused: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DbStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    state_changed: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    errors: i64,
    #[serde(default)]
    pull_errors: i64,
}

const SCAN_STATES: [&str; 2] = ["scanning", "scan-waiting"];

fn assess_folder(f: &FolderRef, st: DbStatus, now: i64, scan_stall_secs: i64) -> FolderHealth {
    let state_age_secs = st
        .state_changed
        .as_deref()
        .and_then(|t| plugin_toolkit::time::Timestamp::parse_rfc3339(t).ok())
        .map(|t| now - t.unix_seconds());
    let error = st.error.filter(|e| !e.is_empty());
    let mut problems = Vec::new();
    if st.state == "error" || error.is_some() {
        problems.push(format!(
            "folder in error: {}",
            error.as_deref().unwrap_or(&st.state)
        ));
    }
    if SCAN_STATES.contains(&st.state.as_str())
        && state_age_secs.is_some_and(|a| a > scan_stall_secs)
    {
        problems.push(format!(
            "{} for {}s (over {scan_stall_secs}s)",
            st.state,
            state_age_secs.unwrap_or_default()
        ));
    }
    if st.pull_errors > 0 {
        problems.push(format!("{} pull error(s)", st.pull_errors));
    }
    FolderHealth {
        folder: f.id.clone(),
        label: f.label.clone(),
        state: st.state,
        state_age_secs,
        error,
        errors: st.errors,
        pull_errors: st.pull_errors,
        problems,
    }
}

pub fn folders_health(
    member: &str,
    base: &str,
    key: &str,
    scan_stall_secs: i64,
) -> Result<FoldersHealth> {
    let client = Client::new();
    let folders: Vec<FolderRef> =
        api::get_json(&client, base, Some(key), "/rest/config/folders").map_err(|e| anyhow!(e))?;
    let now = plugin_toolkit::time::now().unix_seconds();
    let mut out = Vec::new();
    for f in folders.iter().filter(|f| !f.paused) {
        let st: DbStatus = api::get_json(
            &client,
            base,
            Some(key),
            &format!("/rest/db/status?folder={}", encode(&f.id)),
        )
        .map_err(|e| anyhow!(e))?;
        out.push(assess_folder(f, st, now, scan_stall_secs));
    }
    Ok(FoldersHealth {
        member: member.to_string(),
        ok: out.iter().all(|f| f.problems.is_empty()),
        folders: out,
    })
}

/// **Detection.** Report each unpaused folder's state and flag folders in
/// error, with pull errors, or stuck scanning past `--scan-stall-secs`.
/// Read-only.
#[orca_tool(domain = "syncthing", verb = "folders.health", role = "any")]
async fn syncthing_folders_health(
    args: FoldersHealthArgs,
    _ctx: &ToolCtx,
) -> Result<FoldersHealth> {
    let base = api::base_url(&args.member).map_err(|e| anyhow!(e))?;
    let key = api::api_key(&args.member).map_err(|e| anyhow!(e))?;
    folders_health(
        &args.member,
        &base,
        key.expose_secret(),
        args.scan_stall_secs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_toolkit::contract::config::{Config, Model, Ports};
    use plugin_toolkit::contract::CallerIdentity;
    use std::sync::Arc;

    fn ctx(home: &std::path::Path) -> ToolCtx {
        ToolCtx::new(Arc::new(Config {
            anthropic_api_key: None,
            lmstudio_url: String::new(),
            ollama_url: String::new(),
            default_model: Model::LMStudio {
                id: String::new(),
                url: String::new(),
            },
            app_dir: home.to_path_buf(),
            memory_root: home.to_path_buf(),
            db_path: home.join("orca.db"),
            ports: Ports::default(),
        }))
    }

    #[tokio::test]
    async fn ignore_perms_sync_refuses_non_admins_on_the_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let user = CallerIdentity {
            user_id: "u".into(),
            username: "op".into(),
            role: "user".into(),
            can_mutate: true,
        };
        for (ctx, want) in [
            (ctx(dir.path()), "no caller identity"),
            (ctx(dir.path()).with_auth(user), "requires role 'admin'"),
        ] {
            let args = IgnorePermsSyncArgs {
                member: "m".into(),
                folder: "backups".into(),
                dry_run: true,
            };
            let err = syncthing_ignore_perms_sync(args, &ctx).await.unwrap_err();
            assert!(err.to_string().contains(want), "{err}");
        }
    }

    #[test]
    fn only_permission_failures_count_as_chmod_errors() {
        let errs: FolderErrors = plugin_toolkit::serde_json::from_str(
            r#"{"errors":[
                {"path":"gitea-offsite","error":"syncing: handling dir (setting permissions): chmod /backups/gitea-offsite: operation not permitted"},
                {"path":"a","error":"chmod /backups/a: Operation not permitted"},
                {"path":"b","error":"open /backups/b: permission denied"},
                {"path":"c","error":"chmod /backups/c: no such file or directory"}
            ]}"#,
        )
        .unwrap();
        let c = chmod_errors(errs);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[0].starts_with("gitea-offsite: "));
        let none: FolderErrors =
            plugin_toolkit::serde_json::from_str(r#"{"errors":null}"#).unwrap();
        assert!(chmod_errors(none).is_empty());
    }

    #[test]
    fn folder_config_path_is_encoded() {
        assert_eq!(
            folder_config_path("pbs store"),
            "/rest/config/folders/pbs%20store"
        );
    }

    #[test]
    fn folder_config_reads_ignore_perms() {
        let c: FolderConfig = plugin_toolkit::serde_json::from_str(
            r#"{"id":"x","path":"/backups","ignorePerms":true,"type":"sendreceive"}"#,
        )
        .unwrap();
        assert!(c.ignore_perms);
        assert_eq!(c.path, "/backups");
    }

    fn status(json: &str) -> DbStatus {
        plugin_toolkit::serde_json::from_str(json).unwrap()
    }

    #[test]
    fn folder_health_flags_error_and_stalled_scan() {
        let f = FolderRef {
            id: "pbs".into(),
            label: "PBS".into(),
            paused: false,
        };
        let now = plugin_toolkit::time::Timestamp::parse_rfc3339("2026-10-20T12:00:00Z")
            .unwrap()
            .unix_seconds();
        let err = assess_folder(
            &f,
            status(
                r#"{"state":"error","stateChanged":"2026-10-20T11:59:00Z","error":"folder path missing","errors":0,"pullErrors":2}"#,
            ),
            now,
            3600,
        );
        assert_eq!(
            err.problems,
            vec!["folder in error: folder path missing", "2 pull error(s)"]
        );
        let stalled = assess_folder(
            &f,
            status(r#"{"state":"scanning","stateChanged":"2026-10-20T09:00:00.123456789+00:00"}"#),
            now,
            3600,
        );
        assert_eq!(stalled.state_age_secs, Some(10800));
        assert_eq!(stalled.problems, vec!["scanning for 10800s (over 3600s)"]);
        let fresh = assess_folder(
            &f,
            status(r#"{"state":"scanning","stateChanged":"2026-10-20T11:30:00Z"}"#),
            now,
            3600,
        );
        assert!(fresh.problems.is_empty());
        let idle = assess_folder(&f, status(r#"{"state":"idle","error":""}"#), now, 3600);
        assert!(idle.problems.is_empty());
        assert_eq!(idle.error, None);
    }

    fn ig(json: &str) -> Ignores {
        plugin_toolkit::serde_json::from_str(json).unwrap()
    }

    #[test]
    fn folder_id_is_percent_encoded() {
        assert_eq!(
            ignores_path("pbs store/x"),
            "/rest/db/ignores?folder=pbs%20store%2Fx"
        );
        assert_eq!(
            ignores_path("abcd-1234_x.y"),
            "/rest/db/ignores?folder=abcd-1234_x.y"
        );
    }

    #[test]
    fn expanded_ignores_are_checked_with_the_raw_escape() {
        let r = evaluate(
            "pbs",
            ig(r##"{"ignore":["#escape=|","#include more"],"expanded":["|*.fidx","*.didx"]}"##),
        );
        assert_eq!(r.patterns, vec!["#escape=|", "|*.fidx", "*.didx"]);
        assert!(!r.ok);
        assert!(r.violations.iter().all(|v| v.pattern == "*.didx"));
    }

    #[test]
    fn raw_ignores_are_used_without_expanded_and_clean_folder_is_ok() {
        assert!(evaluate("pbs", ig(r#"{"ignore":[".lock","*.tmp_*"]}"#)).ok);
        assert!(!evaluate("pbs", ig(r#"{"ignore":[".chunks"],"expanded":null}"#)).ok);
        assert!(evaluate("pbs", ig("{}")).ok);
    }
}
