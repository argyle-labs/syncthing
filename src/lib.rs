//! syncthing service backend — Syncthing file synchronization.
//!
//! Implements `ServiceBackend` so the generic `service.*` tools
//! (deploy/backup/restore/configure/status/connect/sync) drive syncthing. No
//! `#[orca_tool]`s — the only orca dep is `plugin-toolkit`. Modeled on the
//! nfs StorageBackend. See orca/docs/PLUGIN-PROGRAM.md.
//!
//! The plugin also registers a [`SyncthingReplication`] over the `replication`
//! domain: the *observed* health side of an orca `storage.replication`
//! relationship that the mount-converge failover gate consults. Both backends
//! are advertised from one `[[bin]]` — registered as typed facets in `main.rs`.
#![allow(clippy::disallowed_types)]

use plugin_toolkit::client::Client;
use plugin_toolkit::service::{
    BoxFuture, Endpoint, Runtime, ServiceBackend, ServiceCapability, ServiceError, ServiceStatus,
    WorkloadSpec,
};

pub mod api;
pub mod replication;
pub use replication::SyncthingReplication;

// The service + replication backends are registered as typed facets on the
// `Plugin` builder in `main.rs`; the builder emits the `backends()` payload and
// the wire dispatch for both. This crate hand-writes no op-string routing.

/// syncthing backend. Holds only the provider name; per-instance endpoint/creds
/// come from the `Endpoint` the generic `service.*` tools hand each op.
#[derive(Debug, Clone)]
pub struct SyncthingBackend {
    provider: &'static str,
}

impl SyncthingBackend {
    pub fn new(provider: &'static str) -> Self {
        Self { provider }
    }
}

impl ServiceBackend for SyncthingBackend {
    fn provider(&self) -> &str {
        self.provider
    }

    /// Runtimes syncthing can be placed on. `service.deploy` hands the
    /// `workload_spec` below to a matching deploy target — this backend never
    /// drives pct/docker itself (that mechanic lives in the deploy-target domain).
    fn runtimes(&self) -> Vec<Runtime> {
        vec![Runtime::Docker, Runtime::Podman, Runtime::Lxc, Runtime::Vm]
    }

    fn capabilities(&self) -> Vec<ServiceCapability> {
        vec![
            ServiceCapability::Deploy,
            ServiceCapability::Backup,
            ServiceCapability::Restore,
            ServiceCapability::Configure,
            ServiceCapability::Status,
        ]
    }

    fn default_port(&self) -> u16 {
        8384
    }

    /// In-workload paths holding config/data. This is ALL syncthing declares for
    /// backup — the generic pluggable backup (tar for containers/LXC, PBS for
    /// Proxmox guests when available) snapshots these. No backup/restore code
    /// here; those are inherited from ServiceBackend's defaults.
    fn data_paths(&self) -> Vec<String> {
        vec!["/config".to_string()]
    }

    fn workload_spec<'a>(
        &'a self,
        _runtime: Runtime,
        _ep: &'a Endpoint,
    ) -> BoxFuture<'a, Result<WorkloadSpec, ServiceError>> {
        // TODO: describe the syncthing workload (image/template, ports, mounts,
        // env) for the chosen runtime. The deploy target turns this into a
        // compose service / LXC config / VM. See deploy-target::WorkloadSpec.
        Box::pin(async move { Err(ServiceError::unimplemented("syncthing.workload_spec")) })
    }

    fn configure<'a>(
        &'a self,
        _ep: &'a Endpoint,
        _config: &'a str,
    ) -> BoxFuture<'a, Result<(), ServiceError>> {
        // TODO: apply syncthing-specific config idempotently.
        Box::pin(async move { Err(ServiceError::unimplemented("syncthing.configure")) })
    }

    /// Liveness + version for one instance.
    ///
    /// Probes `/rest/noauth/health`, which needs **no** API key — so an instance
    /// whose key is not in the secret store still reports honest liveness rather
    /// than erroring. When a key *is* configured the version is added as detail;
    /// its absence degrades detail, never health.
    ///
    /// The URL comes from the endpoint's `routes` via [`Endpoint::primary_url`]
    /// (orca has no scalar `base_url` — reachability is the ordered route set).
    /// An endpoint with no URL-addressable route is an unknown, not a false
    /// negative, so it is reported as an error rather than as "unhealthy".
    fn status<'a>(
        &'a self,
        ep: &'a Endpoint,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        Box::pin(async move {
            let base = ep.primary_url();
            let base = base.trim_end_matches('/');
            if base.is_empty() {
                return Err(ServiceError::Other(format!(
                    "syncthing instance '{}' has no URL-addressable route; \
                     add one with `service.connect --route`",
                    ep.name
                )));
            }

            let client = Client::new();
            let health: Health = api::get_json(&client, base, None, "/rest/noauth/health")
                .map_err(ServiceError::Other)?;
            let healthy = health.status.eq_ignore_ascii_case("ok");

            // Version is best-effort detail: it needs the API key, and a missing
            // key must not turn a live instance into an unhealthy one.
            let detail = match api::api_key_opt(&ep.name).and_then(|k| {
                api::get_json::<Version>(&client, base, Some(&k), "/rest/system/version").ok()
            }) {
                Some(v) => format!("{} ({})", health.status, v.version),
                None => format!("{} (version needs an API key)", health.status),
            };

            Ok(ServiceStatus {
                healthy,
                detail,
                ..Default::default()
            })
        })
    }
}

// ── Syncthing REST response shapes (only the fields `status` reads) ───────────

/// `/rest/noauth/health` — `{"status":"OK"}`. Unauthenticated by design.
#[derive(Debug, serde::Deserialize)]
struct Health {
    #[serde(default)]
    status: String,
}

/// `/rest/system/version` — requires the API key.
#[derive(Debug, serde::Deserialize)]
struct Version {
    #[serde(default)]
    version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_provider() {
        let b = SyncthingBackend::new("syncthing");
        assert_eq!(b.provider(), "syncthing");
    }
}
