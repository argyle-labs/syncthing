# Syncthing

Continuous file synchronization between hosts.

**Status:** running — two hosts (`<ip-a>` and `<ip-b>`), web `:8384`, sync `:22000`

- **Hosts**: two peers (e.g. a NAS and a VM) at `<ip-a>` and `<ip-b>`
- **Web UI port**: 8384
- **Sync port**: 22000

## Notes

Replicates data between two hosts for redundancy (e.g. Immich photos, media). Each instance exposes its web UI on 8384 and syncs peer-to-peer on 22000.
