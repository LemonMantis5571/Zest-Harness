# Provider quota

Zest keeps three different kinds of information separate:

1. **Local usage** — tokens and requests sent through Zest, stored in the local
   usage ledger.
2. **Rate limits** — short-window limits returned by the provider after a
   request. These can be stale while the app is idle.
3. **Account balance** — money or credits available in a provider account.

The top-bar quota panel only shows values from the provider. It never turns
local token counts into a remaining plan number.

## What is live today

- Anthropic response headers are saved when the native Messages API returns
  them.
- OpenAI-compatible providers read the standard `x-ratelimit-*` response
  headers for request and token windows. A proxy in front of one must forward
  those headers for Zest to see them.
- DeepSeek is checked on demand through its official `GET /user/balance`
  endpoint. The check runs only for `https://api.deepseek.com` and uses the
  existing key from the OS credential store or configured environment variable.
- Codex CLI (`kind = "codex_cli"`) is checked through the installed
  `codex app-server` for every configured CLI id. Zest sends
  `account/rateLimits/read`; the CLI owns the login and makes the
  authenticated request. If the CLI is not installed or signed in, the
  panel explains that instead.
- ChatGPT Codex (`kind = "codex_oauth"`) is checked with
  `GET https://chatgpt.com/backend-api/wham/usage` using that provider's
  stored session. A failed or unsigned-in check is an error or unavailable
  state, never a 0% guess or a local ledger total.
- Claude Desktop and Claude Code share the same Claude.ai usage limit. On
  Windows, Zest reads Claude Desktop's read-only local cache at
  `%APPDATA%\Claude\plan-usage-history.json` and shows its 5-hour and 7-day
  percentages. The adapter reads only the timestamp and usage fields; it never
  reads an access token. The cache does not include reset timestamps, so Zest
  shows when the sample was updated and marks it stale after 24 hours.

If Windows resolves `codex` to a Store app executable that refuses child
processes, point Zest at a standalone CLI before launching it:

```powershell
$env:ZEST_CODEX_COMMAND = 'C:\path\to\codex.exe'
npm run dev
```

OpenAI documents the standard rate-limit headers in its [API reference](https://platform.openai.com/docs/api-reference/backward-compatibility),
DeepSeek documents the balance response in [Get User Balance](https://api-docs.deepseek.com/api/get-user-balance/),
and Codex documents the local app-server method in its [account API](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#7-rate-limits-chatgpt).

## Claude Desktop and Claude Code

Claude's official docs confirm that Claude.ai, Claude Code, and Claude Desktop
use the same usage limit. Claude Code can also emit a real `rate_limit_event`
during a normal turn, and Zest keeps that event. The Desktop cache lets Zest
show the shared percentage before Zest has made a Claude turn, while the CLI
event remains the more current source when it is available.

Zest does not scrape the Desktop window, read OAuth credentials, or call the
private OAuth usage endpoint. If Desktop has not written a cache yet, or the
cache is stale, the panel says so instead of inventing a number. Reset times
are shown when Claude's event provides them; the Desktop cache currently stores
percentages only.

When a provider does not expose a supported check, the panel says why and the
local usage view remains available. If a provider later publishes an official
endpoint, add it as a provider-specific adapter with tests and a documented
permission scope.

Anthropic documents the shared product limit in [How do usage and length limits
work?](https://support.claude.com/en/articles/11647753-how-do-usage-and-length-limits-work),
the Desktop usage ring in its [error reference](https://code.claude.com/docs/en/errors),
and the CLI `rate_limits` fields in [Customize your status
line](https://code.claude.com/docs/en/statusline).

## Security rules

- Never put API keys in the quota response, logs, or UI.
- Never call a custom endpoint's account URL by guessing it.
- Never label local estimates as “remaining”, “balance”, or “quota”.
- A failed check is an error/unavailable state, not zero balance.
