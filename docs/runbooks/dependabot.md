# Dependabot runbook

Dependabot opens automated PRs for three ecosystems. This runbook explains the
merge gates, how to handle security alerts, and when to escalate.

---

## Ecosystems covered

| Ecosystem | Directory | Cadence | Open-PR cap | Group strategy |
|---|---|---|---|---|
| cargo | `/` | Daily | 10 | Minor + patch grouped; major separate |
| npm (pnpm) | `/frontend` | Weekly | 5 | Minor + patch grouped; major separate |
| github-actions | `/` | Weekly | 5 | All updates grouped |

---

## Merge gates

All Dependabot PRs must pass the same required CI status checks as regular PRs:

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm -r typecheck        # for npm PRs only
```

Do **not** merge a PR with failing checks even if the change looks trivial —
dependency updates occasionally expose pre-existing latent failures.

### Typical minor/patch PR (low risk)

1. CI green → review diff in `Cargo.lock` / `pnpm-lock.yaml`.
2. Check the crate/package changelog for any behaviour change.
3. Approve + squash-merge. No additional review required.

### Major-version PR (higher risk)

Each major bump gets its own PR (not grouped). Steps:

1. Read the upstream migration guide / CHANGELOG.
2. Look for breaking API changes that touch xiaoguai code.
3. Run `cargo test --workspace` locally if the changelog mentions breaking changes.
4. If tests need updating, push fixup commits onto the Dependabot branch:
   ```bash
   git fetch origin
   git checkout dependabot/cargo/<crate>-<new-ver>
   # make changes
   git push origin dependabot/cargo/<crate>-<new-ver>
   ```
5. Merge only after all checks pass.

---

## Security alerts

GitHub will flag Dependabot PRs with a **Security** badge when a CVE is
attached. Priority rules:

| Severity | Action | Deadline |
|---|---|---|
| Critical / High | Merge immediately (skip weekly cadence) | Same business day |
| Medium | Merge in the next scheduled batch | Within 1 week |
| Low | Merge at convenience | Within 1 sprint |

If a Critical/High alert appears and Dependabot has not yet opened a PR
(e.g. no patch exists upstream), open a GitHub Security Advisory and notify
the team via the on-call channel.

---

## Handling pnpm workspaces

The npm ecosystem entry points at `/frontend` which contains
`pnpm-workspace.yaml`. Dependabot reads `package.json` files under that tree
and updates both `admin-ui/package.json` and `chat-ui/package.json` in a
single PR when both share the same dependency.

`versioning-strategy: increase` means Dependabot will bump the version range
in `package.json` (e.g. `^1.2.0` → `^1.3.0`) rather than widening it, keeping
the lockfile deterministic.

After merging a pnpm PR, verify the lockfile is consistent:

```bash
cd frontend && pnpm install --frozen-lockfile
```

---

## Escalation

If a Dependabot PR introduces a test regression that cannot be fixed within
two business days, close the PR with the label **deferred** and open a
tracking issue linking to the CVE or changelog entry. Do not leave broken
PRs open — they block the open-PR cap and delay future updates.

For supply-chain incidents (compromised package, malicious update):

1. Immediately revert or close the PR.
2. Run `cargo deny check bans` / `cargo audit` to assess blast radius.
3. Notify the security alias and open a private Security Advisory on GitHub.

---

## Configuration file

`.github/dependabot.yml` — do not edit the schedule or grouping without
updating this runbook.
