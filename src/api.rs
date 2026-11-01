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

/// The kind of a transport failure. Tool errors reach `role = "any"` callers
/// and the host's error text embeds the route, so only the kind is surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    ConnectionRefused,
    Timeout,
    Tls,
    Dns,
    Unreachable,
    Other,
}

impl ErrorKind {
    fn classify(err: &plugin_toolkit::anyhow::Error) -> Self {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind as Io;
            match io.kind() {
                Io::ConnectionRefused => return Self::ConnectionRefused,
                Io::TimedOut => return Self::Timeout,
                Io::HostUnreachable | Io::NetworkUnreachable => return Self::Unreachable,
                _ => {}
            }
        }
        let text = format!("{err:#}").to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
        if has(&["connection refused", "refused"]) {
            Self::ConnectionRefused
        } else if has(&["timed out", "timeout", "deadline"]) {
            Self::Timeout
        } else if has(&["tls", "ssl", "certificate", "handshake"]) {
            Self::Tls
        } else if has(&["dns", "resolve", "lookup", "name or service", "nodename"]) {
            Self::Dns
        } else if has(&["unreachable", "no route"]) {
            Self::Unreachable
        } else {
            Self::Other
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionRefused => "ConnectionRefused",
            Self::Timeout => "Timeout",
            Self::Tls => "Tls",
            Self::Dns => "Dns",
            Self::Unreachable => "Unreachable",
            Self::Other => "Other",
        }
    }
}

fn transport_error(
    method: &str,
    endpoint: &str,
    path: &str,
    err: plugin_toolkit::anyhow::Error,
) -> String {
    let kind = ErrorKind::classify(&err);
    plugin_toolkit::tracing::debug!(endpoint, path, method, error = %format!("{err:#}"), "syncthing request failed");
    format!("{method} {endpoint}{path}: {}", kind.as_str())
}

fn malformed(endpoint: &str, path: &str, err: plugin_toolkit::anyhow::Error) -> String {
    plugin_toolkit::tracing::debug!(endpoint, path, error = %format!("{err:#}"), "syncthing response malformed");
    format!("{endpoint}{path}: malformed response")
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
        .map_err(|e| transport_error("GET", endpoint, path, e))?;
    if !resp.is_success() {
        return Err(format!("{endpoint}{path}: HTTP {}", resp.status));
    }
    resp.json::<T>().map_err(|e| malformed(endpoint, path, e))
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
        .map_err(|e| format!("encode {method} {endpoint}{path}: {e}"))?;
    let resp = client
        .send(req)
        .map_err(|e| transport_error(method, endpoint, path, e))?;
    if !resp.is_success() {
        return Err(format!("{endpoint}{path}: HTTP {}", resp.status));
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

    fn get_err(host_err: &str) -> String {
        let host_err = host_err.to_string();
        let sink = Box::new(move |_cap: &str, _op: &str| -> Result<String, String> {
            Err(host_err.clone())
        });
        plugin_toolkit::capsink::with_cap_sink(sink, || {
            get_json::<plugin_toolkit::serde_json::Value>(
                &Client::new(),
                "node-a",
                "https://nas.lan:8384",
                Some("k"),
                "/rest/db/status",
            )
            .unwrap_err()
        })
    }

    #[test]
    fn leak_cases_never_reach_the_message() {
        let cases = [
            ("connection refused by nas.lan.", "nas.lan"),
            ("dial tcp:nas.lan:8384 refused", "nas.lan"),
            ("connect to ::ffff:192.0.2.10 timed out", "192.0.2.10"),
            (
                "url https://admin:pw@nas.lan:8384/rest: tls handshake eof",
                "admin",
            ),
            ("dns error for nas%2Elan", "nas%2elan"),
            ("dns lookup xn--nas-cpa.lan failed", "xn--nas"),
        ];
        for (host_err, secret) in cases {
            let err = get_err(host_err);
            let lower = err.to_ascii_lowercase();
            assert!(!lower.contains(secret), "{err}");
            assert!(!lower.contains("8384"), "{err}");
            assert!(!lower.contains("nas"), "{err}");
            assert!(err.starts_with("GET node-a/rest/db/status: "), "{err}");
        }
    }

    #[test]
    fn each_kind_is_classified() {
        let cases = [
            (
                "error sending request: Connection refused (os error 61)",
                "ConnectionRefused",
            ),
            ("operation timed out", "Timeout"),
            ("invalid peer certificate: UnknownIssuer", "Tls"),
            ("dns error: failed to lookup address information", "Dns"),
            ("No route to host (os error 65)", "Unreachable"),
            ("connection reset by peer", "Other"),
        ];
        for (host_err, kind) in cases {
            assert_eq!(
                get_err(host_err),
                format!("GET node-a/rest/db/status: {kind}")
            );
        }
    }

    #[test]
    fn io_errors_classify_by_kind() {
        use std::io::{Error, ErrorKind as Io};
        let cases = [
            (Io::ConnectionRefused, ErrorKind::ConnectionRefused),
            (Io::TimedOut, ErrorKind::Timeout),
            (Io::HostUnreachable, ErrorKind::Unreachable),
            (Io::NetworkUnreachable, ErrorKind::Unreachable),
        ];
        for (io, kind) in cases {
            let err = plugin_toolkit::anyhow::Error::new(Error::new(io, "x"));
            assert_eq!(ErrorKind::classify(&err), kind);
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
