#!/usr/bin/env bash
# Verify every place that hard-codes the Rust toolchain agrees with
# rust-toolchain.toml, which is the single source of truth.
#
# Why: bumping the MSRV is a coordinated edit across a dozen files. On the
# 1.93 -> 1.94 bump (#494) the two deploy Dockerfiles were missed on the first
# pass and only found by reading ADR-0021's change list — a grep of the
# workflows alone does not surface them. A stale ARG RUST_VERSION does not
# break CI; it breaks the container build later, far from the change.
set -uo pipefail

fail=0
note() { printf '  %-52s %s\n' "$1" "$2"; }
bad()  { printf '::error::%s\n' "$1"; fail=1; }

TOOLCHAIN=$(grep -E '^channel' rust-toolchain.toml | sed -E 's/.*"([^"]+)".*/\1/')
[ -n "$TOOLCHAIN" ] || { bad "cannot read channel from rust-toolchain.toml"; exit 1; }
MINOR="${TOOLCHAIN%.*}"          # 1.94.0 -> 1.94

echo "source of truth: rust-toolchain.toml = ${TOOLCHAIN} (minor ${MINOR})"
echo

# 1. workspace MSRV --------------------------------------------------------
RV=$(grep -E '^rust-version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
note "Cargo.toml rust-version" "$RV"
[ "$RV" = "$MINOR" ] || bad "Cargo.toml rust-version is '$RV', expected '$MINOR'"

# 2. pinned action versions ------------------------------------------------
while read -r pin; do
  [ -z "$pin" ] && continue
  v="${pin#*@}"
  case "$v" in
    stable|nightly) continue ;;   # handled by the `toolchain:` input below
  esac
  [ "$v" = "$TOOLCHAIN" ] || bad "workflow pin dtolnay/rust-toolchain@$v != $TOOLCHAIN"
done < <(grep -rho 'dtolnay/rust-toolchain@[A-Za-z0-9.]*' .github/workflows/ | sort -u)
note "dtolnay/rust-toolchain@ pins" "$(grep -rho 'dtolnay/rust-toolchain@[0-9.]*' .github/workflows/ | sort -u | tr '\n' ' ')"

# 3. string-form `toolchain:` inputs ---------------------------------------
while read -r line; do
  [ -z "$line" ] && continue
  v=$(echo "$line" | sed -E "s/.*toolchain: *'?([0-9.]+)'?.*/\1/")
  [ "$v" = "$MINOR" ] || [ "$v" = "$TOOLCHAIN" ] \
    || bad "workflow 'toolchain: $v' != $MINOR/$TOOLCHAIN"
done < <(grep -rh "toolchain: *'" .github/workflows/ || true)
note "workflow toolchain: inputs" "$(grep -rh "toolchain: *'" .github/workflows/ | tr -d ' ' | tr '\n' ' ')"

# 4. Dockerfiles -----------------------------------------------------------
while read -r hit; do
  [ -z "$hit" ] && continue
  f="${hit%%:*}"; v="${hit##*=}"
  [ "$v" = "$MINOR" ] || [ "$v" = "$TOOLCHAIN" ] \
    || bad "$f has ARG RUST_VERSION=$v, expected $MINOR"
done < <(grep -rn 'ARG RUST_VERSION=' deploy/ 2>/dev/null | sed 's/:[0-9]*:/:/' || true)
note "deploy Dockerfiles ARG" "$(grep -rho 'ARG RUST_VERSION=[0-9.]*' deploy/ | sort -u | tr '\n' ' ')"

# 5. install docs ----------------------------------------------------------
# Four docs must state the same minimum — they have drifted apart before by
# being updated piecemeal.
DOCS="README.md deploy/README.md docs/user-guide/quickstart.md python/xiaoguai/README.md"
for d in $DOCS; do
  [ -f "$d" ] || { bad "install doc missing: $d"; continue; }
  found=$(grep -oE 'Rust 1\.[0-9]+' "$d" | sort -u | sed 's/Rust //' | tr '\n' ' ')
  if [ -z "$found" ]; then
    bad "$d states no minimum Rust version (must say 'Rust $MINOR')"
  else
    for v in $found; do
      [ "$v" = "$MINOR" ] || bad "$d says 'Rust $v', expected 'Rust $MINOR'"
    done
    note "$d" "Rust $found"
  fi
done

echo
if [ "$fail" -eq 0 ]; then
  echo "OK: every toolchain reference agrees with ${TOOLCHAIN}"
else
  echo "::error::toolchain references are inconsistent — see above"
  echo "When bumping the MSRV, update ALL of the places listed above together."
  echo "See docs/architecture/adr/0023-rust-toolchain-bump-194.md for the full list."
fi
exit "$fail"
