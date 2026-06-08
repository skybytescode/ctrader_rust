---
name: xauusd-trade-idea
description: Get a structured XAUUSD trade idea (bias / entry / stop / target / rationale) from the professional gold trader subagent. Delegates to the xauusd-trader agent which pulls live price, multi-TF candles, today's EC events, and recent news bodies before forming a view.
---

# XAUUSD Trade Idea

Spawn the **xauusd-trader** subagent with a clear instruction:

> "Produce a structured XAUUSD trade idea for the current market state.
> Follow your workflow: self-serve from DuckDB — anchor current price from
> xauusd_m1, work HTF→LTF (D1→H1→M15→M5; there is no H4 table), compute the
> session VWAP + 8 EMA + ATR(14,H1) yourself from the candle rows, pull
> today's vol≥2 EC events and the next 24h from xauusd_economic_calendar,
> scan recent gold-relevant news in news_historical (read the 3-5 most
> relevant bodies for the actual narrative), then form a bias and output in
> your standard template. The candle/calendar/news tables are a shallow
> rolling window (weeks) — work with what's there; no deep history exists.
> If the data is thin / weekend-quiet / inside an event-window cooldown,
> recommend FLAT and say why."

Use the **Task tool** with `subagent_type: "xauusd-trader"`. Pass the
above as the prompt. Return the agent's reply verbatim — do not
summarize, do not editorialize, do not strip the structured format.

If the user has added context to the slash command (e.g. `/xauusd-trade-idea
focus on the H1 setup` or `/xauusd-trade-idea I'm already short from 4520,
should I add?`), include that context in the prompt to the subagent so it
can tailor the analysis.
