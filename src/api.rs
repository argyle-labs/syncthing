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
    let Some(host) = route_host(base) else {
        return err.to_string();
    };
    let mut needles = vec![host.clone()];
    if host.contains(':') {
        needles.insert(0, format!("[{host}]"));
    }
    let mut msg = err.to_string();
    for needle in &needles {
        msg = replace_tokens(&msg, needle);
    }
    msg
}

/// The lowercased host of `base`, IPv6 without brackets.
fn route_host(base: &str) -> Option<String> {
    let rest = base.split_once("://").map_or(base, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Replace each whole-token occurrence of `needle` (already lowercase) with
/// `<route>`, absorbing a leading `scheme://`, a trailing `:port`, and, for a
/// URL, its path.
fn replace_tokens(msg: &str, needle: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    let bytes = msg.as_bytes();
    let is_host = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':');
    let mut out = String::with_capacity(msg.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(off) = lower[from..].find(needle) {
        let at = from + off;
        let mut end = at + needle.len();
        from = end;
        let before_ok = at == 0
            || !is_host(bytes[at - 1])
            || bytes[at - 1] == b':' && at >= 3 && &bytes[at - 3..at] == b"://";
        let after_ok = end == bytes.len() || !is_host(bytes[end]) || bytes[end] == b':';
        if !before_ok || !after_ok {
            continue;
        }
        let mut start = at;
        let is_url = at >= 3 && &bytes[at - 3..at] == b"://";
        if is_url {
            start = at - 3;
            while start > 0 && bytes[start - 1].is_ascii_alphabetic() {
                start -= 1;
            }
        }
        if end < bytes.len() && bytes[end] == b':' {
            let digits = bytes[end + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 0 {
                end += 1 + digits;
            }
        }
        if is_url {
            end += bytes[end..]
                .iter()
                .take_while(|&&b| {
                    !b.is_ascii_whitespace() && !matches!(b, b')' | b'"' | b'\'' | b'>' | b',')
                })
                .count();
            while matches!(bytes[end - 1], b':' | b'.' | b';') {
                end -= 1;
            }
        }
        out.push_str(&msg[copied..start]);
        out.push_str("<route>");
        copied = end;
        from = end;
    }
    out.push_str(&msg[copied..]);
    out
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
    fn scrub_matches_host_case_insensitively() {
        let msg = scrub(
            "connect to HTTPS://NAS.Example.COM:8384/rest failed",
            "https://nas.example.com:8384",
        );
        assert_eq!(msg, "connect to <route> failed");
    }

    #[test]
    fn scrub_handles_elided_default_port() {
        let msg = scrub(
            "dns error for https://nas.lan:443/rest/db and nas.lan",
            "https://nas.lan",
        );
        assert_eq!(msg, "dns error for <route> and <route>");
    }

    #[test]
    fn scrub_handles_route_with_trailing_path() {
        let msg = scrub(
            "GET https://nas.lan/sync/rest/x: refused by nas.lan:443",
            "https://nas.lan/sync",
        );
        assert_eq!(msg, "GET <route>: refused by <route>");
    }

    #[test]
    fn scrub_handles_ipv6_with_port() {
        let msg = scrub(
            "url (https://[::1]:8384/rest): refused by [::1]:8384 at ::1",
            "https://[::1]:8384",
        );
        assert_eq!(msg, "url (<route>): refused by <route> at <route>");
    }

    #[test]
    fn scrub_handles_ipv6_without_port() {
        let msg = scrub(
            "tls handshake with [fe80::1] at 10:42: eof",
            "https://[FE80::1]",
        );
        assert_eq!(msg, "tls handshake with <route> at 10:42: eof");
    }

    #[test]
    fn scrub_leaves_words_containing_a_short_host() {
        let msg = scrub(
            "status: st: refused; first stage at st:8384",
            "https://st:8384",
        );
        assert_eq!(msg, "status: <route>: refused; first stage at <route>");
    }

    #[test]
    fn scrub_keeps_message_readable() {
        let msg = scrub("error sending request for url (https://192.0.2.10:8384/rest/db/status): connection refused", "https://192.0.2.10:8384");
        assert_eq!(
            msg,
            "error sending request for url (<route>): connection refused"
        );
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
