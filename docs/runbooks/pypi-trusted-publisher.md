# PyPI Trusted Publisher (OIDC) Setup

Runbook for configuring "trusted publishing" on both **PyPI** (production) and
**TestPyPI** (rehearsal) so the `pip-wheel.yml` CI workflow can publish without
a long-lived API token.

## Background

PyPI's [trusted-publisher](https://docs.pypi.org/trusted-publishers/) mechanism
issues a short-lived OIDC token to the GitHub Actions runner at publish time.
The workflow requests this token automatically when the job's permissions include
`id-token: write`; no `PYPI_API_TOKEN` secret is stored in the repository.

---

## 1. Create the project on PyPI (first time only)

If the `xiaoguai` project does **not** yet exist on PyPI:

1. Log in at <https://pypi.org>.
2. Go to **Your projects → New project** (or just publish once with a token to
   create the project namespace, then follow the steps below).

> Skip this section if the project already exists.

---

## 2. Add a Pending Publisher on PyPI (production)

1. Log in at <https://pypi.org>.
2. Navigate to the **`xiaoguai`** project page.
3. Click **Manage → Publishing**.
4. Scroll to **"Add a new pending publisher"** (or "Add a new publisher" if the
   project already exists).
5. Fill in the form:

   | Field | Value |
   |---|---|
   | **PyPI project name** | `xiaoguai` |
   | **Owner** | `xiaoguai-agent` |
   | **Repository name** | `xiaoguai` |
   | **Workflow name** | `pip-wheel.yml` |
   | **Environment name** | *(leave blank)* |

6. Click **Add**.

> **Optional approval gate**: if you want human approval before every PyPI
> push, create a GitHub environment named `production` (repo → Settings →
> Environments → New environment), enable "Required reviewers", and set
> **Environment name** to `production` in step 5 above.  Then add
> `environment: production` to the `publish` job in `pip-wheel.yml`.

---

## 3. Add a Pending Publisher on TestPyPI (rehearsal)

Repeat the same steps at <https://test.pypi.org>:

1. Log in at <https://test.pypi.org>.
2. Navigate to (or create) the **`xiaoguai`** project.
3. Click **Manage → Publishing → Add a new pending publisher**.
4. Fill in the form with the **same values** as above:

   | Field | Value |
   |---|---|
   | **PyPI project name** | `xiaoguai` |
   | **Owner** | `xiaoguai-agent` |
   | **Repository name** | `xiaoguai` |
   | **Workflow name** | `pip-wheel.yml` |
   | **Environment name** | *(leave blank)* |

5. Click **Add**.

---

## 4. How the workflow gates publishing

| Trigger | `publish-testpypi` job | `publish` job |
|---|---|---|
| `workflow_dispatch` (manual) | Runs | Does NOT run |
| Push to `v*` tag containing `a`, `b`, `rc`, or `.dev` | Runs | Does NOT run |
| Push to stable `v*` tag (e.g. `v1.2.0`) | Does NOT run | Runs |

Pre-release tag examples that go to **TestPyPI only**:
- `v1.2.0a1`, `v1.2.0b2`, `v1.2.0rc1`, `v1.2.0.dev3`

Stable tag examples that go to **PyPI**:
- `v1.1.7`, `v1.2.0`, `v2.0.0`

---

## 5. Verifying the setup

After adding the pending publisher, trigger a rehearsal:

```bash
# From the repo root, dispatch the workflow manually via CLI:
gh workflow run pip-wheel.yml --field version_override=0.0.0+oidctest
```

Watch the run at <https://github.com/xiaoguai-agent/xiaoguai/actions>.  The
`publish-testpypi` job should succeed with a line like:

```
Successfully uploaded xiaoguai-0.0.0+oidctest-cp312-cp312-manylinux_...whl
```

Then confirm the package appears at:
<https://test.pypi.org/project/xiaoguai/>

---

## 6. Rotating or removing legacy API tokens

Once OIDC publishing is confirmed working:

1. Go to PyPI → Account settings → API tokens.
2. Revoke any `xiaoguai`-scoped tokens that were used previously.
3. Remove any `PYPI_API_TOKEN` secret from the GitHub repository
   (repo → Settings → Secrets and variables → Actions).

There is no `password:` field in the workflow's publish steps, so the action
will error immediately if someone accidentally re-adds a secret without also
re-adding `password:`.  This is the correct fail-safe.

---

## 7. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `403 Invalid or non-existent authentication information` | Trusted publisher not configured on PyPI | Follow section 2 / 3 above |
| `id-token: write` permission error | Workflow job missing `permissions.id-token: write` | Already set in `pip-wheel.yml`; check YAML indentation |
| Token exchange fails for forks / PRs | OIDC not available for pull-request workflows from forks | Expected; publishing only runs on tag push to the origin repo |
| TestPyPI version already exists | TestPyPI does not allow re-uploading the same version | Bump the patch or add a `.devN` suffix and re-tag |
| Pending publisher not claimed | The first publish after adding a pending publisher "claims" it automatically | If the workflow is never triggered, the publisher stays pending indefinitely |
