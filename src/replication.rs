//! Syncthing replication-status provider — the *observed* side of an orca
//! `storage.replication` relationship.
//!
//! orca's mount-converge failover gate ([`should_swap`]) refuses to swap an
//! active mount off a source until the replication relationship the share
//! references is **confirmed healthy**. Core ships no provider, so today that
//! signal is always *Unknown* (the gate holds). This module supplies the
//! `syncthing` provider: given a folder id and the member hosts, it reads each
//! member's Syncthing REST API and answers "is this folder in sync across every
//! member, right now?".
//!
//! Health is the **min over members and over the folder's real state**, not "the
//! peer is connected" — the lesson of `docs/diagnostics.md` (a replica whose
//! folder silently error-stalled for weeks while the peer link looked fine). A
//! member counts as healthy only when the folder is present, not paused, has no
//! errors, is fully in sync (`needBytes == 0`), and is not an empty
//! `sendreceive` side that could propagate deletions.
//!
//! Credentials are orca-native: each member's REST API key lives in the orca
//! secret store under `scoped_name("syncthing", <member>, "apikey")`; the base
//! URL defaults to `http://<member>:8384` and may be overridden by a
//! `scoped_name("syncthing", <member>, "url")` secret.

use plugin_toolkit::client::{Client, Request};
use plugin_toolkit::secrets;
use plugin_toolkit::storage::replication_status::ReplicationStatusProvider;
use plugin_toolkit::storage::{ReplicationStatus, StorageError};
use serde::Deserialize;

/// Default Syncthing GUI/REST port. Overridable per member via a `url` secret.
const DEFAULT_PORT: u16 = 8384;

/// Provider name — the string a relationship's `provider` field carries.
const PROVIDER: &str = "syncthing";

/// The `syncthing` [`ReplicationStatusProvider`]. Stateless; every call resolves
/// per-member credentials from the secret store and queries the REST API fresh
/// (on-demand, never a poll-cache).
#[derive(Debug, Clone)]
pub struct SyncthingReplication {
    provider: &'static str,
}

impl SyncthingReplication {
    pub fn new(provider: &'static str) -> Self {
        Self { provider }
    }
}

// ── Syncthing REST response shapes (only the fields we read) ──────────────────

#[derive(Debug, Deserialize)]
struct Folder {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    paused: bool,
    /// `sendreceive` / `sendonly` / `receiveonly`.
    #[serde(rename = "type", default)]
    folder_type: String,
}

#[derive(Debug, Deserialize)]
struct DbStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    need_bytes: i64,
    #[serde(default)]
    need_items: i64,
    #[serde(default)]
    local_bytes: i64,
    #[serde(default)]
    global_bytes: i64,
    #[serde(default)]
    errors: i64,
}

#[derive(Debug, Deserialize)]
struct SystemError {
    #[serde(default)]
    when: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct SystemErrors {
    #[serde(default)]
    errors: Option<Vec<SystemError>>,
}

#[derive(Debug, Deserialize)]
struct ConnTotal {
    #[serde(default)]
    connected: bool,
}

#[derive(Debug, Deserialize)]
struct Connections {
    #[serde(default)]
    connections: std::collections::HashMap<String, ConnTotal>,
}

/// One member's observed folder health, reduced to the signals the gate needs.
struct MemberHealth {
    in_sync: bool,
    /// A human phrase for the aggregate `detail` line — includes the member name.
    note: String,
}

impl SyncthingReplication {
    /// Base REST URL for `member` — the `url` secret if set, else the port
    /// convention. Trailing slash trimmed so paths concatenate cleanly.
    fn base_url(member: &str) -> Result<String, StorageError> {
        let key = secrets::scoped_name(PROVIDER, member, "url");
        let url = secrets::get(&key)
            .map_err(|e| StorageError::Other(format!("read url secret for {member}: {e}")))?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Self::default_base_url(member));
        Ok(url.trim_end_matches('/').to_string())
    }

    /// The port-convention base URL for a member with no `url` secret override.
    /// Pure — no runtime/secret dependency, so the convention is unit-testable.
    fn default_base_url(member: &str) -> String {
        format!("http://{member}:{DEFAULT_PORT}")
    }

    /// Resolve `member`'s API key from the secret store. A missing key is a
    /// configuration gap, surfaced as an error so the member reads unhealthy
    /// (the gate holds) rather than silently "connected".
    fn api_key(member: &str) -> Result<String, StorageError> {
        let key = secrets::scoped_name(PROVIDER, member, "apikey");
        secrets::get_required(&key).map_err(|e| {
            StorageError::Other(format!(
                "no Syncthing API key for member '{member}' (secret '{key}'): {e}"
            ))
        })
    }

    /// GET `{base}{path}` with the API key header, decoding JSON into `T`.
    fn get_json<T: serde::de::DeserializeOwned>(
        client: &Client,
        base: &str,
        key: &str,
        path: &str,
    ) -> Result<T, StorageError> {
        let url = format!("{base}{path}");
        let resp = client
            .send(
                Request::new("GET", &url)
                    .header("X-API-Key", key)
                    .timeout_ms(8_000),
            )
            .map_err(|e| StorageError::Transport(format!("GET {url}: {e}")))?;
        if !resp.is_success() {
            return Err(StorageError::Transport(format!(
                "GET {url} → HTTP {}",
                resp.status
            )));
        }
        resp.json::<T>()
            .map_err(|e| StorageError::Other(format!("decode {url}: {e}")))
    }

    /// Observe one member's health for `folder`, applying the `diagnostics.md`
    /// checks. Any transport/config failure is an `Err` — the caller treats a
    /// failed member as not-in-sync, never as healthy.
    fn member_health(
        client: &Client,
        member: &str,
        folder: &str,
    ) -> Result<MemberHealth, StorageError> {
        let base = Self::base_url(member)?;
        let key = Self::api_key(member)?;

        // 1. Folder must exist and not be paused (diagnostics.md #2).
        let folders: Vec<Folder> = Self::get_json(client, &base, &key, "/rest/config/folders")?;
        let Some(f) = folders.iter().find(|f| f.id == folder || f.label == folder) else {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!("{member}: folder '{folder}' not configured"),
            });
        };
        if f.paused {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!("{member}: folder paused"),
            });
        }

        // 2. Peer link must be up — otherwise "in sync" is stale (diagnostics.md #2).
        let conns: Connections = Self::get_json(client, &base, &key, "/rest/system/connections")?;
        let peer_connected = conns.connections.values().any(|c| c.connected);
        if !peer_connected {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!("{member}: no peer connected"),
            });
        }

        // 3. Folder db-status: fully in sync, no errors (the core signal).
        let status: DbStatus = Self::get_json(
            client,
            &base,
            &key,
            &format!("/rest/db/status?folder={}", f.id),
        )?;
        if status.errors > 0 {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!("{member}: {} folder error(s)", status.errors),
            });
        }
        if status.need_bytes > 0 || status.need_items > 0 {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!("{member}: behind by {} bytes", status.need_bytes),
            });
        }

        // 4. Empty-`sendreceive` deletion cannon (diagnostics.md #3): a local
        //    side that is empty/tiny while the global set is large can propagate
        //    deletions. Never call that healthy.
        if f.folder_type == "sendreceive"
            && status.global_bytes > 0
            && status.local_bytes * 100 < status.global_bytes
        {
            return Ok(MemberHealth {
                in_sync: false,
                note: format!(
                    "{member}: sendreceive local nearly empty ({}B of {}B) — deletion risk",
                    status.local_bytes, status.global_bytes
                ),
            });
        }

        // 5. Cross-check the global error log for this folder (diagnostics.md #1,
        //    the silent path-mismatch killer). Non-fatal to fetch.
        if let Ok(errs) = Self::get_json::<SystemErrors>(client, &base, &key, "/rest/system/error")
        {
            if let Some(list) = errs.errors {
                if let Some(e) = list.iter().find(|e| e.message.contains(&f.id)) {
                    return Ok(MemberHealth {
                        in_sync: false,
                        note: format!("{member}: system error [{}] {}", e.when, e.message),
                    });
                }
            }
        }

        Ok(MemberHealth {
            in_sync: true,
            note: format!("{member}: in sync ({} state)", status.state),
        })
    }
}

#[plugin_toolkit::derive::orca_async]
impl ReplicationStatusProvider for SyncthingReplication {
    fn name(&self) -> &str {
        self.provider
    }

    async fn status(
        &self,
        folder: &str,
        members: &[String],
    ) -> Result<ReplicationStatus, StorageError> {
        if members.is_empty() {
            return Ok(ReplicationStatus {
                provider: self.provider.to_string(),
                healthy: false,
                last_sync_ms: None,
                detail: Some("no members in relationship".to_string()),
            });
        }

        let client = Client::new();
        let mut notes = Vec::with_capacity(members.len());
        let mut all_in_sync = true;
        for member in members {
            match Self::member_health(&client, member, folder) {
                Ok(h) => {
                    all_in_sync &= h.in_sync;
                    notes.push(h.note);
                }
                Err(e) => {
                    all_in_sync = false;
                    notes.push(format!("{member}: {e}"));
                }
            }
        }

        Ok(ReplicationStatus {
            provider: self.provider.to_string(),
            healthy: all_in_sync,
            last_sync_ms: None,
            detail: Some(format!("folder '{folder}': {}", notes.join("; "))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_provider_name() {
        assert_eq!(SyncthingReplication::new("syncthing").name(), "syncthing");
    }

    #[test]
    fn base_url_defaults_to_port_convention() {
        // The fallback used when a member has no `url` secret override.
        assert_eq!(
            SyncthingReplication::default_base_url("10.0.0.10"),
            "http://10.0.0.10:8384"
        );
    }
}
