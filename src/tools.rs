//! syncthing tool surface.
//!
//!   - `syncthing.ignores.check_pbs`  whether a folder's ignore patterns exclude PBS datastore files

use plugin_toolkit::client::Client;
use plugin_toolkit::prelude::*;

use crate::api;
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
    check_pbs(&base, &key, &args.folder)
}

#[cfg(test)]
mod tests {
    use super::*;

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
