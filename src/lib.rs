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
//! are advertised from one `[[bin]]` — see [`backends_json`] / [`dispatch`].
#![allow(clippy::disallowed_types)]

use plugin_toolkit::service::{
    BoxFuture, Endpoint, Runtime, ServiceBackend, ServiceCapability, ServiceError, ServiceStatus,
    WorkloadSpec,
};

pub mod replication;
pub use replication::SyncthingReplication;

/// Invoke-prefix the loader routes `service.*` backend ops back through.
const SERVICE_PREFIX: &str = "service.__backend.syncthing";
/// Invoke-prefix the loader routes `replication.status` back through.
const REPLICATION_PREFIX: &str = "replication.__backend.syncthing";

/// The combined `backends()` payload this plugin advertises: its
/// [`ServiceBackend`] **and** its [`SyncthingReplication`] provider, so one
/// subprocess lights up both domains. Serialized as a two-element `BackendDef`
/// array — the loader registers each against its domain dispatch table.
pub fn backends_json() -> String {
    use plugin_toolkit::backend_def::{replication_backend_def, service_backend_def};
    let service = service_backend_def(&SyncthingBackend::new("syncthing"), SERVICE_PREFIX);
    let replication = replication_backend_def("syncthing", REPLICATION_PREFIX);
    plugin_toolkit::serde_json::to_string(&[service, replication])
        .unwrap_or_else(|_| "[]".to_string())
}

/// Route a proxied backend op to the right domain dispatcher. Returns `None` for
/// a tool that belongs to neither backend (nothing else is served here).
pub fn dispatch(
    tool: &str,
    args: plugin_toolkit::serde_json::Value,
) -> Option<Result<plugin_toolkit::serde_json::Value, plugin_toolkit::serde_json::Value>> {
    if let Some(op) = tool
        .strip_prefix(SERVICE_PREFIX)
        .and_then(|r| r.strip_prefix('.'))
    {
        let backend = SyncthingBackend::new("syncthing");
        return Some(plugin_toolkit::reactor::block_on(
            plugin_toolkit::service::dispatch_op(&backend, op, args),
        ));
    }
    if let Some(op) = tool
        .strip_prefix(REPLICATION_PREFIX)
        .and_then(|r| r.strip_prefix('.'))
    {
        let provider = SyncthingReplication::new("syncthing");
        return Some(plugin_toolkit::reactor::block_on(
            plugin_toolkit::storage::replication_status::dispatch_op(&provider, op, args),
        ));
    }
    None
}

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

    fn status<'a>(
        &'a self,
        _ep: &'a Endpoint,
    ) -> BoxFuture<'a, Result<ServiceStatus, ServiceError>> {
        // TODO: real health/diagnostics.
        Box::pin(async move { Err(ServiceError::unimplemented("syncthing.status")) })
    }
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
