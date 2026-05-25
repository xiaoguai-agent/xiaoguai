# Open Source License Compliance Checklist

Source: Original work released under MIT License by xiaoguai contributors
License: MIT
Corpus role: Risk flagging — license obligations, copyleft triggers, patent traps

---

## Purpose

This checklist guides engineering and legal teams through open-source license compliance review
before shipping a product that incorporates third-party open-source software (OSS).

## 1. License Category Classification

### Category A — Permissive (Low Risk)
Minimal restrictions. Requires attribution but permits proprietary use and distribution.

| License | Attribution Required | Patent Grant | Notable Restriction |
|---|---|---|---|
| MIT | Yes (copyright notice) | None | None |
| BSD 2-Clause | Yes (copyright notice) | None | None |
| BSD 3-Clause | Yes + non-endorsement | None | No use of contributor names for endorsement |
| Apache 2.0 | Yes (NOTICE file) | Yes — explicit patent grant | Patent retaliation clause |
| ISC | Yes (copyright notice) | None | None |

### Category B — Weak Copyleft (Moderate Risk)
Copyleft applies only to modifications of the library itself; proprietary applications linking to
the library are generally permissible.

| License | Copyleft Scope | Dynamic Linking Safe? | Static Linking Safe? |
|---|---|---|---|
| LGPL 2.1 | Library modifications only | Generally yes | Requires user to relink |
| LGPL 3.0 | Library modifications + installation info | Generally yes | Requires user to relink |
| MPL 2.0 | File-level copyleft | Yes | Yes (with file-level disclosure) |
| EPL 2.0 | Module-level copyleft | Usually yes | Risk — consult counsel |

### Category C — Strong Copyleft (High Risk / Proprietary Blocker)
Copyleft extends to the entire program when combined with covered code. **Not safe for proprietary
products without a commercial license exception.**

| License | Key Trigger |
|---|---|
| GPL 2.0 | Distribution of combined work |
| GPL 3.0 | Distribution + patent and anti-tivoization |
| AGPL 3.0 | Distribution OR providing the work over a network |
| EUPL 1.2 | Distribution; compatible with several other copyleft licenses |

### Category D — Non-Commercial / Restricted
Not OSS by OSI definition. Prohibited in commercial products without vendor permission.

| License | Key Restriction |
|---|---|
| Creative Commons NC variants | Non-commercial use only |
| JSON License | Software shall be used for Good, not Evil (legal ambiguity) |
| Proprietary / Unlicensed | No rights granted; assume all rights reserved |
| SSPL 1.0 | Network service derivative works trigger copyleft |

## 2. Compliance Action Matrix

### Category A — Required Actions
- [x] Include copyright notices and LICENSE files in distributed builds
- [x] Maintain THIRD-PARTY-NOTICES.md or equivalent
- [x] For Apache 2.0: include NOTICE file if upstream NOTICE file exists
- [x] For Apache 2.0: document that the Apache patent grant applies

### Category B — Required Actions
- [x] All of the above for Category A
- [x] For LGPL: distribute library in a form allowing user relinking, or use dynamic linking
- [x] For MPL: ensure modified MPL files are available under MPL
- [x] Legal sign-off before static linking with LGPL components

### Category C — Required Actions
- [x] All of the above for Category B
- [ ] **STOP** — do not include in proprietary product without legal review
- [ ] Obtain commercial license exception from upstream
- [ ] Or restructure to use a permissively licensed alternative
- [ ] For AGPL: any SaaS deployment that allows users to interact with the software over a network
       triggers the copyleft obligation — even without binary distribution

### Category D — Required Actions
- [ ] **STOP** — do not include without explicit vendor authorization
- [ ] Remove from dependency tree or obtain commercial license

## 3. Patent Risk Assessment

### Apache 2.0 Patent Grant
Apache 2.0 includes an explicit patent license from contributors. However, the patent
retaliation clause (§ 3) terminates the patent grant if the licensee initiates patent
litigation against a contributor. Evaluate patent portfolio exposure before asserting patents
against Apache 2.0 contributor organizations.

### GPLv3 Patent Provisions
GPLv3 includes non-aggression and anti-tivoization provisions. Distribution under GPLv3 may
implicitly grant broader patent licenses than the copyright license alone.

### Unpatented Prior Art
Consider whether OSS components establish prior art that could affect your own pending patent
applications.

## 4. Checklist: Pre-Ship Review

- [ ] All dependencies inventoried (use SBOM tool: syft, CycloneDX)
- [ ] Each dependency classified (A / B / C / D)
- [ ] No Category C or D components in production build without legal sign-off
- [ ] THIRD-PARTY-NOTICES.md current and accurate
- [ ] NOTICE files from Apache 2.0 components bundled
- [ ] LGPL components linked dynamically or source provided for relinking
- [ ] Legal review completed and documented
- [ ] SBOM attached to release artifacts

## 5. Risk Flag Summary for RAG Extraction

When reviewing a codebase for license risks, flag the following patterns:

- **AGPL in SaaS product** — immediate block; triggers copyleft on network access
- **GPL in closed-source binary** — block unless commercial license obtained
- **SSPL in cloud service** — typically treated as non-OSS by most legal teams
- **No license file** — all rights reserved by default; block
- **Dual license (FOSS + commercial)** — confirm which license applies to your use case
- **License incompatibility** — GPL 2.0 and Apache 2.0 are incompatible when combined
- **Patent retaliation clauses** — Apache 2.0, GPLv3, MPL 2.0 all include them
