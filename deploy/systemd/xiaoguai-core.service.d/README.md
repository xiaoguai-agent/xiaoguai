# xiaoguai-core.service.d — systemd drop-ins

Drop-in files in this directory extend `xiaoguai-core.service` without
modifying the base unit. systemd merges them in lexicographic order.

## Files

### `10-low-port.conf` — bind to :80 / :443 without root

The base unit runs as the unprivileged `xiaoguai` system account and drops
**all** capabilities (`CapabilityBoundingSet=`). By default xiaoguai-core
binds to port 7600 (or whatever `settings.server.port` is set to), so no
special capabilities are needed.

If you want to bind directly to `:80` or `:443` — without a reverse proxy
in front — drop this file into the live systemd drop-in directory:

```bash
sudo cp 10-low-port.conf \
    /etc/systemd/system/xiaoguai-core.service.d/10-low-port.conf
sudo systemctl daemon-reload
sudo systemctl restart xiaoguai-core
```

The drop-in grants only `CAP_NET_BIND_SERVICE` (bind to ports < 1024) and
nothing else — all other capabilities remain absent from both the ambient
and bounding sets. This is the minimal-privilege approach; running as root
or giving `CAP_NET_ADMIN` would be far more dangerous.

You still need to update the bind port in your config:

```yaml
# /etc/xiaoguai/config.yaml
server:
  host: 0.0.0.0
  port: 443   # or 80
```

Or via environment in a second drop-in:

```
# /etc/systemd/system/xiaoguai-core.service.d/20-port.conf
[Service]
Environment="XIAOGUAI_SERVER__PORT=443"
```

### Adding your own drop-ins

Any file matching `/etc/systemd/system/xiaoguai-core.service.d/*.conf` is
merged. Common uses:

- `20-env.conf` — set `Environment=` or `EnvironmentFile=` for secrets.
- `30-limits.conf` — raise `LimitNOFILE` for very high concurrency.
- `40-tls.conf` — add `ReadOnlyPaths=` for TLS certificate directories.

Run `systemd-analyze verify /etc/systemd/system/xiaoguai-core.service`
after adding any drop-in to catch syntax errors before restarting.
