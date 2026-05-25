## Summary

<!-- 2–4 bullet points describing what this PR does and why. -->

-
-

## Motivation

<!-- Link the issue(s) this closes. e.g. Closes #123 -->

Closes #

## Type of change

<!-- Check all that apply. -->

- [ ] `feat` — new capability
- [ ] `fix` — bug fix
- [ ] `docs` — documentation only
- [ ] `chore` — tooling, dependencies, CI
- [ ] `test` — tests only
- [ ] `perf` — performance improvement
- [ ] `ci` — CI / CD pipeline

## Wave-3 subsystem(s) touched

<!-- Check all that apply. -->

- [ ] HotL / policy store
- [ ] outcomes / OutcomeRecorder
- [ ] packs / SkillPackRepository
- [ ] watch / file-watcher
- [ ] anomaly detection
- [ ] rate-limit / token budget
- [ ] IM-adapter (Slack / Teams / Discord)
- [ ] observability / tracing / metrics
- [ ] cli / admin tooling
- [ ] admin-ui (web)
- [ ] chat-ui (web)
- [ ] none / infra only

## Breaking changes

<!-- Describe any breaking change to API, wire format, config schema, or observable behaviour. Write "None" if clean. -->

None

## Test plan

- [ ] `cargo test --workspace` passes locally
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo fmt --all -- --check` passes
- [ ] Integration / e2e tests updated or verified (describe below)
- [ ] Manual smoke test performed (describe below)

<!-- Describe integration / e2e steps and manual smoke test here -->

## Docs updated

- [ ] mdbook chapter added / updated (`docs/src/…`)
- [ ] Runbook added / updated (`docs/runbooks/…`)
- [ ] ADR added / updated (`docs/adr/…`)
- [ ] CHANGELOG.md entry added

## Deployment notes

- [ ] No new env vars / config keys
- [ ] New env var(s) added — documented in `docs/config-reference.md`
- [ ] DB migration added (`migrations/YYYYMMDD_*.sql`)
- [ ] Helm / Terraform / Kustomize manifests updated
- [ ] Post-deploy verification step required (describe below)

<!-- Describe post-deploy verification here if needed -->
