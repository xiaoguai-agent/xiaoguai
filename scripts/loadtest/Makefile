# scripts/loadtest/Makefile
#
# k6 load-test runner for Xiaoguai.
#
# Prerequisites:
#   k6 installed (https://k6.io/docs/get-started/installation/)
#
# Usage:
#   make smoke                  — CI-safe 30 s run at 5 VUs (chat-burst only)
#   make full                   — all scenarios at full VU count (long-running, opt-in)
#   make chat SESSION_ID=<uuid> — chat-burst scenario at full load
#   make webhook WEBHOOK_ROUTE_ID=<id> WEBHOOK_ROUTE_TOKEN=<tok>
#   make usage
#   make mixed SESSION_ID=<uuid>
#   make spike SESSION_ID=<uuid>
#
# Environment (override as needed):
#   BASE_URL  — API base URL (default: http://localhost:7600)
#   API_TOKEN — Bearer token (optional on dev stacks that skip auth)
#   SESSION_ID          — pre-created session UUID (chat / mixed / spike)
#   WEBHOOK_ROUTE_ID    — pre-created webhook route ID
#   WEBHOOK_ROUTE_TOKEN — token for the webhook route

BASE_URL            ?= http://localhost:7600
API_TOKEN           ?=
SESSION_ID          ?=
WEBHOOK_ROUTE_ID    ?=
WEBHOOK_ROUTE_TOKEN ?=

K6          := k6
K6_DIR      := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))k6
SCENARIOS   := $(K6_DIR)/scenarios

# Common env flags passed to every k6 invocation.
K6_ENV = -e BASE_URL=$(BASE_URL) \
         -e API_TOKEN=$(API_TOKEN) \
         -e SESSION_ID=$(SESSION_ID) \
         -e WEBHOOK_ROUTE_ID=$(WEBHOOK_ROUTE_ID) \
         -e WEBHOOK_ROUTE_TOKEN=$(WEBHOOK_ROUTE_TOKEN)

.PHONY: smoke full chat webhook usage mixed spike check-k6

# ---------------------------------------------------------------------------
# CI smoke — safe to run on every PR (30 s, 5 VUs)
# ---------------------------------------------------------------------------
smoke: check-k6
	$(K6) run $(K6_ENV) \
		--vus 5 \
		--duration 30s \
		$(SCENARIOS)/chat-burst.js

# ---------------------------------------------------------------------------
# Full suite — all scenarios at design load (long-running, manual opt-in)
# ---------------------------------------------------------------------------
full: check-k6 chat webhook usage mixed spike

# ---------------------------------------------------------------------------
# Individual scenarios at full design load
# ---------------------------------------------------------------------------
chat: check-k6
	$(K6) run $(K6_ENV) $(SCENARIOS)/chat-burst.js

webhook: check-k6
	$(K6) run $(K6_ENV) $(SCENARIOS)/webhook-storm.js

usage: check-k6
	$(K6) run $(K6_ENV) $(SCENARIOS)/usage-readheavy.js

mixed: check-k6
	$(K6) run $(K6_ENV) $(SCENARIOS)/mixed.js

spike: check-k6
	$(K6) run $(K6_ENV) $(SCENARIOS)/spike.js

# ---------------------------------------------------------------------------
# Prerequisite: verify k6 is on PATH
# ---------------------------------------------------------------------------
check-k6:
	@command -v $(K6) >/dev/null 2>&1 || { \
		echo "ERROR: k6 not found.  Install it from https://k6.io/docs/get-started/installation/"; \
		exit 1; \
	}
