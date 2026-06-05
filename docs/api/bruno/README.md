# Xiaoguai Bruno API Collection — Wave 3

[Bruno](https://www.usebruno.com/) collection for the Xiaoguai wave-3 REST API.
Covers the three new endpoint groups added in v1.2.3–v1.2.28:
**HotL policies**, **Outcomes telemetry**, and **Skills marketplace**.

## Structure

```
bruno/
  bruno.json           # Collection manifest
  environments/
    local.bru          # http://localhost:7600 + dev-token
    staging.bru        # https://staging-api.example.com
  hotl/
    list-policies.bru  # GET  /v1/hotl/policies
    create-policy.bru  # POST /v1/hotl/policies
    get-policy.bru     # GET  /v1/hotl/policies/:id
    update-policy.bru  # PUT  /v1/hotl/policies/:id
    delete-policy.bru  # DELETE /v1/hotl/policies/:id
    check.bru          # POST /v1/hotl/check
  outcomes/
    record.bru         # POST /v1/outcomes
    list.bru           # GET  /v1/outcomes
    summary.bru        # GET  /v1/outcomes/summary
    timeseries.bru     # GET  /v1/outcomes/timeseries
  skills/
    list-installed.bru # GET  /v1/skills/installed
    install.bru        # POST /v1/skills/install
```

## Importing into Bruno

1. Open Bruno and click **Open Collection**.
2. Navigate to `docs/api/bruno/` and select the folder.
3. Bruno will detect `bruno.json` and load all requests.

## Environment Setup

Select an environment from the top-right dropdown:

| Environment | Base URL                          | Notes                          |
|-------------|-----------------------------------|--------------------------------|
| `local`     | `http://localhost:7600`           | Uses `dev-token` (no real auth)|
| `staging`   | `https://staging-api.example.com` | Replace `token` with real JWT  |

To customise, edit the relevant `.bru` file under `environments/` or override
variables in Bruno's environment editor (changes stay local, not committed).

## Authentication

Every `/v1/**` endpoint except `GET /v1/skills/catalog` requires a Bearer
token. All requests in this collection use `auth: bearer` with `{{token}}`
resolved from the active environment.

The `local` environment sets `token: dev-token`. In a local dev server started
without `--auth`, any non-empty string is accepted.

For staging/production, obtain a tenant bearer token from your Xiaoguai admin
and set it in the `staging` environment's `token` variable.

## Key Variables

| Variable         | Default (local)                          | Purpose                        |
|------------------|------------------------------------------|--------------------------------|
| `baseUrl`        | `http://localhost:7600`                  | API root                       |
| `token`          | `dev-token`                              | Bearer token                   |
| `tenantId`       | `11111111-1111-1111-1111-111111111111`   | UUID used in HotL requests     |
| `policyId`       | `aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa`   | Pre-existing policy for GET/PUT/DELETE |
| `installedPackId`| `cccccccc-cccc-cccc-cccc-cccccccccccc`   | Pre-existing install row       |

## Running via CLI

```bash
# Install Bruno CLI
npm install -g @usebruno/cli

# Run all requests against the local environment
cd docs/api/bruno
bru run --env local

# Run only the HotL folder
bru run hotl/ --env local

# Run a single request
bru run hotl/create-policy.bru --env local
```

## Spec Reference

The canonical OpenAPI 3.1 spec lives on branch `docs/openapi-wave3` at
`docs/api/openapi.yaml`.
