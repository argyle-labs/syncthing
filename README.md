<p align="center">
  <img src="assets/icon-256.png" width="120" alt="syncthing" />
</p>

# syncthing

Syncthing is a continuous, peer-to-peer file synchronization tool.

A first-party [orca](https://github.com/argyle-labs/orca) plugin (service-backend).

This repo is **self-contained** — the steps below run syncthing **by hand, without orca**. orca automates exactly this (same image, ports, and data) through one generic surface.

---

## Run it without orca

### Docker Compose

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

### Other runtimes

**Podman** — the compose above works with `podman compose up -d`, or run it directly:

```sh
podman run -d --name syncthing --restart unless-stopped \
    -p 8384:8384/tcp \
    -p 22000:22000/tcp \
    -p 22000:22000/udp \
    -p 21027:21027/udp \
    -v ./config:/config \
    -v /path/to/data:/data \
    lscr.io/linuxserver/syncthing:latest
```

**LXC** — on a container-capable LXC (e.g. a Proxmox LXC with nesting enabled) run the same image via Docker/Podman as above, or install syncthing from upstream directly on the guest: <https://syncthing.net/>.

**VM** — install syncthing from upstream (<https://syncthing.net/>) or run the same container image inside the VM; expose port `8384`.

**Unraid** — add via *Community Applications*, or *Docker → Add Container* with image `lscr.io/linuxserver/syncthing:latest`, port `8384`, and the volume paths above.

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
