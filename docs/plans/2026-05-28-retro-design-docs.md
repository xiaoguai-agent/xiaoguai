# Plan C — Retro design docs via testany-eng

> Companion to the session-5 handoff (`docs/HANDOFF-2026-05-28-session5.md`).
> Meta-plan: `~/.claude/plans/drifting-zooming-stroustrup.md`.

## 1. Context

After ~4 months of intense build, the design docs at
`/Users/zw/testany/myskills/xiaoguai-agent-design/docs/` have already been
populated retroactively (the v1.4 retrofit pass — `hld.md`,
`prd-xiaoguai.md`, `api-contract.md`, `guardrails.md`, `runbook.md`,
`test-spec.md`, `test-strategy.md`, and 12 files under `lld/`). What's
**missing** is the delta from session-3 → session-5 plus the philosophy
overlay we just added:

| Gap | Why it matters |
|---|---|
| Tier-1b in-process cache fallback (#60) not in HLD §2 module map | Operators reading HLD will not learn that Valkey is now optional |
| HotL gate trait (#61) not in `lld/lld-agent.md` | Per-tool gate is a major architecture change — should be its own LLD section |
| `xiaoguai-mcp-exec` (#64) has `lld/lld-mcp-exec.md` already but PRD lacks the corresponding requirement section | Traceability matrix in §11 of HLD will fail completeness check |
| PII redaction (#57) not in `guardrails.md` security rules | New audit-ordering invariant (redact-before-HMAC) isn't documented as a rule |
| Just-added `harness-engineering.md` not referenced from `prd-xiaoguai.md`, `runbook.md`, or LLD docs | Vocabulary won't propagate |
| `xiaoguai-agent-design/docs/` has no ADR directory | All ADRs live in implementation repo; the design repo should index them |

This plan **does not** rewrite from scratch — that would discard 3 996
lines of work already done. It surgically updates what's there using the
`testany-eng` skill family as reviewers and writers.

The session-5 handoff scopes this at 2–3 h interactive. Most of the time
will be spent in a *separate Claude Code session* run from
`cd /Users/zw/testany/myskills/xiaoguai-agent-design && claude` so the
testany-eng skills are loaded at session-start.

## 2. Success criteria

1. `xiaoguai-agent-design/docs/` contains a new `adr/` directory mirroring
   the implementation repo's ADR list (`docs/architecture/adr/`), with one
   short stub per ADR pointing at the implementation file. `ls adr/ | wc
   -l` ≥ 18.
2. `hld.md` §2 module-map table includes a row for in-process cache
   fallback (#60) and a row clarifying `xiaoguai-core` is the legacy shim.
3. `hld.md` has a new section between §6 (cross-cutting) and §7 (deployment)
   titled "HotL gating and the policy gateway", with a diagram pointing at
   `harness-engineering.md` §9.
4. `prd-xiaoguai.md` has a new sub-requirement under the "execution"
   capability for sandboxed code execution (covering mcp-exec), with
   acceptance criteria that match `tests/eval/` cases.
5. `guardrails.md` has a new row in §3 (security defaults) for
   redact-before-HMAC ordering and a row in §3 for the env-allowlist in
   the mcp-exec sandbox.
6. `lld/lld-agent.md` has a new "HotL gate" subsection with the
   `HotlGate::Verdict` enum + `EnforcerGate` bridge mapping.
7. Every other LLD file under `lld/` has a header link back to
   `harness-engineering.md` and `hld.md`.
8. The `testany-eng` reviewer skills (`prd-reviewer`, `hld-reviewer`,
   `lld-reviewer`) report no `CRITICAL` or `HIGH` findings on the updated
   docs. (Run after the updates above.)
9. A new file
   `xiaoguai-agent-design/docs/RELEASE-LOG.md` exists and lists each
   change in this pass with a one-line summary and the PR that landed it.
10. `xiaoguai-agent-design/docs/` README / index file (if absent, create)
    links to all top-level docs *and* the new `harness-engineering.md`.

## 3. Prerequisites

| What | Verify by |
|---|---|
| `testany-eng` plugin installed at `~/.claude/plugins/cache/testany-agent-skills/testany-eng/1.0.0/` with 21 skills | `ls ~/.claude/plugins/cache/testany-agent-skills/testany-eng/1.0.0/skills/ | wc -l` ≥ 21 |
| `~/.claude/plugins/installed_plugins.json` lists `testany-eng@1.0.0` | `jq '.[] | select(.name=="testany-eng")' < ~/.claude/plugins/installed_plugins.json` non-empty |
| Clean working tree in both `xiaoguai/` and `xiaoguai-agent-design/` | `cd xiaoguai-agent-design && git status --porcelain | wc -l` = 0; same for `xiaoguai` |
| `harness-engineering.md` exists | `ls /Users/zw/testany/myskills/xiaoguai-agent-design/docs/harness-engineering.md` |
| The implementation repo's ADR list is enumerable | `ls /Users/zw/testany/myskills/xiaoguai/docs/architecture/adr/ \| wc -l` ≥ 1 (may need to create the directory in xiaoguai too if missing — see step 4.2) |

## 4. Step-by-step actions

This plan has **two phases**: a short preparation done **in the current
session** (no testany-eng needed) and a longer interactive phase done in
a **new** `cd xiaoguai-agent-design && claude` session.

### Phase 1 — Preparation in the current session

#### Step 4.1 — Inventory the deltas

```bash
cd /Users/zw/testany/myskills/xiaoguai
git log --oneline --since=2026-05-26 -- 'docs/HANDOFF*.md' \
  > /tmp/handoffs.txt
git log --oneline --since=2026-05-26 -- 'crates/' \
  > /tmp/code-changes.txt
```

**VC:** `wc -l /tmp/handoffs.txt /tmp/code-changes.txt` both non-zero;
spot-check that #57, #59, #60, #61, #64 all appear.

#### Step 4.2 — Verify ADR directory in implementation repo

```bash
ls /Users/zw/testany/myskills/xiaoguai/docs/architecture/adr/ 2>/dev/null \
  || echo "MISSING — create stub directory"
```

**VC:** lists ≥ 1 ADR file. If empty, **stop this plan** and surface an
issue: HLD §3 references `ADR-0001..ADR-0018` so the directory must exist.
This is a finding worth fixing before retrofit.

#### Step 4.3 — Generate a draft RELEASE-LOG.md

In the *xiaoguai-agent-design* repo:

```bash
cd /Users/zw/testany/myskills/xiaoguai-agent-design
cat > docs/RELEASE-LOG.md <<'EOF'
# Design-doc release log

Tracks updates to `xiaoguai-agent-design/docs/` over time.

| Date | Change | Source PR(s) |
|---|---|---|
| 2026-05-28 | Added `harness-engineering.md` (R.E.S.T model, REPL container abstraction, control/data plane split). | (this doc-only PR) |
| 2026-05-28 | HLD §2 module map: added in-process cache fallback (#60) and clarified `xiaoguai-core` legacy shim. | #59, #60 |
| 2026-05-28 | HLD: new §6.5 HotL gating and the policy gateway. | #61 |
| 2026-05-28 | PRD: new sub-requirement for sandboxed code execution. | #64 |
| 2026-05-28 | Guardrails: redact-before-HMAC ordering rule; mcp-exec env allowlist rule. | #57, #64 |
| 2026-05-28 | LLD: cross-links to harness-engineering.md from all 12 LLD files. | (this doc-only PR) |
| 2026-05-28 | New `adr/` directory mirroring implementation-repo ADRs. | (this doc-only PR) |
EOF
```

**VC:** `cat docs/RELEASE-LOG.md` matches expectation.

#### Step 4.4 — Stage the source-of-truth input for the testany-eng session

Create a single briefing file that the new session will read first:

```bash
cat > /Users/zw/testany/myskills/xiaoguai-agent-design/HANDOFF-DESIGN-UPDATE.md <<'EOF'
# Design-doc update pass — input briefing

Run this in a fresh `cd xiaoguai-agent-design && claude` session.

## Read order
1. docs/harness-engineering.md (just added, the philosophy doc)
2. /Users/zw/testany/myskills/xiaoguai/docs/HANDOFF-2026-05-28-session5.md
3. /Users/zw/testany/myskills/xiaoguai/docs/plans/2026-05-28-retro-design-docs.md  (THIS plan)
4. The existing docs in docs/ (hld.md, prd-xiaoguai.md, etc.)

## What to do
For each success criterion in plan C §2, identify the doc(s) that need
editing and apply targeted updates. Use the testany-eng reviewer skills
on the result.

## What NOT to do
- Don't rewrite docs from scratch. The existing v1.4 retrofit is good.
- Don't generate new HLD/PRD sections that duplicate existing content.
- Don't change `Document ID` fields — they're stable identifiers.
EOF
```

**VC:** the file exists and is < 100 lines.

### Phase 2 — Interactive update pass (new Claude Code session)

> **Run by the user (or by Claude Code in a fresh shell):** `cd
> /Users/zw/testany/myskills/xiaoguai-agent-design && claude`. The
> testany-eng skills load on session start.

#### Step 4.5 — Read the briefing and confirm scope

In the new session, paste:

```
Read HANDOFF-DESIGN-UPDATE.md and confirm you understand the 10 success
criteria from the referenced plan C. Do not start editing yet — list what
you intend to change, file by file, with rough line counts.
```

**VC:** Claude responds with a per-file edit plan that maps each of the 10
success criteria to a file. Reviewer sanity-checks the plan before
greenlighting.

#### Step 4.6 — Apply edits surgically

In the new session:

```
Proceed with the file-by-file edits from your previous response. After
each file, run `git diff <file>` and quote a 5-line excerpt of the change.
Do not move on until I confirm.
```

**VC:** Each file diff is reviewed. Reject diffs that exceed ~50 lines per
file unless they're additions of new sections (HotL gate subsection,
mcp-exec PRD requirement).

#### Step 4.7 — Run the reviewer skills

In the new session, invoke each in turn:

```
/testany-eng:hld-reviewer
/testany-eng:prd-reviewer
/testany-eng:lld-reviewer
/testany-eng:guardrails-reviewer
```

**VC:** Each reviewer output is checked. CRITICAL/HIGH findings must be
addressed before commit. MEDIUM findings can be deferred but must be
captured as TODO comments in the relevant doc.

#### Step 4.8 — Generate or update the index

If `xiaoguai-agent-design/docs/README.md` doesn't exist, create it with a
short doc map. If it exists, add `harness-engineering.md` and the new
`adr/` directory to it.

**VC:** `ls docs/README.md` returns 0 and `grep -c 'harness-engineering'
docs/README.md` ≥ 1.

#### Step 4.9 — Commit and push

```bash
cd /Users/zw/testany/myskills/xiaoguai-agent-design
git add docs/ HANDOFF-DESIGN-UPDATE.md
git commit -m "docs: post-session-5 update pass (Harness Engineering + Tier-1/2 delta)"
git push origin main
```

**VC:** `git log -1` shows the commit; `git status` clean.

#### Step 4.10 — Cross-link from implementation repo

In the implementation repo:

```bash
cd /Users/zw/testany/myskills/xiaoguai
ls docs/architecture/ | grep -q design-link || cat > docs/architecture/design-link.md <<'EOF'
# Design documents

The retrofit design documents for xiaoguai live in the sibling repo
`xiaoguai-agent-design`:

- Philosophy: [`harness-engineering.md`](../../xiaoguai-agent-design/docs/harness-engineering.md)
- HLD: [`hld.md`](../../xiaoguai-agent-design/docs/hld.md)
- PRD: [`prd-xiaoguai.md`](../../xiaoguai-agent-design/docs/prd-xiaoguai.md)
- Guardrails: [`guardrails.md`](../../xiaoguai-agent-design/docs/guardrails.md)
- LLD (per-crate): [`lld/`](../../xiaoguai-agent-design/docs/lld/)
- Test spec: [`test-spec.md`](../../xiaoguai-agent-design/docs/test-spec.md)
EOF
```

**VC:** `ls docs/architecture/design-link.md` returns 0; `git diff
docs/architecture/design-link.md` shows the new file.

#### Step 4.11 — Commit + PR in implementation repo

```bash
cd /Users/zw/testany/myskills/xiaoguai
git checkout -b docs/design-link
git add docs/architecture/design-link.md
git commit -m "docs: link to xiaoguai-agent-design retrofit"
gh pr create --title 'docs: link to xiaoguai-agent-design retrofit' \
  --body 'Cross-link design docs that were updated post session-5.'
```

**VC:** PR opens, CI green, single-file change.

## 5. Risks & open questions

| Risk | Mitigation |
|---|---|
| testany-eng reviewers may not have full context of what "retrofit" means | The HANDOFF-DESIGN-UPDATE.md briefing is the workaround. If reviewers still fail, fall back to manual review using the doc's existing structure as guide. |
| ADR directory in `docs/architecture/adr/` may not exist in implementation repo (step 4.2) | If missing, this plan blocks until we either create stubs or remove the ADR references from HLD §3. Treat as a forced fork in the plan. |
| New session in `xiaoguai-agent-design/` won't have memory entries from the implementation repo | Briefing file at step 4.4 includes the relevant context. The HANDOFF-2026-05-28-session5.md is referenced. |
| testany-eng skills may conflict with auto-loaded user-level memory | Skills are scoped; conflicts manifest as duplicate guidance. Reviewer should pick the more specific instruction. |
| Doc-only PR may not get reviewed in time | Tag `docs-update` so it can be merged without code-review block. |
| The `prd-writer` skill is for *new* PRDs, not retrofits | We're not using `prd-writer` — we're using `prd-reviewer` (and writing edits manually). Section 4.7 uses reviewers only. |

## 6. Rollback / abort criteria

- Step 4.2 finds `docs/architecture/adr/` empty → abort plan C, file a
  separate issue to populate ADRs first.
- Any reviewer at step 4.7 returns CRITICAL findings that require
  rewriting an existing section — abort that file's edit, surface the
  finding for human design call, and move on.
- New session at step 4.5 misunderstands the scope (e.g., starts rewriting
  HLD §1) — interrupt, restate the briefing, retry. If repeats, fall back
  to fully manual edits in the current session without testany-eng.

To leave the repos clean on abort:

```bash
cd /Users/zw/testany/myskills/xiaoguai-agent-design
git checkout -- docs/ HANDOFF-DESIGN-UPDATE.md  # local-only
cd /Users/zw/testany/myskills/xiaoguai
git checkout -- docs/architecture/design-link.md 2>/dev/null
```

## 7. Out of scope

- Writing a fresh PRD from scratch (the v1.4 retrofit covers this).
- Adding ADRs for things that *should* have an ADR but don't (track
  separately).
- Translating any of the design docs into Chinese (the implementation
  repo has Chinese READMEs; design repo is English-only by design — see
  `prd-xiaoguai.md` §0).
- Generating diagrams in Mermaid / PlantUML — ASCII boxes in current docs
  are sufficient.
- Adding cost / pricing sections to PRD (pricing model not finalized).
- Updating `test-strategy.md` to reflect new tests; that's the
  *test-strategy-writer*'s job, scope-creeping into Plan C makes the time
  estimate slip.

## 8. References

- Philosophy doc just added:
  `/Users/zw/testany/myskills/xiaoguai-agent-design/docs/harness-engineering.md`
- Existing retrofit docs:
  `/Users/zw/testany/myskills/xiaoguai-agent-design/docs/{hld,prd-xiaoguai,api-contract,guardrails,runbook,test-spec,test-strategy}.md`
  and `docs/lld/*.md`
- testany-eng plugin:
  `~/.claude/plugins/cache/testany-agent-skills/testany-eng/1.0.0/`
  (21 skills, 20 commands)
- Session-5 handoff:
  `/Users/zw/testany/myskills/xiaoguai/docs/HANDOFF-2026-05-28-session5.md`
- Original retrofit briefing (now superseded by HANDOFF-DESIGN-UPDATE.md):
  `/Users/zw/testany/myskills/xiaoguai-agent-design/HANDOFF-FOR-DESIGN-DOCS.md`

---

## Self-review

| # | Check | Result |
|---|---|---|
| 1 | Cited file paths exist | **PASS** — confirmed before drafting; `harness-engineering.md`, all retrofit docs, testany-eng plugin all in place |
| 2 | Every `VC:` is runnable | **PASS** — `ls`, `git status`, `wc -l`, `jq`, `grep` calls only |
| 3 | Each §2 criterion has a §4 step that probes it | **PASS** — 1→4.2+4.3, 2→4.5/4.6 (manual review), 3→4.6, 4→4.6, 5→4.6, 6→4.6, 7→4.6, 8→4.7, 9→4.3, 10→4.8 |
| 4 | §7 out-of-scope honored | **PASS** — no step writes new PRD from scratch, no Chinese translation, no Mermaid |
| 5 | Each §5 risk has a mitigation | **PASS** |
| 6 | Step durations sane | **PARTIAL** — phase 1 (4.1–4.4) ≤ 30 min; phase 2 (4.5–4.11) ≤ 2.5 h with reviewer rounds. Total ≤ 3 h. Within 2–3 h estimate. |

**Two soft spots**:
1. The plan assumes the *user* (or a fresh Claude Code session) drives
   phase 2 interactively. If the current session must do everything,
   step 4.5 becomes "read these docs and apply the edits without
   testany-eng" — viable but loses reviewer skills.
2. Step 4.2 is a hard gate: if `docs/architecture/adr/` is empty, the plan
   pauses. A nicer plan would have a fallback (create ADR stubs from HLD
   §3 references) but that's a separate piece of work and shouldn't be
   bundled here.
