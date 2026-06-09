# xiaoguai (Python wrapper)

`pip install xiaoguai` — a thin Python launcher that bundles the Rust
[`xiaoguai`](https://github.com/xiaoguai-agent/xiaoguai) CLI binary
inside a platform-specific wheel.

> On Debian 12 / Ubuntu 24 and other PEP 668 "externally-managed" systems,
> `pip install` into the system Python is blocked. Use **pipx** instead:
> `sudo apt install -y pipx && pipx ensurepath && pipx install xiaoguai`.

After install:

```bash
xiaoguai --help
xiaoguai chat --mock --prompt "hello"
```

The console script forwards every argument to the bundled native
binary. There is no Python agent logic in this package — it exists
so `pip` users have an install path alongside Cargo, Homebrew, and
the standalone tarball.

## Supported platforms

The CI matrix produces wheels for:

| Target triple                  | Wheel tag (approx.)           |
|--------------------------------|-------------------------------|
| `aarch64-apple-darwin`         | `macosx_11_0_arm64`           |
| `x86_64-apple-darwin`          | `macosx_10_12_x86_64`         |
| `x86_64-unknown-linux-gnu`     | `manylinux_2_28_x86_64`       |
| `aarch64-unknown-linux-gnu`    | `manylinux_2_28_aarch64`      |

Other platforms (Alpine / musl, Windows, FreeBSD) are out of scope
for v1.1.7. Build from source instead:

```bash
cargo install --path crates/xiaoguai-cli
```

## HTTP client (wave-3)

`pip install 'xiaoguai[client]'` — adds `xiaoguai.client.XiaoguaiClient`, a
synchronous HTTP client for the `xiaoguai-api` REST server (requires `httpx>=0.25`).

> **Note:** the client snippets below predate the single-user pivot (DEC-033).
> The live API now serves on `:7600` with optional HTTP Basic auth and no
> tenant scoping. The bundled binary launcher above is the supported path;
> treat these examples as illustrative pending a client refresh.

### Covered endpoints (v1.2.x)

| Domain | Methods |
|---|---|
| **HotL** | `list_hotl_policies`, `create_hotl_policy`, `delete_hotl_policy` |
| **Outcomes** | `record_outcome`, `outcomes_summary`, `outcomes_timeseries` |
| **Skills** | `list_skill_catalog`, `list_installed_skills`, `install_skill`, `uninstall_skill` |

### Quick start

```python
from xiaoguai.client import XiaoguaiClient

with XiaoguaiClient("http://localhost:7600", token="my-bearer-token") as c:
    # HotL — boundary policy admin
    policy = c.create_hotl_policy(
        tenant_id="my-tenant-uuid",
        scope="llm_call",
        window_seconds=3600,
        max_count=100,
        escalate_to="ops@example.com",
    )
    policies = c.list_hotl_policies(tenant_id="my-tenant-uuid", scope="llm_call")
    c.delete_hotl_policy(policy.id)

    # Outcomes — ROI telemetry
    c.record_outcome(
        tenant_id="my-tenant",
        agent_name="sales-bot",
        kind="revenue_usd",
        value=1500.0,
        description="Closed enterprise deal",
    )
    summary = c.outcomes_summary(tenant_id="my-tenant", range="7d")
    ts = c.outcomes_timeseries(tenant_id="my-tenant", range="30d", kind="hours_saved")

    # Skills — pack marketplace
    catalog = c.list_skill_catalog()
    pack = c.install_skill(tenant_id="my-tenant", pack_slug="rag-legal")
    installed = c.list_installed_skills(tenant_id="my-tenant")
    c.uninstall_skill(pack.id)
```

### Error handling

```python
from xiaoguai.client import (
    XiaoguaiNotFoundError,
    XiaoguaiValidationError,
    XiaoguaiConflictError,
)

try:
    c.install_skill(tenant_id="t1", pack_slug="rag-legal")
except XiaoguaiConflictError:
    print("already installed")
except XiaoguaiNotFoundError:
    print("unknown pack slug")
```

### Typed models

`HotlPolicy`, `HotlVerdict`, `OutcomeRecord`, `OutcomeSummary`,
`OutcomeTimeseries`, `InstalledSkillPack`, `SkillPackEntry` — all frozen
dataclasses with `from_dict` class methods.

## Troubleshooting

If `xiaoguai` after a fresh install prints "native binary not
bundled", the wheel matched on architecture but its package data is
empty (rare — usually an `sdist` install rather than a wheel). Set
`XIAOGUAI_PY_DEBUG=1` to see the resolution path the launcher tried.

## Documentation

Full documentation, configuration, and architecture notes live in
the upstream repository — see the
[main README](https://github.com/xiaoguai-agent/xiaoguai#readme).

## License

[Apache-2.0](./LICENSE). Same license as the upstream Rust project.
