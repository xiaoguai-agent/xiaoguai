# Release signing runbook

**Audience:** end users who want to verify downloaded tarballs, and maintainers
who need to understand or update the signing pipeline.

---

## Background

Since v1.1.6.3 every xiaoguai release tarball ships three provenance artefacts:

| File | Purpose |
|---|---|
| `xiaoguai-<ver>-<arch>.tar.gz.sig` | Detached cosign signature (Sigstore Rekor-anchored) |
| `xiaoguai-<ver>-<arch>.tar.gz.pem` | Short-lived signing certificate chain (OIDC-backed) |
| `xiaoguai-<ver>-<arch>.tar.gz.bundle` | All-in-one Sigstore bundle (preferred for future tooling) |
| `multiple.intoto.jsonl` | SLSA Level 3 provenance attestation (in-toto envelope) |

Both are produced in GitHub Actions with no long-lived signing keys.
The trust root is the GitHub Actions OIDC token issued for this repository.

---

## End-user verification

### Prerequisites

Install `cosign` and `slsa-verifier`:

```sh
# cosign — pick the latest release from https://github.com/sigstore/cosign/releases
COSIGN_VERSION=v2.4.1
curl -fsSL \
  "https://github.com/sigstore/cosign/releases/download/${COSIGN_VERSION}/cosign-linux-amd64" \
  -o /usr/local/bin/cosign
chmod +x /usr/local/bin/cosign

# slsa-verifier — https://github.com/slsa-framework/slsa-verifier/releases
SLSA_VERSION=v2.6.0
curl -fsSL \
  "https://github.com/slsa-framework/slsa-verifier/releases/download/${SLSA_VERSION}/slsa-verifier-linux-amd64" \
  -o /usr/local/bin/slsa-verifier
chmod +x /usr/local/bin/slsa-verifier
```

On macOS use the `-darwin-amd64` / `-darwin-arm64` suffix.
On Windows use the `.exe` variants; see each project's README.

### Download release files

```sh
VERSION=v1.1.6.3          # replace with the version you want
ARCH=x86_64-unknown-linux-gnu   # or aarch64-unknown-linux-gnu
BASE="xiaoguai-${VERSION#v}-${ARCH}"
RELEASE="https://github.com/xiaoguai-agent/xiaoguai/releases/download/${VERSION}"

curl -fsSLO "${RELEASE}/${BASE}.tar.gz"
curl -fsSLO "${RELEASE}/${BASE}.tar.gz.pem"
curl -fsSLO "${RELEASE}/${BASE}.tar.gz.sig"
curl -fsSLO "${RELEASE}/multiple.intoto.jsonl"
```

### Verify with cosign

```sh
cosign verify-blob \
  --certificate  "${BASE}.tar.gz.pem" \
  --signature    "${BASE}.tar.gz.sig" \
  --certificate-identity-regexp \
    'https://github.com/xiaoguai-agent/xiaoguai/.github/workflows/release-tarball.yml@.*' \
  --certificate-oidc-issuer \
    'https://token.actions.githubusercontent.com' \
  "${BASE}.tar.gz"
```

Expected output (last line):
```
Verified OK
```

What this checks:
- The signature was produced during a real GitHub Actions run.
- The OIDC token was issued for the `release-tarball.yml` workflow in this
  repository (the `--certificate-identity-regexp` pins the workflow path).
- The signing certificate chains up to the Sigstore public-good Fulcio CA.
- A transparency log entry exists in Rekor, making the signing event auditable.

### Verify with slsa-verifier (SLSA L3)

```sh
slsa-verifier verify-artifact "${BASE}.tar.gz" \
  --provenance-path multiple.intoto.jsonl \
  --source-uri  github.com/xiaoguai-agent/xiaoguai \
  --source-tag  "${VERSION}"
```

Expected output:
```
Verified SLSA provenance
```

What this checks additionally:
- The build was performed hermetically in an environment controlled by
  `slsa-github-generator`, not by arbitrary code in this repository.
- The provenance payload lists the exact git commit and tag used.
- The in-toto envelope is signed by the generator's ephemeral key, which
  itself was provisioned via GitHub Actions OIDC.

---

## How it works (pipeline overview)

```
push v* tag
    │
    ├─ build (x86_64)   ─┐
    ├─ build (aarch64)  ─┤──► sign ──► publish
    │                    │      │
    └─────────────────── ┘      └──► signatures artifact
                                           │
    hash ──► provenance (slsa-generator) ──► multiple.intoto.jsonl → release
                                     │
                               verify-provenance ──► passes before publish
```

1. **build** jobs compile the Rust binary for each target and upload the
   tarballs as workflow artifacts.
2. **sign** downloads each tarball and calls `cosign sign-blob --yes`, which
   requests a short-lived certificate from Sigstore Fulcio backed by the
   GitHub Actions OIDC token, signs the blob, and records the operation in
   Rekor.  No private key is stored anywhere.
3. **hash** computes `sha256sum` of all tarballs and base64-encodes the result
   for hand-off to the SLSA generator.
4. **provenance** runs the
   `slsa-framework/slsa-github-generator` reusable workflow (pinned to
   `v2.0.0`).  The generator builds an in-toto provenance statement, signs it
   with an ephemeral key, uploads the attestation to Rekor, and produces
   `multiple.intoto.jsonl`.
5. **verify-provenance** runs `slsa-verifier` inside the same pipeline as a
   sanity check before any assets are exposed to users.
6. **publish** collects all artefacts and attaches them to the GitHub Release
   via `softprops/action-gh-release`.

---

## Maintainer notes

### Pinning cosign-installer

The workflow uses `sigstore/cosign-installer@v3` (a floating major-version
pin).  If a breaking change lands in v3, pin to a specific commit SHA:

```yaml
uses: sigstore/cosign-installer@11086d25041f77fe8fe96ad2e43c58e9bd5c1c4f  # v3.x.y
```

Check the latest at https://github.com/sigstore/cosign-installer/releases.

### Pinning slsa-github-generator

The workflow pins `slsa-framework/slsa-github-generator` to `@v2.0.0`.
The slsa-framework team does NOT support floating-major tags for the reusable
workflow because callers are running untrusted code in the generator's
isolated environment — you must always pin to an exact version or commit SHA.

To upgrade:
1. Check the [slsa-github-generator releases](https://github.com/slsa-framework/slsa-github-generator/releases).
2. Update the `uses:` line to `@v<new-version>`.
3. Verify the release notes for any breaking changes to the `with:` inputs.

### Rotating trust if the repository is transferred

If the repository is moved to a new org (e.g. `xiaoguai-agent/xiaoguai` →
`another-org/xiaoguai`), the certificate identity in the cosign verification
command must be updated to match the new workflow URL.  Old signatures created
under the previous org will still verify against the old identity; new releases
will carry the new identity.

### Offline / air-gapped verification

`cosign verify-blob` needs to reach Rekor (`rekor.sigstore.dev`) to confirm
the transparency log entry.  In an air-gapped environment:

```sh
cosign verify-blob \
  --insecure-ignore-tlog \   # skip Rekor check
  --certificate  "${BASE}.tar.gz.pem" \
  --signature    "${BASE}.tar.gz.sig" \
  --certificate-identity-regexp '...' \
  --certificate-oidc-issuer '...' \
  "${BASE}.tar.gz"
```

Note: skipping the transparency log weakens the guarantee against key
compromise between signing and verification.  Use only when necessary.

`slsa-verifier` does not require network access once the provenance file is
downloaded locally.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `cosign: command not found` | cosign not installed | Follow prerequisites above |
| `Error: certificate signed by untrusted authority` | Wrong `--certificate-oidc-issuer` | Use `https://token.actions.githubusercontent.com` exactly |
| `Error: provided subject does not match` | Tarball modified after signing | Do not use the artefact; report to maintainers |
| `FAILED: SLSA verification failed` | Provenance mismatch or wrong `--source-tag` | Ensure `--source-tag` matches the git tag exactly (e.g. `v1.1.6.3`) |
| `slsa-verifier: unable to find provenance` | Wrong `--provenance-path` | Download `multiple.intoto.jsonl` from the same release page |
