# Syncthing diagnostics — proposed capabilities

Field notes from the 2026-08-10 fleet incident, where the replica NAS was
believed to be replicating but two of its folders had silently not synced for
weeks. Written as an implementation spec for detection this plugin should add.

All checks below are available through the Syncthing REST API (`X-API-Key` from
`config.xml`): `/rest/config/folders`, `/rest/db/status?folder=<id>`,
`/rest/system/error`, `/rest/system/connections`, `/rest/db/completion`.

---

## 1. Folder path not inside a container bind mount (the silent killer)

**Symptom.** A folder never syncs. `/rest/system/error` shows:

```
Failed to create folder root directory (folder.id=downloads
  error="mkdir /mnt/user: permission denied")
Failed initial scan (error="folder path missing" folder.id=downloads)
```

**Cause.** Syncthing runs in a container. A folder's `path` was configured as a
**host** path (`/mnt/user/downloads`) that is **not bind-mounted** into the
container, so from inside the container that path does not exist and cannot be
created. The folder enters an error state and stays there — the peer connection
is fine, other folders sync, so the failure is invisible unless you look at this
specific folder.

**How it happens.** Image migration. The config was created under an image that
bind-mounted all of `/mnt/user` (paths like `/mnt/user/downloads` resolved), then
moved to an image with **per-subdirectory** binds (`/data`, `/backups`, …). The
folders that were repointed to the new container paths kept working; the ones that
were missed (`downloads`, `pbs`) kept their old host paths and broke.

**Detection.** For each folder in `/rest/config/folders`, verify its `path`
resolves inside the container to an existing, writable directory (cross-check
against the container's bind mounts). A folder whose `path` starts with a host
mount prefix that is not a declared bind is misconfigured. Surface it explicitly —
do **not** report the instance "healthy" just because the peer is connected and
other folders are green.

**Remediation.** Add the missing bind (`-v /mnt/user/downloads:/downloads`) to the
container **and** the unraid template so it survives a recreate, then `PATCH
/rest/config/folders/<id>` to set `path` to the in-container path, unpause, and
trigger `/rest/db/scan`.

## 2. Paused / errored folders masquerading as a healthy instance

**Symptom.** "Syncthing is up and connected" — but a folder is `paused` or in an
error state and holds stale data.

**Detection.** Instance health must be the **min** over folders, not the peer
connection state. Report each folder's `state` (`idle`/`scanning`/`syncing`/
`error`), `paused`, `needBytes`, `needFiles`, and `errors` from
`/rest/db/status`. `needBytes>0` while `state=idle` for a long time = stuck.
Compare local `sequence` vs `remoteSequence[peer]` to catch a folder that is
quietly falling behind.

## 3. Empty `sendreceive` folder = a deletion cannon

**Symptom.** A `sendreceive` folder whose local copy is empty (or nearly) while
the peer holds the real data.

**Why it is dangerous.** `sendreceive` is bidirectional. An empty local side can
propagate **deletions** to the peer — turning a broken replica into an active
data-loss vector. In the incident the replica's `pbs` folder was empty (1 KB) and
pointed at the peer's live PBS datastore; had its path been "fixed" naively it
could have deleted the primary's backups.

**Detection.** Flag any `sendreceive` folder where the local dataset is
empty/tiny but the global (peer) dataset is large. Recommend `receiveonly` for
true replica targets, or **pause** the folder until intent is confirmed. Never
auto-repair an empty `sendreceive` folder by fixing its path — pause first.

## 4. Datastores that should not be file-synced at all

Some "folders" are really live application stores (a PBS chunkstore, a database
dir). File-level sync of a live chunkstore is unsafe and pointless — those have
their own native replication. Detect known datastore signatures under a folder
path and warn against syncing them here. (See the pbs plugin's `diagnostics.md`.)
