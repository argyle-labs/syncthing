//! Dynamic (subprocess) entrypoint for the syncthing plugin.
//!
//! One `[[bin]]` advertises TWO domain backends — the `ServiceBackend` (deploy/
//! backup/configure/status) and the `replication` [`SyncthingReplication`]
//! provider (the observed sync-health the mount-converge failover gate reads).
//! The hybrid `serve_tool_plugin!` arm serves both: `backends` is the combined
//! two-`BackendDef` payload; `backend_dispatch` routes each domain's
//! `*.__backend.syncthing.*` calls to the right dispatcher (see `lib.rs`).
plugin_toolkit::serve_tool_plugin! {
    name: "syncthing",
    target_compat: "any",
    backends: syncthing::backends_json(),
    backend_dispatch: syncthing::dispatch,
}
