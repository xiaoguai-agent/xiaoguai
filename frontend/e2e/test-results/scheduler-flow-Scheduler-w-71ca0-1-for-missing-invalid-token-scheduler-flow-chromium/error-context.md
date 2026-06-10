# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: scheduler-flow.spec.ts >> Scheduler webhook-route flow >> webhook endpoint returns 401 for missing/invalid token
- Location: tests/scheduler-flow.spec.ts:154:3

# Error details

```
Error: apiRequestContext.post: connect ECONNREFUSED ::1:7600
Call log:
  - → POST http://localhost:7600/v1/scheduler/webhooks/invalid-route-id
    - user-agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.7778.96 Safari/537.36
    - accept: */*
    - accept-encoding: gzip,deflate,br
    - content-type: application/json
    - content-length: 2

```