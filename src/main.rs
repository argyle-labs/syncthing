//! Dynamic (subprocess) entrypoint for the syncthing plugin.
//!
//! One `[[bin]]` advertises TWO domain backends — the `ServiceBackend` (deploy/
//! backup/configure/status) and the `replication` [`SyncthingReplication`]
//! provider (the observed sync-health the mount-converge failover gate reads).
//! Both are registered as typed backends on the [`Plugin`] builder, which emits
//! all the wire dispatch — the plugin hand-writes no op strings.

plugin_toolkit::instrument::bootstrap!();

use plugin_toolkit::plugin::Plugin;
use syncthing::{SyncthingBackend, SyncthingReplication};

#[allow(unused_imports)]
use syncthing::tools as _;

fn main() -> plugin_toolkit::anyhow::Result<()> {
    plugin_toolkit::logging::init(plugin_toolkit::logging::LogInit {
        env_var: "ORCA_LOG",
        default_filter: "info",
        tee_path: None,
        tee_max_bytes: None,
        tee_keep: None,
    })?;
    Plugin::named("syncthing")
        .version(env!("CARGO_PKG_VERSION"))
        .service(SyncthingBackend::new("syncthing"))
        .replication(SyncthingReplication::new("syncthing"))
        .tools(["syncthing."])
        .serve()
}
