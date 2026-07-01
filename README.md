<p align="center">
  <img src="assets/icon-256.png" width="120" alt="syncthing" />
</p>

# syncthing

Syncthing is a continuous, peer-to-peer file synchronization tool.

A first-party [orca](https://github.com/argyle-labs/orca) plugin (service-backend).

This repo is **self-contained** — the steps below run syncthing **by hand, without orca**. orca automates exactly this (same image, ports, and data) through one generic surface.

---

## Run it without orca

### Docker / Podman

```yaml
# compose.yml
services:
  syncthing:
    image: lscr.io/linuxserver/syncthing:latest
    container_name: syncthing
    restart: unless-stopped
    ports:
      - "8384:8384/tcp"   # web UI
      - "22000:22000/tcp"   # sync
      - "22000:22000/udp"   # sync
      - "21027:21027/udp"   # discovery
    volumes:
      - ./config:/config
      - /path/to/data:/data
```

```sh
docker compose up -d
```

Podman: the same file with `podman-compose up -d`.

### Ports & data

| | |
|---|---|
| Default port | `8384` |
| Upstream | <https://syncthing.net/> |


### Backup & restore

Back up the config/data volume(s) above — that's the whole service state (stop the container first for a clean copy). Restore by putting them back and starting it.

> With orca this is **`service.backup` / `service.restore`** — location-agnostic (docker / podman / lxc / vm), one command regardless of where syncthing runs. No per-service backup script.

## With orca

orca drives this plugin through the single generic `service.*` surface — no per-plugin tools:

```sh
orca service.deploy syncthing      # render + launch on any supported runtime
orca service.status syncthing      # health + rich diagnostics (typed payload)
orca service.backup syncthing      # location-agnostic backup (tar; PBS on Proxmox)
orca service.configure syncthing   # apply config via the upstream API
```

## Layout

- `src/` — the plugin (pure Rust): the `ServiceBackend` descriptor + `configure` / `status`.
- [CAPABILITIES.md](CAPABILITIES.md) — the service-backend contract checklist.
- `assets/` — plugin icon.
