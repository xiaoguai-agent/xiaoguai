# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: admin-ui/admin-personas.spec.ts >> admin-ui Personas pane — CRUD against mocked /v1/personas >> list renders mocked personas after tenant id entered
- Location: tests/admin-ui/admin-personas.spec.ts:148:3

# Error details

```
Error: browserType.launch: Executable doesn't exist at /Users/zw/Library/Caches/ms-playwright/webkit-2287/pw_run.sh
╔════════════════════════════════════════════════════════════╗
║ Looks like Playwright was just installed or updated.       ║
║ Please run the following command to download new browsers: ║
║                                                            ║
║     pnpm exec playwright install                           ║
║                                                            ║
║ <3 Playwright Team                                         ║
╚════════════════════════════════════════════════════════════╝
```