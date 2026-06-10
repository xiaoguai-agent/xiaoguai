# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/chat-hotl-suspend-resume.spec.ts >> chat-ui HotL suspend/resume e2e (sprint-12 S12-10 — §4.3.2) >> approve_via_chat_dispatches_tool: SSE allow + tool_call_finished renders the result
- Location: tests/chat-ui/chat-hotl-suspend-resume.spec.ts:284:3

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