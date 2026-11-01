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
    let base = base.trim_end_matches('/').to_string();
    require_tls(endpoint, &base, row.allow_plaintext)?;
    Ok(Target { base, key })
}

/// The API key grants full control of the node, so it is only sent over
/// `https` unless the endpoint opts in with `allow_plaintext`.
pub fn require_tls(endpoint: &str, base: &str, allow_plaintext: bool) -> Result<(), String> {
    if allow_plaintext
        || base
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        return Ok(());
    }
    Err(format!(
        "syncthing endpoint '{endpoint}': refusing to send the API key over non-https route \
         {base}; enable Syncthing GUI TLS (gui.tls=true) and set insecure=true for its \
         self-signed cert, or set allow_plaintext=true on the endpoint"
    ))
}

/// `endpoint`'s API key. A missing key is a configuration gap, surfaced as an
/// error so the endpoint reads unhealthy rather than silently "connected".
pub fn api_key(endpoint: &str) -> Result<secrets::Secret<String>, String> {
    secrets::resolve_scoped(PROVIDER, endpoint, "apikey")
        .map_err(|e| format!("syncthing endpoint '{endpoint}': {e}"))
}

/// Errors name `{endpoint}{path}`, never the route: tool errors reach
/// `role = "any"` callers, and the host's transport error embeds the URL.
fn scrub(err: impl std::fmt::Display, base: &str) -> String {
    let authority = base.split_once("://").map_or(base, |(_, a)| a);
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(h, _)| h)
        .trim_start_matches('[')
        .trim_end_matches(']');
    let mut msg = err.to_string().replace(base, "").replace(authority, "");
    if !host.is_empty() {
        msg = msg.replace(host, "<route>");
    }
    msg
}

/// GET `{base}{path}`, decoding JSON into `T`. `key` is sent as `X-API-Key`
/// when `Some`; Syncthing's `/rest/noauth/*` paths need no key.
pub fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    endpoint: &str,
    base: &str,
    key: Option<&str>,
    path: &str,
) -> Result<T, String> {
    let url = format!("{base}{path}");
    let mut req = Request::new("GET", &url).timeout_ms(TIMEOUT_MS);
    if let Some(k) = key {
        req = req.header("X-API-Key", k);
    }
    let resp = client
        .send(req)
        .map_err(|e| format!("GET {endpoint}{path}: {}", scrub(e, base)))?;
    if !resp.is_success() {
        return Err(format!("GET {endpoint}{path} → HTTP {}", resp.status));
    }
    resp.json::<T>()
        .map_err(|e| format!("decode {endpoint}{path}: {}", scrub(e, base)))
}

/// `method` `{base}{path}` with a JSON body; the reply body is ignored.
pub fn send_json<B: serde::Serialize>(
    client: &Client,
    method: &str,
    endpoint: &str,
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
        .map_err(|e| format!("encode {method} {endpoint}{path}: {}", scrub(e, base)))?;
    let resp = client
        .send(req)
        .map_err(|e| format!("{method} {endpoint}{path}: {}", scrub(e, base)))?;
    if !resp.is_success() {
        return Err(format!("{method} {endpoint}{path} → HTTP {}", resp.status));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apikey_is_keyed_to_the_endpoint_not_its_address() {
        let name = secrets::scoped_name(PROVIDER, "node-a", "apikey").unwrap();
        assert_eq!(name.as_str(), "syncthing.node-a.apikey");
        assert!(plugin_toolkit::runtime::validate_endpoint_name("192.0.2.10").is_err());
    }

    #[test]
    fn transport_error_names_endpoint_not_route() {
        let base = "https://192.0.2.10:8384";
        let sink = Box::new(|_cap: &str, _op: &str| -> Result<String, String> {
            Err(
                "error sending request for url (https://192.0.2.10:8384/rest/db/status): \
                 connection refused by 192.0.2.10"
                    .into(),
            )
        });
        let get = plugin_toolkit::capsink::with_cap_sink(sink, || {
            get_json::<plugin_toolkit::serde_json::Value>(
                &Client::new(),
                "node-a",
                base,
                Some("k"),
                "/rest/db/status",
            )
            .unwrap_err()
        });
        let sink = Box::new(|_cap: &str, _op: &str| -> Result<String, String> {
            Err("error sending request for url (https://192.0.2.10:8384/rest/db/status)".into())
        });
        let send = plugin_toolkit::capsink::with_cap_sink(sink, || {
            send_json(
                &Client::new(),
                "PATCH",
                "node-a",
                base,
                "k",
                "/rest/db/status",
                &1,
            )
            .unwrap_err()
        });
        for err in [get, send] {
            assert!(err.contains("node-a/rest/db/status"), "{err}");
            assert!(!err.contains("192.0.2.10"), "{err}");
            assert!(!err.contains("8384"), "{err}");
        }
    }

    #[test]
    fn https_route_passes() {
        assert!(require_tls("node-a", "https://192.0.2.10:8384", false).is_ok());
    }

    #[test]
    fn http_route_without_opt_in_refuses() {
        let err = require_tls("node-a", "http://192.0.2.10:8384", false).unwrap_err();
        assert!(err.contains("'node-a'"));
        assert!(err.contains("gui.tls=true"));
        assert!(err.contains("allow_plaintext"));
    }

    #[test]
    fn http_route_with_opt_in_passes() {
        assert!(require_tls("node-a", "http://192.0.2.10:8384", true).is_ok());
    }
}
