# xiaoguai (Python wrapper)

`pip install xiaoguai` — a thin Python launcher that bundles the Rust
[`xiaoguai`](https://github.com/xiaoguai-agent/xiaoguai) CLI binary
inside a platform-specific wheel.

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

[BUSL-1.1](./LICENSE). Same license as the upstream Rust project.
