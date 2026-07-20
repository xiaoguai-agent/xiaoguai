# History rewrite — 2026-07-20

On 2026-07-20 the full commit history was rewritten to converge every commit on
a single owner identity, `wei <zhouwei008@gmail.com>`. Two older addresses
(`boyi.liang@gmail.com`, `98p@sina.com`) were mapped onto it via `.mailmap`
and `git filter-repo --mailmap`.

This note exists because the rewrite has two consequences that are visible from
outside the repo and cannot be undone.

## What changed, and what did not

Every commit SHA changed. **No file content changed.** This was verified before
publishing: all 21 branch trees and all 129 tag trees hashed byte-identical
before and after, and commit/branch/tag counts were preserved (935 / 21 / 129).
`main`'s tree hash was unchanged across the rewrite.

`dependabot[bot]` and `forensics@localhost` commits were deliberately left
alone — they are not the owner's identity. The `GitHub <noreply@github.com>`
*committer* on squash-merge commits was likewise left as-is: it is accurate.

## Consequence 1 — release provenance records pre-rewrite SHAs

The SLSA provenance attached to each GitHub Release (`multiple.intoto.jsonl`)
records the source commit the artifacts were built from. For releases cut
before 2026-07-20 those SHAs no longer match what the tag points to. Example,
v1.34.0:

    configSource.digest.sha1 = f48152ffe728e601256d2252ce4cd44627423abf
    v1.34.0 now points to    = 4ab0fac893c63d300a900744faf4f333d0347655

**This is not being corrected, and the mismatch is less severe than it looks:**

- The sigstore signatures (`.sig` / `.pem` / `.bundle`) cover the **artifact
  bytes**, which are unchanged. Artifact verification still succeeds.
- The recorded SHAs remain **fetchable from GitHub**: the ~397 `refs/pull/*`
  refs still carry the pre-rewrite history and cannot be deleted. The
  provenance chain is resolvable, just no longer reachable via the tag.
- The provenance is not false. At build time the tag *did* point at that
  commit. The repository moved afterwards; the signed record did not.

Correcting it would mean rebuilding from the new SHAs and re-signing, which
produces **different binaries with different digests** and would require
replacing already-published assets across 46 releases. Anyone who had already
downloaded and recorded a checksum would find it no longer matches. That is a
worse outcome than a documented mismatch, so the records are left intact.

Releases cut after 2026-07-20 are unaffected: the workflow reads the tag at
build time, so their provenance is correct by construction.

## Consequence 2 — 313 merge commits lost their "Verified" badge

Those signatures were GitHub's own web-flow key, applied to commits created
through the GitHub UI/API. They attested *"GitHub processed this merge"* — not
authorship or code integrity. `git filter-repo` strips such headers cleanly, so
the commits show no badge rather than an invalid one. No tag was ever signed,
and none of the owner's own commits had ever been signed, so no personal
attestation was lost. GitHub only signs commits it creates, so this cannot be
restored retroactively.

Going forward, commits and tags are signed with a dedicated SSH key
(`gpg.format = ssh`), which is a stronger claim than the web-flow signature
ever was: it attests authorship, not merely that GitHub handled the merge.

## If you have an old clone

Pre-rewrite clones share no commits with the current history. Re-clone, or:

    git fetch origin --prune --tags --force
    git reset --hard origin/main
