# syncthing — ServiceBackend + ReplicationStatusProvider contract

Pure-Rust plugin (**no bash/compose/provision scripts**) driven by the generic
`service.*` surface — no per-plugin tools. Runtimes: **docker,podman,lxc,vm**.
One `[[bin]]` advertises **two** domain backends: the `ServiceBackend` (deploy/
status of a syncthing instance) and a `replication` **ReplicationStatusProvider**
(the observed sync-health the mount-converge failover gate consults).

## Replication status (the `replication` domain) — monitor slice
- [x] `status(folder, members)` — reads each member's Syncthing REST API and
      returns a typed `ReplicationStatus{healthy,detail}`. `healthy` is the **min
      over members and folder state** (present, not paused, peer connected,
      `needBytes==0`, no errors, not an empty-`sendreceive` deletion risk) per
      `docs/diagnostics.md`. Adopts existing instances: per-member REST API key
      from the orca secret store (`scoped_name("syncthing",<member>,"apikey")`),
      base URL `http://<member>:8384` or a `…,"url"` secret override.
- [ ] `configure` — native folder/device **provisioning** from orca (next slice;
      Syncthing ships no OpenAPI/GraphQL spec, so codegen has nothing to consume —
      a vendored spec would be authored here if/when this surface grows).

Wired to failover: attach a `storage.replication` relationship (provider
`syncthing`, folder id, willow/maple routes) to a share; the gate then holds any
route swap until this provider confirms the folder is in sync across members.

## Per-plugin code (the only work this repo owns)
- [x] `provider` / `runtimes` / `default_port` / `capabilities` / `data_paths` — declarative descriptor
- [ ] `workload_spec(runtime)` — *what* to run; `deploy_target` renders it to a container / LXC / VM
- [ ] `configure` — apply syncthing config via its upstream API
- [ ] `status` — health + rich diagnostics returned in the typed `ServiceStatus.info`

> Declarative descriptor is implemented and the plugin **registers + loads live**
> in orca today (`service.list` shows it). `workload_spec`/`configure`/`status`
> are being filled in per plugin.

## Provided generically by orca (NO code here)
- `deploy` — `service.deploy` → `deploy_target.launch(WorkloadSpec)`
- `backup` / `restore` — pluggable `BackupMethod` (tar; **PBS** for Proxmox guests)
- single `service.*` tool surface, exposed over CLI / REST / MCP
