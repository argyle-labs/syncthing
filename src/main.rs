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

fn main() -> plugin_toolkit::anyhow::Result<()> {
    Plugin::named("syncthing")
        .version(env!("CARGO_PKG_VERSION"))
        .service(SyncthingBackend::new("syncthing"))
        .replication(SyncthingReplication::new("syncthing"))
        .serve()
}
