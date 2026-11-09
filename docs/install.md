# Installing Kitsunarr

> Kitsunarr is in early development and cannot download media yet. Run it next to your existing Sonarr/Tanukarr, not instead of them. Migration only reads from the source and never touches your media files.

## Docker / Podman

The image `ghcr.io/kvasfan/kitsunarr` is published by the release workflow. Until the first release, build it locally from the repository root:

```sh
docker build -t kitsunarr:local -f deployment/Dockerfile .
```

`deployment/docker-compose.yml` contains a complete example. Notes:

- **`/config`** holds the database (`kitsunarr.db`), backups and `config.toml`. Back it up.
- **Media.** Mount your media at the **same paths your Sonarr/Tanukarr container used**, or add path mappings in the migration wizard. Kitsunarr does not move or rename files during migration.
- **User.** The container runs as UID/GID 1000 by default. Use `user: "UID:GID"` to match the owner of your media and config directories.
- **Health check.** The image checks itself with `kitsunarr healthcheck`; no shell tools are needed.

## Native binary (Linux)

```sh
cargo install dioxus-cli@0.7.10 --locked
dx build -p kitsunarr-web --release --profile web-release
cargo build --release -p kitsunarr-server
sudo install -m 755 target/release/kitsunarr /usr/local/bin/kitsunarr
sudo mkdir -p /etc/kitsunarr /var/lib/kitsunarr
kitsunarr --config-dir /etc/kitsunarr --data-dir /var/lib/kitsunarr
```

A systemd unit is provided in `deployment/kitsunarr.service`.

## Reverse proxy

Serve Kitsunarr at a sub-path by setting `KITSUNARR_URL_BASE=/kitsunarr`. Also set `KITSUNARR_TRUSTED_PROXIES` to the proxy's address, so client IPs (for login throttling) and HTTPS detection (for `Secure` cookies) are correct.

The proxy must forward the `Host` and `Origin` headers unchanged, because the cross-site request protection compares them. Server-Sent Events (`/api/v1/events`) need response buffering disabled. For nginx:

```nginx
location /kitsunarr/ {
    proxy_pass http://127.0.0.1:8990;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_buffering off;
}
```

## Backups

- Scheduled backups run weekly; the last 8 are kept.
- Each migration writes its own pre-migration backup.
- Run `kitsunarr backup` for an ad-hoc copy.
- To restore, stop the server and run `kitsunarr restore <file>`. The current database is kept next to the restored one.
