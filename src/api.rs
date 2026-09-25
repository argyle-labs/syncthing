//! Shared Syncthing REST access — credential resolution and JSON GET.
//!
//! Both facets this crate registers talk to the same REST API: the
//! [`crate::replication`] provider (per-member folder health) and the
//! [`crate::SyncthingBackend`] service `status` probe. The credential
//! convention and the request shape are identical, so they live here once
//! rather than being duplicated per facet.
//!
//! Errors surface as a plain `String`. Each caller maps that into its own
//! domain error (`StorageError` / `ServiceError`) — this module stays free of
//! either domain so neither facet drags in the other's types.

use plugin_toolkit::client::{Client, Request};
use plugin_toolkit::secrets;

/// Default Syncthing GUI/REST port. Overridable per member via a `url` secret.
pub const DEFAULT_PORT: u16 = 8384;

/// Provider name — the string a relationship's `provider` field carries, and
/// the scope every secret below is keyed under.
pub const PROVIDER: &str = "syncthing";

/// Per-request timeout. Syncthing answers these reads promptly; a probe that
/// hangs is a failed probe, not a slow one.
const TIMEOUT_MS: u64 = 8_000;

/// The port-convention base URL for a member with no `url` secret override.
/// Pure — no runtime/secret dependency, so the convention is unit-testable.
pub fn default_base_url(member: &str) -> String {
    format!("http://{member}:{DEFAULT_PORT}")
}

/// Base REST URL for `member` — the `url` secret if set, else the port
/// convention. Trailing slash trimmed so paths concatenate cleanly.
pub fn base_url(member: &str) -> Result<String, String> {
    let key = secrets::scoped_name(PROVIDER, member, "url");
    let url = secrets::get(&key)
        .map_err(|e| format!("read url secret for {member}: {e}"))?
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_base_url(member));
    Ok(url.trim_end_matches('/').to_string())
}

/// Resolve `member`'s API key from the secret store. A missing key is a
/// configuration gap, surfaced as an error so the member reads unhealthy
/// rather than silently "connected".
pub fn api_key(member: &str) -> Result<String, String> {
    let key = secrets::scoped_name(PROVIDER, member, "apikey");
    secrets::get_required(&key)
        .map_err(|e| format!("no Syncthing API key for member '{member}' (secret '{key}'): {e}"))
}

/// `member`'s API key if one is configured, else `None`.
///
/// Distinct from [`api_key`]: the unauthenticated health probe treats a missing
/// key as "less detail available", not as a failure, so it must be able to tell
/// an absent key from a broken secret store.
pub fn api_key_opt(member: &str) -> Option<String> {
    let key = secrets::scoped_name(PROVIDER, member, "apikey");
    secrets::get(&key)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

/// GET `{base}{path}`, decoding JSON into `T`. `key` is sent as `X-API-Key`
/// when `Some`; Syncthing's `/rest/noauth/*` paths need no key.
pub fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    base: &str,
    key: Option<&str>,
    path: &str,
) -> Result<T, String> {
    let url = format!("{base}{path}");
    let mut req = Request::new("GET", &url).timeout_ms(TIMEOUT_MS);
    if let Some(k) = key {
        req = req.header("X-API-Key", k);
    }
    let resp = client.send(req).map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.is_success() {
        return Err(format!("GET {url} → HTTP {}", resp.status));
    }
    resp.json::<T>().map_err(|e| format!("decode {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_defaults_to_port_convention() {
        assert_eq!(default_base_url("10.0.0.10"), "http://10.0.0.10:8384");
    }
}
