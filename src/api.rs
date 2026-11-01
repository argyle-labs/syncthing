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
use plugin_toolkit::route;
use plugin_toolkit::secrets;

use crate::tools::endpoint_db;

/// Provider name — the string a relationship's `provider` field carries, and
/// the scope every secret below is keyed under.
pub const PROVIDER: &str = "syncthing";

/// Per-request timeout. Syncthing answers these reads promptly; a probe that
/// hangs is a failed probe, not a slow one.
const TIMEOUT_MS: u64 = 8_000;

/// A registered endpoint's reachable base URL and API key.
pub struct Target {
    pub base: String,
    pub key: secrets::Secret<String>,
}

/// Resolve a registered `syncthing` endpoint: the first reachable route and the
/// API key stored against the endpoint by `syncthing.create --apikey`.
pub async fn resolve(endpoint: &str) -> Result<Target, String> {
    let row = endpoint_db::require(endpoint).map_err(|e| e.to_string())?;
    let key = api_key(endpoint)?;
    let base = route::resolve_reachable(endpoint, &row.routes, row.insecure)
        .await
        .map_err(|e| format!("syncthing endpoint '{endpoint}': {e}"))?;
    Ok(Target {
        base: base.trim_end_matches('/').to_string(),
        key,
    })
}

/// `endpoint`'s API key. A missing key is a configuration gap, surfaced as an
/// error so the endpoint reads unhealthy rather than silently "connected".
pub fn api_key(endpoint: &str) -> Result<secrets::Secret<String>, String> {
    secrets::resolve_scoped(PROVIDER, endpoint, "apikey")
        .map_err(|e| format!("syncthing endpoint '{endpoint}': {e}"))
}

/// `endpoint`'s API key if one is configured, else `None`.
///
/// Distinct from [`api_key`]: the unauthenticated health probe treats a missing
/// key as "less detail available", not as a failure.
pub fn api_key_opt(endpoint: &str) -> Option<secrets::Secret<String>> {
    api_key(endpoint)
        .ok()
        .filter(|s| !s.expose_secret().trim().is_empty())
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

/// `method` `{base}{path}` with a JSON body; the reply body is ignored.
pub fn send_json<B: serde::Serialize>(
    client: &Client,
    method: &str,
    base: &str,
    key: &str,
    path: &str,
    body: &B,
) -> Result<(), String> {
    let url = format!("{base}{path}");
    let req = Request::new(method, &url)
        .timeout_ms(TIMEOUT_MS)
        .header("X-API-Key", key)
        .json(body)
        .map_err(|e| format!("encode {method} {url}: {e}"))?;
    let resp = client
        .send(req)
        .map_err(|e| format!("{method} {url}: {e}"))?;
    if !resp.is_success() {
        return Err(format!("{method} {url} → HTTP {}", resp.status));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apikey_is_keyed_to_the_endpoint_not_its_address() {
        let name = secrets::scoped_name(PROVIDER, "willow", "apikey").unwrap();
        assert_eq!(name.as_str(), "syncthing.willow.apikey");
        assert!(plugin_toolkit::runtime::validate_endpoint_name("10.10.10.10").is_err());
    }
}
