# Agents & Skills

Inventory of the Claude Code subagents and skills available in this workspace.

- **Project** items live in this repo under `.claude/` and travel with the clone.
- **Global** items come from the user's `~/.claude/` config and are *not* part of
  this repo — they're listed for reference only.

_Last updated: 2026-06-08_

---

## Agents (subagent types)

Spawned via the Agent tool with `subagent_type`.

### Project (`.claude/agents/`)

| Agent | Tools | Purpose |
|-------|-------|---------|
| `xauusd-trader` | Bash, Read, Grep, Glob | Professional XAUUSD (gold) day trader. Produces an intraday plan — a laddered set of VWAP + 8 EMA setups (5–20 positions) with direction / trigger / entry / stop / targets / size, plus the news-driven day bias. Self-serves from live DuckDB tables (M5/M15/H1/D1 candles, today's EC calendar, recent news), computing VWAP/8EMA/ATR/session levels itself. Read-only — never places trades. |

### Global / built-in

| Agent | Tools | Purpose |
|-------|-------|---------|
| `claude` | `*` | Catch-all for any task that doesn't fit a more specific agent. |
| `general-purpose` | `*` | Researching complex questions, searching code, multi-step tasks. |
| `Explore` | All except Agent/Edit/Write/NotebookEdit/ExitPlanMode | Read-only broad fan-out search; locates code, doesn't review it. |
| `Plan` | All except Agent/Edit/Write/NotebookEdit/ExitPlanMode | Software architect — designs step-by-step implementation plans. |
| `claude-code-guide` | Glob, Grep, Read, WebFetch, WebSearch | Answers questions about Claude Code, the Agent SDK, and the Claude API. |
| `statusline-setup` | Read, Edit | Configures the Claude Code status line. |

---

## Skills

Invoked via the Skill tool or `/<name>`.

### Project (`.claude/skills/`)

These are the trading playbooks / system prompts that power this app's features.

| Skill | Purpose |
|-------|---------|
| `claude-blitz` | XAUUSD 1-MINUTE scalping playbook — triple-screen (M15 trend → M5 momentum → M1 entry); only scalps with the M15 trend, enters on an M1 pullback + confirmation, runs tight. System prompt for the "Claude Blitz" 1-min scalp card. Read-only. |
| `claude-volume` | XAUUSD volume-profile playbook — reasons from pre-computed per-day + weekly POC and Value-Area High/Low to produce ONE intraday setup keyed to acceptance/rejection, magnets, and low-volume breakouts. System prompt for the "Claude Volume" card. Read-only. |
| `gold-day-trader` | XAUUSD intraday day-trading playbook — a library of strategies (VWAP/8EMA, ORB, liquidity sweeps, S/R + PDH/PDL, trend pullbacks, ATR-extreme fades, post-news retests) under a regime-first selection process. The multi-strategy model in the Gold Trade Ideas feature. |
| `xrp-5m` | XRPUSD 5-MINUTE intraday playbook — crypto day-trade strategies under a regime-first process, with Bitcoin's tape as the master filter. System prompt for the "XRP 5m" bot card. Read-only. |

### Global / built-in

| Skill | Purpose |
|-------|---------|
| `deep-research` | Fan-out web research → fetch → adversarially verify → cited report. |
| `code-review` | Review the current diff for bugs and cleanups at a chosen effort level. |
| `simplify` | Apply reuse/simplification/efficiency cleanups to changed code (quality only). |
| `verify` | Run the app and observe behavior to confirm a change works. |
| `run` | Launch and drive this project's app to see a change working. |
| `review` | Review a pull request. |
| `security-review` | Security review of pending changes on the current branch. |
| `fewer-permission-prompts` | Scan transcripts and add a read-only Bash/MCP allowlist to settings. |
| `update-config` | Configure the Claude Code harness via settings.json (hooks, permissions, env). |
| `keybindings-help` | Customize keyboard shortcuts in `~/.claude/keybindings.json`. |
| `loop` | Run a prompt or slash command on a recurring interval. |
| `schedule` | Create/manage scheduled remote agents (cron routines). |
| `claude-api` | Reference for the Claude API / Anthropic SDK (models, pricing, params). |
| `init` | Initialize a new CLAUDE.md with codebase documentation. |
