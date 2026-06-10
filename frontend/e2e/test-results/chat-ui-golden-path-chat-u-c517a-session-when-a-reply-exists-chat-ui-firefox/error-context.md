# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: chat-ui/golden-path.spec.ts >> chat-ui golden path >> Branch from here (v1.1.2 fork) opens a forked session when a reply exists
- Location: tests/chat-ui/golden-path.spec.ts:110:3

# Error details

```
Error: browserType.launch: Executable doesn't exist at /Users/zw/Library/Caches/ms-playwright/firefox-1522/firefox/Nightly.app/Contents/MacOS/firefox
╔════════════════════════════════════════════════════════════╗
║ Looks like Playwright was just installed or updated.       ║
║ Please run the following command to download new browsers: ║
║                                                            ║
║     pnpm exec playwright install                           ║
║                                                            ║
║ <3 Playwright Team                                         ║
╚════════════════════════════════════════════════════════════╝
```