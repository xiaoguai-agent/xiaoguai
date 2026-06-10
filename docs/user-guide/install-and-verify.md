# Install & verify

Per-method install commands, the output you should see, and a smoke test for
each. Xiaoguai is a **single binary over an embedded SQLite file** — every
method below ends with a `xiaoguai` process on `http://localhost:7600`
(default; state in `~/.xiaoguai/data.db` or `/var/lib/xiaoguai` for the
packaged service).

The universal verifier is built in:

```bash
xiaoguai doctor
```

```text
✓ database     writable, schema current (/Users/you/.xiaoguai/data.db)
✓ providers    default: Ollama (local) (local — no API key needed)
! ollama       reachable, but model qwen2.5-coder is not pulled — run: ollama pull qwen2.5-coder
✓ port         7600 is free (no server running)
```

Marks: `✓` healthy, `!` warning (server still boots; exit code stays 0),
`✗` broken (doctor exits 1). A port that is *already serving* a healthy
xiaoguai is a `✓` with a note, not an error.

## Method by method

| Method | Install command | Expected output | Smoke test |
|---|---|---|---|
| **pip** (recommended) | `pip install xiaoguai` (PEP 668 systems: `pipx install xiaoguai`) | pip reports `Successfully installed xiaoguai-…`; `xiaoguai --version` prints the version | `xiaoguai doctor` → table above, then `xiaoguai serve` → `✓ xiaoguai running at http://localhost:7600`; `curl http://localhost:7600/healthz` → `ok` |
| **.deb** | `sudo apt install ./xiaoguai-cli_*_amd64.deb` | dpkg configures the package and the systemd unit starts | `systemctl status xiaoguai-core --no-pager` → `active (running)`; `curl http://localhost:7600/healthz` → `ok`; `xiaoguai doctor` |
| **.rpm** | `sudo rpm -i xiaoguai-cli-*.x86_64.rpm` | scriptlets create the `xiaoguai` user and start the unit | same as .deb |
| **tarball** | extract, then `sudo bash scripts/install.sh` | installer prints the unit + binary locations | same as .deb |
| **from source** | `cargo install --path crates/xiaoguai-cli --locked` | cargo finishes with `Installed package …` | `xiaoguai chat --mock --prompt 'hello'` (no network), then `xiaoguai doctor` |
| **Docker** | `docker compose -f deploy/docker-compose.yml up --build` | compose logs show `serve: api listening` | `curl http://localhost:7600/healthz` → `ok` |

## First run, in order

1. `xiaoguai doctor` — everything below `✗` must be fixed first.
2. `xiaoguai serve` — on success it prints:

   ```text
   ✓ xiaoguai running at http://localhost:7600
     Open the chat UI at http://localhost:7600/ — or send a first message: xiaoguai repl
   ```

   If the port is taken you get three remedies (check `healthz` for an
   already-running instance / `XIAOGUAI_SERVER__PORT=7601` / `lsof -i :7600`)
   instead of a bare error.
3. No providers yet? The boot banner says so and offers the two paths:
   local (`ollama pull qwen2.5-coder`) or `xiaoguai init` for a cloud key.
4. Re-run `xiaoguai doctor` — all `✓` (an `!` for an un-pulled Ollama model
   is fine until the first real chat).

## Run it as a daemon

One command, no unit files to hand-edit:

```bash
sudo xiaoguai service install   # Linux: systemd unit + xiaoguai user + dirs
xiaoguai service install        # macOS: per-user launchd agent (no root)
xiaoguai service status         # systemctl status / launchctl list
xiaoguai service uninstall      # stop + remove; data stays in place
```

`--print-only` on `install` renders the unit/plist and target paths without
touching the system. Windows is not supported — use Docker or WSL.

macOS logs land in `~/Library/Logs/xiaoguai/`; Linux logs in
`journalctl -u xiaoguai-core`.
