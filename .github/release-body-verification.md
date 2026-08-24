<!-- __TAG__ is substituted with the tag by release-tarball.yml.
     Appended to every GitHub Release body by release-tarball.yml.
     Kept out of the workflow YAML so it can be edited without touching
     release plumbing, and so the CHANGELOG section can precede it. -->
## Verifying release artifacts

### cosign (keyless / Sigstore)

```sh
# Download the tarball + certificate + signature
VERSION="__TAG__"
ARCH=x86_64-unknown-linux-gnu   # or aarch64-unknown-linux-gnu
# Filenames keep the tag's leading 'v' (build-tarball.sh: xiaoguai-v${VERSION}-...)
BASE="xiaoguai-${VERSION}-${ARCH}"

cosign verify-blob \
  --certificate  "${BASE}.tar.gz.pem" \
  --signature    "${BASE}.tar.gz.sig" \
  --certificate-identity-regexp \
    'https://github.com/xiaoguai-agent/xiaoguai/.github/workflows/release-tarball.yml@.*' \
  --certificate-oidc-issuer \
    'https://token.actions.githubusercontent.com' \
  "${BASE}.tar.gz"
```

A `VERIFIED OK` message confirms the tarball was built by this workflow
and has not been modified since signing.

### SLSA Level 3 provenance

```sh
# The attestation is attached as multiple.intoto.jsonl
slsa-verifier verify-artifact "${BASE}.tar.gz" \
  --provenance-path multiple.intoto.jsonl \
  --source-uri github.com/xiaoguai-agent/xiaoguai \
  --source-tag "__TAG__"
```

See [docs/runbooks/release-signing.md](https://github.com/xiaoguai-agent/xiaoguai/blob/main/docs/runbooks/release-signing.md)
for full installation instructions and trust-root details.
