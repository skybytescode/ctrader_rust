import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import './App.css'

type Tick = { type: 'tick'; symbol: string; bid: number; ask: number }
type StatusMsg = { type: 'status'; value: string }
type EcStatusMsg = { type: 'ec_status'; value: string }
type EcEvent = {
  ts: string
  currency: string
  volatility: number
  name: string
  actual: number | null
  forecast: number | null
  previous: number | null
  surprise: number | null
}
type EcTodayMsg = { type: 'ec_today'; events: EcEvent[] }
type MfbEvent = {
  ts: string
  currency: string
  volatility: number
  name: string
  country: string
  actual: number | null
  forecast: number | null
  previous: number | null
}
type MfbStatusMsg = { type: 'mfb_status'; value: string }
type MfbTodayMsg = { type: 'mfb_today'; events: MfbEvent[] }
type MfbNewsItem = {
  article_id: string
  category: string
  title: string
  url: string
  summary: string
  source: string
  published_utc: string
}
type MfbNewsStatusMsg = { type: 'mfb_news_status'; value: string }
type MfbNewsTodayMsg = { type: 'mfb_news_today'; items: MfbNewsItem[] }
type FfCalEvent = {
  ts: string
  currency: string
  volatility: number
  name: string
  country: string
  actual: string | null
  forecast: string | null
  previous: string | null
}
type FfNewsItem = {
  article_id: string
  title: string
  url: string
  source: string
  preview: string
  published_utc: string
}
type FfCalStatusMsg = { type: 'ff_cal_status'; value: string }
type FfCalTodayMsg = { type: 'ff_cal_today'; events: FfCalEvent[] }
type FfNewsStatusMsg = { type: 'ff_news_status'; value: string }
type FfNewsTodayMsg = { type: 'ff_news_today'; items: FfNewsItem[] }
type NewsStatusMsg = { type: 'news_status'; value: string }
type NewsArticle = {
  article_id: string
  title: string
  published_utc: string
  summary: string
  url: string
  author: string
  tags: string
  hour_utc: number
  weekday: number
  body: string | null
}
type NewsTodayMsg = { type: 'news_today'; articles: NewsArticle[] }

type OpenPosition = {
  id: number
  symbol: string
  side: 'BUY' | 'SELL' | string
  oz: number
  entry: number | null
  sl: number | null
  tp: number | null
}
type PendingOrder = {
  id: number
  symbol: string
  side: 'BUY' | 'SELL' | string
  type: string
  oz: number
  price: number | null
  sl: number | null
  tp: number | null
}
type OrderReview = {
  ok: boolean
  recommendation: string | null
  confidence: string | null
  reason: string | null
  error: string | null
  duration_ms: number
}
type PositionReview = {
  ok: boolean
  action: string | null
  new_sl: number | null
  new_tp: number | null
  confidence: string | null
  reason: string | null
  error: string | null
  duration_ms: number
}
type PositionsMsg = { type: 'positions'; positions: OpenPosition[]; orders: PendingOrder[] }
type TradeNotice = {
  reason: string
  symbol: string
  side: string
  oz: number
  close: number
  pnl: number
  position_id: number
}
type TradeEventMsg = { type: 'trade_event'; notice: TradeNotice }

type AutoState = { enabled: boolean; oz: number; status: string }
type AutoStatusMsg = { type: 'auto_status'; auto: AutoState }

type Msg = Tick | StatusMsg | EcStatusMsg | EcTodayMsg | NewsStatusMsg | NewsTodayMsg | MfbStatusMsg | MfbTodayMsg | MfbNewsStatusMsg | MfbNewsTodayMsg | FfCalStatusMsg | FfCalTodayMsg | FfNewsStatusMsg | FfNewsTodayMsg
  | PositionsMsg | TradeEventMsg | AutoStatusMsg

type ConnState = 'connecting' | 'connected' | 'disconnected' | 'error'
type Tab = 'dashboard' | 'calendar' | 'news' | 'archive' | 'trade-ideas' | 'positions' | 'automate' | 'forexfactory' | 'market-predictor'

// ── Archives tab types (ported from origin/web_gold) ──────────────────────────
type NewsUpdateResult = {
  last_archive_day: string | null
  cutoff: string
  articles_fetched: number
  bodies_fetched: number
  bodies_empty: number
  bodies_failed: number
  rate_limited: boolean
  days_written: [string, number][]
  message: string | null
}

type EcGoldUpdateResult = {
  table_existed: boolean
  cursor_before: string | null
  walk_start: string
  walk_end: string
  chunks_processed: number
  rows_added_this_call: number
  total_rows: number
  oldest_in_db: string | null
  newest_in_db: string | null
  duration_ms: number
  error: string | null
}

type EcGoldProgress = {
  chunks_done: number
  chunks_total: number
  rows_added_so_far: number
  current_chunk_start: string
  current_chunk_end: string
  elapsed_secs: number
}

type EcGoldStorageResult = {
  incremental: boolean
  disk_latest_day_before: string | null
  db_latest_day: string | null
  days_already_current: number
  files_written: number
  events_written: number
  up_to_date: boolean
  archive_root: string
  duration_ms: number
  error: string | null
}

type EcGoldStorageProgress = {
  files_done: number
  files_total: number
  events_written_so_far: number
  current_day: string
  elapsed_secs: number
}

type TfStats = {
  timeframe: string
  table: string
  rows: number
  oldest: string | null
  newest: string | null
  coverage_days: number | null
  tail_age_secs: number | null
  bar_secs: number
}

type XauusdStatsResult = {
  timeframes: TfStats[]
  market_state: 'live' | 'weekend-closed' | string
  queried_at_utc: string
}

type ArchiveImportResult = {
  ec_files: number
  ec_rows: number
  news_files: number
  news_rows: number
  news_bodies: number
  message: string | null
}

type BackfillState = {
  status: 'idle' | 'running' | 'complete' | 'error' | string
  started_at_utc: string | null
  completed_at_utc: string | null
  current_tf: string | null
  current_mode: string | null
  chunks_this_tf: number
  bars_this_tf: number
  total_bars: number
  last_chunk_oldest_utc: string | null
  tfs_completed: string[]
  tfs_skipped: string[]
  tfs_total: number
  last_error: string | null
}
// ── End Archives tab types ────────────────────────────────────────────────────

// One model's single intraday setup, as returned by get_gold_trade_ideas_multi
// and streamed per-model over the `trade_idea_model` event.
type ModelTradeIdea = {
  provider: string
  model: string
  ok: boolean
  bias: 'LONG' | 'SHORT' | 'FLAT' | string | null
  strategy: string | null
  entry_low: number | null
  entry_high: number | null
  stop: number | null
  target1: number | null
  target2: number | null
  rationale: string | null
  error: string | null
  duration_ms: number
}

// The models we fan out to, in display order. Popups are keyed by provider.
const TRADER_PROVIDERS = ['Claude', 'Gemini', 'Claude Blitz'] as const
type TraderProvider = typeof TRADER_PROVIDERS[number]

// Result of a live market-order placement (place_gold_order).
type OrderResult = {
  sent: boolean
  side: string
  symbol: string
  oz: number
  ctrader_volume: number
  order_type: string
  entry: number | null
  sl: number | null
  tp: number | null
  status: string | null
  error: string | null
}

// Render a price with its final digit shown smaller (the "pipette").
function PipPrice({ value, decimals }: { value: number; decimals: number }) {
  const s = value.toFixed(decimals)
  return <>{s.slice(0, -1)}<span className="pip">{s.slice(-1)}</span></>
}

// One instrument row in the sidebar: symbol, today's change, and live bid / ask /
// spread. `decimals` controls price precision per symbol (XAUUSD → 2, EURUSD → 5).
// `pipSize` (e.g. 0.0001 for EURUSD), when set, expresses the spread and change in
// pips instead of raw price points. `dailyOpen` (today's D1 open) anchors the
// daily change, with `lastClose` (the D1 close) as the price fallback when no live
// tick is streaming (e.g. weekends). The bid is colored by tick direction.
function InstrumentTile({ symbol, tick, prevBid, decimals, pipSize, dailyOpen, lastClose, selected, onSelect }: {
  symbol: string
  tick: Tick | null
  prevBid: number | null
  decimals: number
  pipSize?: number
  dailyOpen?: number
  lastClose?: number
  selected: boolean
  onSelect?: () => void
}) {
  const spread = tick ? tick.ask - tick.bid : null
  const spreadText = spread == null
    ? '—'
    : pipSize
      ? `${(spread / pipSize).toFixed(1)} pips`
      : spread.toFixed(decimals)
  const dir = tick && prevBid != null
    ? (tick.bid > prevBid ? 'up' : tick.bid < prevBid ? 'down' : 'flat')
    : 'flat'

  // Daily change vs today's open: pips for FX (pipSize set), else raw points,
  // plus percent. Uses the live bid, falling back to the D1 close when the market
  // is closed. Colored green (up) / red (down).
  const price = tick ? tick.bid : lastClose ?? null
  let changeText: string | null = null
  let changeDir = 'flat'
  if (price != null && dailyOpen) {
    const change = price - dailyOpen
    const main = pipSize ? (change / pipSize).toFixed(1) : change.toFixed(decimals)
    const signed = change > 0 ? `+${main}` : main
    const pct = (change / dailyOpen) * 100
    const pctStr = `${pct > 0 ? '+' : ''}${pct.toFixed(2)}%`
    changeText = `${signed} (${pctStr})`
    changeDir = change > 0 ? 'up' : change < 0 ? 'down' : 'flat'
  }

  return (
    <li
      className={`instrument ${selected ? 'selected' : 'active'}`}
      onClick={onSelect}
    >
      <div className="sym">{symbol}</div>
      {changeText && <div className={`change ${changeDir}`}>{changeText}</div>}
      <div className={`row dir-${dir}`}>
        <span className="label">Bid</span>
        <span className="val">{tick ? <PipPrice value={tick.bid} decimals={decimals} /> : '—'}</span>
      </div>
      <div className="row">
        <span className="label">Ask</span>
        <span className="val">{tick ? <PipPrice value={tick.ask} decimals={decimals} /> : '—'}</span>
      </div>
      <div className="row spread">
        <span className="label">Spread</span>
        <span className="val">{spreadText}</span>
      </div>
    </li>
  )
}

// Per-instrument Calendar / News relevance. Calendar events are filtered to the
// pair's currencies; news is filtered by a keyword regex over tags + title. A
// symbol absent from these maps (e.g. XAUUSD) shows the unfiltered global feed.
const SYMBOL_CCYS: Record<string, string[]> = {
  EURUSD: ['EUR', 'USD'],
}
const SYMBOL_NEWS_RE: Record<string, RegExp> = {
  EURUSD: /\beur\b|\beuro\b|\becb\b|lagarde|eurozone|euro area|\busd\b|us dollar|greenback|\bfed\b|fomc|powell|federal reserve|nonfarm|payrolls/i,
}

// XRP 5-minute bot dashboard: live quote + start/close controls. The bot itself
// is not wired up yet — the buttons toggle local UI state only.
function XrpBotView({ tick }: { tick: Tick | null }) {
  const [running, setRunning] = useState(false)
  const [idea, setIdea] = useState<ModelTradeIdea | null>(null)
  const [loading, setLoading] = useState(false)
  const spread = tick ? tick.ask - tick.bid : null

  const getSetup = async () => {
    setLoading(true)
    try {
      setIdea(await invoke<ModelTradeIdea>('get_xrp_trade_idea'))
    } catch (e) {
      setIdea({
        provider: 'XRP 5m', model: '', ok: false, bias: null, strategy: null,
        entry_low: null, entry_high: null, stop: null, target1: null, target2: null,
        rationale: null, error: String(e), duration_ms: 0,
      })
    } finally {
      setLoading(false)
    }
  }

  const fmt = (n: number | null) => (n == null ? '—' : n.toFixed(4))
  const biasClass = idea?.bias === 'LONG' ? 'long' : idea?.bias === 'SHORT' ? 'short' : 'flat'

  return (
    <div className="dashboard">
      <h2>XRPUSD — 5-minute Bot</h2>
      <p className="muted">Live price + the 5m agent's read. The bot loop (auto-execution, TP/SL management, 12% daily-loss stop) is not wired up yet — Start/Close toggle panel state only; "Get 5m Setup" is a read-only preview of what the agent proposes.</p>

      <div className="xrp-quote">
        <div><span className="label">Bid</span><strong>{tick ? tick.bid.toFixed(4) : '—'}</strong></div>
        <div><span className="label">Ask</span><strong>{tick ? tick.ask.toFixed(4) : '—'}</strong></div>
        <div><span className="label">Spread</span><strong>{spread != null ? spread.toFixed(4) : '—'}</strong></div>
      </div>

      <div className="bot-controls">
        <button className="bot-btn bot-start" disabled={running} onClick={() => setRunning(true)}>Start Bot</button>
        <button className="bot-btn bot-close" disabled={!running} onClick={() => setRunning(false)}>Close Bot</button>
        <button className="bot-btn" disabled={loading} onClick={getSetup}>{loading ? 'Asking agent…' : 'Get 5m Setup'}</button>
        <span className={`bot-status ${running ? 'on' : 'off'}`}>
          {running ? 'Running (UI only)' : 'Stopped'}
        </span>
      </div>

      {idea && (
        <div className="xrp-idea">
          {idea.ok ? (
            <>
              <div className="xrp-idea-head">
                <span className={`idea-bias ${biasClass}`}>{idea.bias}</span>
                <span className="idea-strategy">{idea.strategy}</span>
                {idea.duration_ms > 0 && <span className="muted small">{(idea.duration_ms / 1000).toFixed(1)}s</span>}
              </div>
              {idea.bias !== 'FLAT' && (
                <div className="xrp-idea-levels">
                  <span>entry <strong>{fmt(idea.entry_low)}–{fmt(idea.entry_high)}</strong></span>
                  <span>stop <strong>{fmt(idea.stop)}</strong></span>
                  <span>tp1 <strong>{fmt(idea.target1)}</strong></span>
                  <span>tp2 <strong>{fmt(idea.target2)}</strong></span>
                </div>
              )}
              {idea.rationale && <p className="xrp-idea-rationale">{idea.rationale}</p>}
            </>
          ) : (
            <p className="xrp-idea-error">error: {idea.error}</p>
          )}
        </div>
      )}
    </div>
  )
}

// Placeholder panel for features that are scaffolded but not yet implemented.
function ComingSoon({ title, note }: { title: string; note?: string }) {
  return (
    <div className="dashboard">
      <h2>{title}</h2>
      <p className="muted">{note ?? 'Coming next.'}</p>
    </div>
  )
}

function App() {
  // Live ticks keyed by symbol (XAUUSD, EURUSD, …); prevBids holds the previous
  // bid per symbol so each tile shows its own up/down direction.
  const [ticks, setTicks] = useState<Record<string, Tick>>({})
  const [prevBids, setPrevBids] = useState<Record<string, number>>({})
  // Today's D1 bar (open + close) per sidebar symbol. `open` anchors the daily
  // change; `close` is the price fallback when no live tick streams (weekends).
  const [dailyBars, setDailyBars] = useState<Record<string, { open: number; close: number }>>({})
  const [conn, setConn] = useState<ConnState>('connecting')
  const [serverStatus, setServerStatus] = useState('')
  const [ecStatus, setEcStatus] = useState('')
  const [ecEvents, setEcEvents] = useState<EcEvent[]>([])
  const [mfbStatus, setMfbStatus] = useState('')
  const [mfbEvents, setMfbEvents] = useState<MfbEvent[]>([])
  const [mfbNewsStatus, setMfbNewsStatus] = useState('')
  const [mfbNewsItems, setMfbNewsItems] = useState<MfbNewsItem[]>([])
  const [ffCalStatus, setFfCalStatus] = useState('')
  const [ffCalEvents, setFfCalEvents] = useState<FfCalEvent[]>([])
  const [ffNewsStatus, setFfNewsStatus] = useState('')
  const [ffNewsItems, setFfNewsItems] = useState<FfNewsItem[]>([])
  const [newsStatus, setNewsStatus] = useState('')
  const [newsArticles, setNewsArticles] = useState<NewsArticle[]>([])
  const [openArticle, setOpenArticle] = useState<NewsArticle | null>(null)
  const [openFfNews, setOpenFfNews] = useState<FfNewsItem | null>(null)
  const [openMfbNews, setOpenMfbNews] = useState<MfbNewsItem | null>(null)
  const [tab, setTab] = useState<Tab>('dashboard')
  const [selectedSymbol, setSelectedSymbol] = useState<string | null>(null)
  const [positions, setPositions] = useState<OpenPosition[]>([])
  const [orders, setOrders] = useState<PendingOrder[]>([])
  const [auto, setAuto] = useState<AutoState>({ enabled: false, oz: 1, status: 'off' })
  const [tradeBanner, setTradeBanner] = useState<TradeNotice | null>(null)
  const reconnectRef = useRef<number | null>(null)
  const bannerTimerRef = useRef<number | null>(null)

  useEffect(() => {
    let ws: WebSocket | null = null
    let cancelled = false

    const connect = () => {
      if (cancelled) return
      setConn('connecting')
      ws = new WebSocket('ws://127.0.0.1:6001')
      ws.onopen = () => setConn('connected')
      ws.onclose = () => {
        setConn('disconnected')
        reconnectRef.current = window.setTimeout(connect, 1500)
      }
      ws.onerror = () => setConn('error')
      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data) as Msg
          switch (msg.type) {
            case 'tick':
              setTicks((cur) => {
                const prev = cur[msg.symbol]
                if (prev) setPrevBids((pb) => ({ ...pb, [msg.symbol]: prev.bid }))
                return { ...cur, [msg.symbol]: msg }
              })
              break
            case 'status':
              setServerStatus(msg.value)
              break
            case 'ec_status':
              setEcStatus(msg.value)
              break
            case 'ec_today':
              setEcEvents(msg.events)
              break
            case 'mfb_status':
              setMfbStatus(msg.value)
              break
            case 'mfb_today':
              setMfbEvents(msg.events)
              break
            case 'news_status':
              setNewsStatus(msg.value)
              break
            case 'news_today':
              setNewsArticles(msg.articles)
              break
            case 'mfb_news_status':
              setMfbNewsStatus(msg.value)
              break
            case 'mfb_news_today':
              setMfbNewsItems(msg.items)
              break
            case 'ff_cal_status':
              setFfCalStatus(msg.value)
              break
            case 'ff_cal_today':
              setFfCalEvents(msg.events)
              break
            case 'ff_news_status':
              setFfNewsStatus(msg.value)
              break
            case 'ff_news_today':
              setFfNewsItems(msg.items)
              break
            case 'positions':
              setPositions(msg.positions ?? [])
              setOrders(msg.orders ?? [])
              break
            case 'trade_event':
              setTradeBanner(msg.notice)
              if (bannerTimerRef.current) window.clearTimeout(bannerTimerRef.current)
              bannerTimerRef.current = window.setTimeout(() => setTradeBanner(null), 30000)
              break
            case 'auto_status':
              setAuto(msg.auto)
              break
          }
        } catch {
          // ignore malformed
        }
      }
    }
    connect()

    return () => {
      cancelled = true
      if (reconnectRef.current) window.clearTimeout(reconnectRef.current)
      ws?.close()
    }
  }, [])

  // Poll today's D1 open for the sidebar symbols (anchors the daily change).
  // Refreshed every 60s so it recovers after startup/market open and rolls over
  // to the new day automatically.
  useEffect(() => {
    let cancelled = false
    const load = async () => {
      for (const sym of ['XAUUSD', 'EURUSD', 'XRPUSD', 'US30']) {
        try {
          const candles = await invoke<Candle[]>('get_trendbars', { symbol: sym, timeframe: 'D1', count: 1 })
          const bar = candles.at(-1)
          if (!cancelled && bar && bar.open > 0) {
            setDailyBars((cur) => {
              const prev = cur[sym]
              if (prev && prev.open === bar.open && prev.close === bar.close) return cur
              return { ...cur, [sym]: { open: bar.open, close: bar.close } }
            })
          }
        } catch {
          // symbol not yet subscribed / market closed — retry next cycle
        }
      }
    }
    load()
    const id = window.setInterval(load, 60_000)
    return () => { cancelled = true; window.clearInterval(id) }
  }, [])

  // XAUUSD is the primary instrument that drives the chart / positions / P-L.
  const tick = ticks['XAUUSD'] ?? null
  const eurTick = ticks['EURUSD'] ?? null
  // The tick for whichever instrument is currently selected (drives its chart).
  const selectedTick = selectedSymbol ? (ticks[selectedSymbol] ?? null) : null
  const isXrp = selectedSymbol === 'XRPUSD'

  // Calendar + News scoped to the selected instrument (EURUSD → EUR/USD only;
  // XAUUSD and the unselected state → the full global feed).
  const ccyFilter = selectedSymbol ? SYMBOL_CCYS[selectedSymbol] : undefined
  const newsRe = selectedSymbol ? SYMBOL_NEWS_RE[selectedSymbol] : undefined
  const shownEc = ccyFilter ? ecEvents.filter((e) => ccyFilter.includes(e.currency)) : ecEvents
  const shownMfb = ccyFilter ? mfbEvents.filter((e) => ccyFilter.includes(e.currency)) : mfbEvents
  const shownNews = newsRe ? newsArticles.filter((a) => newsRe.test(`${a.tags} ${a.title}`)) : newsArticles

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">cT</span>
          <span className="brand-name">Rust Terminal</span>
        </div>

        <h2 className="section">Trading Bots</h2>
        <ul className="instruments">
          <InstrumentTile
            symbol="XAUUSD"
            tick={tick}
            prevBid={prevBids['XAUUSD'] ?? null}
            decimals={2}
            dailyOpen={dailyBars['XAUUSD']?.open}
            lastClose={dailyBars['XAUUSD']?.close}
            selected={selectedSymbol === 'XAUUSD'}
            onSelect={() => { setSelectedSymbol('XAUUSD'); setTab('dashboard'); }}
          />
          <InstrumentTile
            symbol="EURUSD"
            tick={eurTick}
            prevBid={prevBids['EURUSD'] ?? null}
            decimals={5}
            pipSize={0.0001}
            dailyOpen={dailyBars['EURUSD']?.open}
            lastClose={dailyBars['EURUSD']?.close}
            selected={selectedSymbol === 'EURUSD'}
            onSelect={() => { setSelectedSymbol('EURUSD'); setTab('dashboard'); }}
          />
          <InstrumentTile
            symbol="XRPUSD"
            tick={ticks['XRPUSD'] ?? null}
            prevBid={prevBids['XRPUSD'] ?? null}
            decimals={4}
            dailyOpen={dailyBars['XRPUSD']?.open}
            lastClose={dailyBars['XRPUSD']?.close}
            selected={selectedSymbol === 'XRPUSD'}
            onSelect={() => { setSelectedSymbol('XRPUSD'); setTab('dashboard'); }}
          />
          <InstrumentTile
            symbol="US30"
            tick={ticks['US30'] ?? null}
            prevBid={prevBids['US30'] ?? null}
            decimals={1}
            dailyOpen={dailyBars['US30']?.open}
            lastClose={dailyBars['US30']?.close}
            selected={selectedSymbol === 'US30'}
            onSelect={() => { setSelectedSymbol('US30'); setTab('dashboard'); }}
          />
        </ul>

        <footer className="footer">
          <div className={`conn conn-${conn}`}>
            <span className="dot" />
            <span>{conn}</span>
          </div>
          {serverStatus && <div className="srv">{serverStatus}</div>}
        </footer>
      </aside>

      <main className="content">
        <nav className="tabs">
          <button className={tab === 'dashboard' ? 'tab active' : 'tab'} onClick={() => setTab('dashboard')}>Dashboard</button>
          {/* XRP is a self-contained bot view — just a Dashboard, no calendar/news/ideas. */}
          {!isXrp && (
            <>
              <button className={tab === 'calendar' ? 'tab active' : 'tab'} onClick={() => setTab('calendar')}>
                Calendar
                {shownEc.length > 0 && <span className="badge">{shownEc.length}</span>}
              </button>
              <button className={tab === 'news' ? 'tab active' : 'tab'} onClick={() => setTab('news')}>
                News
                {shownNews.length > 0 && <span className="badge">{shownNews.length}</span>}
              </button>
              <button className={tab === 'archive' ? 'tab active' : 'tab'} onClick={() => setTab('archive')}>
                Archives
              </button>
              <button className={tab === 'forexfactory' ? 'tab active' : 'tab'} onClick={() => setTab('forexfactory')}>
                Forex Factory
              </button>
              <button className={tab === 'market-predictor' ? 'tab active' : 'tab'} onClick={() => setTab('market-predictor')}>
                Market Predictor
              </button>
              <button className={tab === 'trade-ideas' ? 'tab active' : 'tab'} onClick={() => setTab('trade-ideas')}>
                Trade Ideas
              </button>
              {selectedSymbol === 'EURUSD' ? (
                <button className={tab === 'automate' ? 'tab active' : 'tab'} onClick={() => setTab('automate')}>
                  Automate_Trading
                </button>
              ) : (
                <button className={tab === 'positions' ? 'tab active' : 'tab'} onClick={() => setTab('positions')}>
                  Positions
                  {(positions.length + orders.length) > 0 && <span className="badge">{positions.length + orders.length}</span>}
                </button>
              )}
            </>
          )}
        </nav>

        {tradeBanner && (
          <div className={`trade-banner ${tradeBanner.pnl >= 0 ? 'ok' : 'loss'}`}>
            <span><strong>{tradeBanner.reason}</strong> · {tradeBanner.side} {tradeBanner.oz} oz {tradeBanner.symbol} @ {tradeBanner.close.toFixed(2)} · P/L <strong>{tradeBanner.pnl >= 0 ? '+' : ''}{tradeBanner.pnl.toFixed(2)} USD</strong></span>
            <button className="trade-banner-x" onClick={() => setTradeBanner(null)} aria-label="Dismiss">×</button>
          </div>
        )}

        <section className="panel">
          {tab === 'dashboard' && (
            isXrp
              ? <XrpBotView tick={selectedTick} />
              : selectedSymbol === 'US30'
                ? <US30View />
                : (selectedSymbol === 'XAUUSD' || selectedSymbol === 'EURUSD')
                  ? <ChartView symbol={selectedSymbol} tick={selectedTick} />
                  : <DashboardView tick={tick} />
          )}
          {tab === 'calendar' && (
            <div className="calendar-split">
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">FXStreet (live)</h4>
                <CalendarView events={shownEc} status={ecStatus} />
              </div>
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">MyFXBook (today)</h4>
                <MyFXBookView events={shownMfb} status={mfbStatus} />
              </div>
            </div>
          )}
          {tab === 'news' && (
            <div className="calendar-split">
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">FXStreet (live)</h4>
                <NewsView articles={shownNews} status={newsStatus} onOpen={setOpenArticle} />
              </div>
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">MyFXBook</h4>
                <MyFXBookNewsView items={mfbNewsItems} status={mfbNewsStatus} onOpen={setOpenMfbNews} />
              </div>
            </div>
          )}
          {tab === 'archive' && <ArchiveView />}
          {tab === 'forexfactory' && (
            <div className="calendar-split">
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">Forex Factory · News</h4>
                <FFNewsView items={ffNewsItems} status={ffNewsStatus} onOpen={setOpenFfNews} />
              </div>
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">Forex Factory · Calendar</h4>
                <FFCalendarView events={ffCalEvents} status={ffCalStatus} />
              </div>
            </div>
          )}
          {tab === 'trade-ideas' && (
            selectedSymbol === 'EURUSD'
              ? <ComingSoon title="EURUSD Trade Ideas" note="Multi-model EURUSD trade ideas are the next step." />
              : <TradeIdeasView />
          )}
          {tab === 'market-predictor' && (
            <div className="calendar-split">
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">Daily Analysis</h4>
                <DailyAnalysisView />
              </div>
              <div className="calendar-pane">
                <h4 className="calendar-pane-title">Current Week &amp; Next Week</h4>
                <WeeklyAnalysisView />
              </div>
            </div>
          )}
          {tab === 'positions' && <PositionsView positions={positions} orders={orders} tick={tick} auto={auto} />}
          {tab === 'automate' && (
            <ComingSoon title="Automate Trading — EURUSD" note="The 24/5 EURUSD auto-trading controls will live here." />
          )}
        </section>

        {openArticle && (
          <ArticleModal article={openArticle} onClose={() => setOpenArticle(null)} />
        )}
        {openFfNews && (
          <FFNewsModal item={openFfNews} onClose={() => setOpenFfNews(null)} />
        )}
        {openMfbNews && (
          <MyFXBookNewsModal item={openMfbNews} onClose={() => setOpenMfbNews(null)} />
        )}
      </main>
    </div>
  )
}

type Candle = { time: number; open: number; high: number; low: number; close: number; volume: number }

// Only the 9 TFs the broker (IC Markets) actually exposes via the
// cTrader Open API. M2/M4/M10/M30/H4 dropped — they were always empty.
type Timeframe = 'M1'|'M3'|'M5'|'M15'|'H1'|'H12'|'D1'|'W1'|'MN1'

const TIMEFRAMES: Timeframe[] = ['M1','M3','M5','M15','H1','H12','D1','W1','MN1']

const SECONDS_PER_BAR: Record<Timeframe, number> = {
  M1: 60, M3: 180, M5: 300, M15: 900,
  H1: 3600, H12: 43200, D1: 86400, W1: 604800, MN1: 2592000,
}

// How many bars to request per timeframe — keep wider TFs to fewer bars so the
// from_timestamp stays within cTrader's per-period range limit.
const BARS_PER_TF: Record<Timeframe, number> = {
  M1: 1000, M3: 1000, M5: 1000, M15: 1000,
  H1: 1000, H12: 500, D1: 500, W1: 300, MN1: 200,
}

// Indicators (VWAP + 8 EMA) only render on intraday TFs where the
// daily-anchored VWAP makes sense. On H1+ the indicators are hidden.
const INDICATOR_TFS: ReadonlyArray<Timeframe> = ['M1', 'M3', 'M5', 'M15']
const isIndicatorTf = (tf: Timeframe) => INDICATOR_TFS.includes(tf)
const EMA_PERIOD = 8

// VWAP session anchor in seconds-of-day (UTC). The "session day" for spot
// gold resets at 21:00 UTC (≈ 17:00 NY EDT) — matches TradingView's default
// VWAP behavior for XAUUSD and aligns with CME Globex's daily close/reopen.
// During EST (winter) the true reset is 22:00 UTC; we hold at 21:00 year-round
// for simplicity, accepting a ~1h offset in the winter half of the year.
const VWAP_SESSION_OFFSET_SEC = 21 * 3600

// "Session day" index for a unix-second timestamp under the 21:00 UTC anchor.
// A bar at 20:59 UTC belongs to the *previous* session day; a bar at 21:00
// UTC starts a new session.
const sessionDay = (unixSec: number) =>
  Math.floor((unixSec - VWAP_SESSION_OFFSET_SEC) / 86400)

// Unix-second timestamp at which a given session day begins (its 21:00 UTC
// anchor). Used by the live-tick path to walk same-day candles for VWAP.
const sessionDayStart = (day: number) =>
  day * 86400 + VWAP_SESSION_OFFSET_SEC

// Compute session-anchored VWAP (resets at 21:00 UTC) and continuous 8 EMA
// from an ascending candle array. Returns two parallel series suitable for
// lightweight-charts setData. VWAP uses typical price = (high+low+close)/3.
function computeOverlays(candles: Candle[]):
  { vwap: { time: number; value: number }[]; ema: { time: number; value: number }[] }
{
  const vwap: { time: number; value: number }[] = []
  const ema: { time: number; value: number }[] = []
  if (candles.length === 0) return { vwap, ema }

  let cumPV = 0, cumV = 0, curDay = -1
  for (const c of candles) {
    const day = sessionDay(c.time)
    if (day !== curDay) { cumPV = 0; cumV = 0; curDay = day }
    const typical = (c.high + c.low + c.close) / 3
    cumPV += typical * c.volume
    cumV  += c.volume
    vwap.push({ time: c.time, value: cumV > 0 ? cumPV / cumV : typical })
  }

  const alpha = 2 / (EMA_PERIOD + 1)
  let prev = candles[0].close
  ema.push({ time: candles[0].time, value: prev })
  for (let i = 1; i < candles.length; i++) {
    prev = alpha * candles[i].close + (1 - alpha) * prev
    ema.push({ time: candles[i].time, value: prev })
  }
  return { vwap, ema }
}

type VolumeLevel = { price: number; kind: string; scope: string; week: string; label: string; volume: number }
type VolumeSnapshot = { now_utc: string; price: number | null; atr_intraday: number | null; levels: VolumeLevel[]; m15_recent: unknown[] }

// Minimal US30 5-minute candle chart. ChartView is heavily XAUUSD-tuned (session
// VWAP from tick-volume, per-TF zoom memory, a deliberate no-fitContent policy for
// 1000-bar charts) which leaves US30's ~150 bars out of view — so this is a clean,
// self-contained candle chart that just loads M5 and fitContent()s.
function Us30Chart() {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const chartRef = useRef<any>(null)
  const seriesRef = useRef<any>(null)
  const firstRef = useRef(true)
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    const series = seriesRef.current
    if (!series) return
    try {
      const candles = await invoke<Candle[]>('get_trendbars', { symbol: 'US30', timeframe: 'M5', count: 200 })
      const sorted = [...candles].sort((a, b) => a.time - b.time)
      const dedup: Candle[] = []
      for (const c of sorted) {
        if (c.open > 0 && c.low > 0 && c.low < c.open * 0.5) continue // delta-decode glitch
        if (dedup.length === 0 || dedup[dedup.length - 1].time !== c.time) dedup.push(c)
        else dedup[dedup.length - 1] = c
      }
      if (dedup.length === 0) { setError('No US30 candles returned.'); return }
      series.setData(dedup.map(c => ({ time: c.time as any, open: c.open, high: c.high, low: c.low, close: c.close })))
      setError(null)
      if (firstRef.current) { chartRef.current?.timeScale().fitContent(); firstRef.current = false }
    } catch (e) { setError(String(e)) }
  }

  useEffect(() => {
    let cancelled = false
    let cleanup = () => {}
    ;(async () => {
      const { createChart, CandlestickSeries } = await import('lightweight-charts')
      if (!containerRef.current || cancelled) return
      const chart = createChart(containerRef.current, {
        autoSize: true,
        layout: { background: { color: '#0b0b0d' }, textColor: '#9a9aa0' },
        grid: { vertLines: { color: '#1b1b1f' }, horzLines: { color: '#1b1b1f' } },
        timeScale: { timeVisible: true, secondsVisible: false, rightOffset: 6, borderColor: '#26262b' },
        rightPriceScale: { borderColor: '#26262b' },
      })
      const series = chart.addSeries(CandlestickSeries, {
        upColor: '#2dd47b', downColor: '#f87171', borderVisible: false,
        wickUpColor: '#2dd47b', wickDownColor: '#f87171',
        priceFormat: { type: 'price', precision: 1, minMove: 0.1 },
      })
      chartRef.current = chart
      seriesRef.current = series
      await load()
      const id = window.setInterval(load, 10000) // keep the forming bar ~live
      cleanup = () => { window.clearInterval(id); chart.remove() }
    })()
    return () => { cancelled = true; cleanup() }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div>
      {error && <div className="archive-result err">Chart: {error}</div>}
      <div ref={containerRef} style={{ height: 460, width: '100%' }} />
    </div>
  )
}

// US30 (Dow) — locked 5-minute chart with on-demand opportunity detection.
type Us30Opp = {
  side: string; strategy?: string; entry: number; stop: number
  target1: number; target2: number | null; confidence: string; rationale: string
}
type Us30Result = { opportunities: Us30Opp[]; note?: string }

function US30View() {
  const [res, setRes] = useState<Us30Result | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const detect = async () => {
    setLoading(true); setError(null)
    try { setRes(await invoke<Us30Result>('get_us30_opportunities')) }
    catch (e) { setError(String(e)) }
    finally { setLoading(false) }
  }

  return (
    <div className="us30-view">
      <div className="archive-actions" style={{ marginBottom: 8 }}>
        <button className="btn" onClick={detect} disabled={loading}>
          {loading ? 'Detecting…' : 'Detect Opportunities'}
        </button>
      </div>

      {error && <div className="archive-result err">Error: {error}</div>}
      {res && (
        <div className="us30-opps">
          {res.note && <div className="muted" style={{ fontSize: 12, marginBottom: 6 }}>{res.note}</div>}
          {res.opportunities.length === 0
            ? <div className="placeholder">No clean setup right now.</div>
            : res.opportunities.map((o, i) => (
                <div key={i} className="us30-opp" style={{ borderLeftColor: leanColor(o.side) }}>
                  <div className="us30-opp-head">
                    <span className="sent-lean" style={{ color: leanColor(o.side) }}>{o.side}</span>
                    {o.strategy && <strong>{o.strategy}</strong>}
                    <span className="muted"> · {o.confidence}</span>
                    <button
                      className="btn us30-place"
                      disabled
                      title="US30 order placement — coming next (real money; needs the US30 order spec + sizing wired in)"
                    >
                      Place
                    </button>
                  </div>
                  <div className="muted" style={{ fontSize: 12 }}>
                    entry {o.entry} · stop {o.stop} · tp {o.target1}{o.target2 ? ` / ${o.target2}` : ''}
                  </div>
                  {o.rationale && <div style={{ fontSize: 12 }}>{o.rationale}</div>}
                </div>
              ))}
        </div>
      )}

      <Us30Chart />
    </div>
  )
}

function ChartView({ symbol, tick, lockTf, plain }: { symbol: string; tick: Tick | null; lockTf?: Timeframe; plain?: boolean }) {
  // Per-symbol display config. EURUSD prices need 5 decimals; the volume-profile
  // feature is XAUUSD-only (backed by an XAUUSD-specific command).
  const decimals = symbol === 'EURUSD' ? 5 : 2
  const minMove = 1 / Math.pow(10, decimals)
  const showVolume = symbol === 'XAUUSD'
  const containerRef = useRef<HTMLDivElement | null>(null)
  const chartRef = useRef<any>(null)
  const seriesRef = useRef<any>(null)
  // Indicator overlays — yellow VWAP, blue 8 EMA. Both follow the same
  // setData/update lifecycle as the candle series.
  const vwapSeriesRef = useRef<any>(null)
  const emaSeriesRef = useRef<any>(null)
  const vwapDataRef = useRef<{ time: number; value: number }[]>([])
  const emaDataRef = useRef<{ time: number; value: number }[]>([])
  // VWAP session state — running totals for CLOSED bars in the current session
  // (excludes the live in-progress bar). Lets us recompute VWAP on every tick
  // without re-walking the full candle array.
  const sessionDayRef = useRef(-1)
  const sessionCumPVRef = useRef(0)
  const sessionCumVRef = useRef(0)
  // Synthetic volume for the live bar: starts at cTrader's snapshot volume and
  // increments by 1 per tick. cTrader's tick stream carries price only (no
  // volume), so we use tick-count as the volume proxy — same convention cTrader
  // uses for its own trendbar volume on FX/CFD instruments.
  const liveBarVRef = useRef(0)
  const lastCandleRef = useRef<Candle | null>(null)
  // All loaded candles for the current TF (ascending). Lazy-load on pan-left
  // prepends older ones here, then we re-call setData with the full array.
  const allCandlesRef = useRef<Candle[]>([])
  const loadingMoreRef = useRef(false)
  const noMoreDataRef = useRef(false)
  // Remember each TF's visible *logical* range so switching M1→M3→M1 restores
  // the user's zoom/pan on M1. Logical range (bar indices) drives the chart's
  // internal bar spacing — without per-TF restore *and* a hard reset on first
  // visit, the previous TF's zoom would visibly leak into the next.
  const zoomByTfRef = useRef<Partial<Record<Timeframe, { from: number; to: number }>>>({})
  const [timeframe, setTimeframe] = useState<Timeframe>(lockTf ?? 'M1')
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Bumped each time the chart is (re)initialised so dependent effects re-run
  // and pick up the now-valid chartRef/seriesRef. Refs themselves don't trigger
  // effect re-runs in React.
  const [chartGen, setChartGen] = useState(0)
  // Mirror the live tick into a ref so the load effect can read the current
  // bid without needing `tick` in its deps (which would refetch on every tick).
  const tickRef = useRef<Tick | null>(tick)
  useEffect(() => { tickRef.current = tick }, [tick])

  // ── Volume levels overlay (this week + last week POC / value area) ─────────
  const [volOn, setVolOn] = useState(false)
  const [volLevels, setVolLevels] = useState<VolumeLevel[]>([])
  const [volIdea, setVolIdea] = useState<ModelTradeIdea | null>(null)
  const [volLoading, setVolLoading] = useState(false)
  const [volError, setVolError] = useState<string | null>(null)
  const priceLinesRef = useRef<any[]>([])
  const volSnapRef = useRef<VolumeSnapshot | null>(null)
  const volLevelsRef = useRef<VolumeLevel[]>([])
  const volPollRef = useRef<number | null>(null)

  // Draw one horizontal price line per volume level on the candle series.
  // Colour by recency (today = cyan, this week = gold, last week = gray);
  // POC solid + bold, VAH/VAL dashed.
  const drawVolumeLevels = (levels: VolumeLevel[]) => {
    const s = seriesRef.current
    if (!s) return
    for (const pl of priceLinesRef.current) { try { s.removePriceLine(pl) } catch { /* ignore */ } }
    priceLinesRef.current = []
    for (const lv of levels) {
      const color = lv.scope === 'today' ? '#22d3ee' : lv.week === 'this' ? '#f4c430' : '#9ca3af'
      const isPoc = lv.kind === 'POC'
      try {
        const pl = s.createPriceLine({
          price: lv.price, color,
          lineWidth: isPoc ? 2 : 1,
          lineStyle: isPoc ? 0 : 2,   // 0 = solid, 2 = dashed
          axisLabelVisible: true,
          title: `${lv.kind} ${lv.label}`,
        })
        priceLinesRef.current.push(pl)
      } catch { /* series may be mid-rebuild */ }
    }
  }

  const clearVolumeLevels = () => {
    const s = seriesRef.current
    if (s) for (const pl of priceLinesRef.current) { try { s.removePriceLine(pl) } catch { /* ignore */ } }
    priceLinesRef.current = []
  }

  const refreshVolLevels = async () => {
    try {
      const snap = await invoke<VolumeSnapshot>('get_volume_levels')
      volSnapRef.current = snap
      volLevelsRef.current = snap.levels
      setVolLevels(snap.levels)
      drawVolumeLevels(snap.levels)
      setVolError(null)
    } catch (e) {
      setVolError(String(e))
      console.error('volume levels:', e)
    }
  }

  const askVolumeClaude = async () => {
    if (!volSnapRef.current) return
    setVolLoading(true)
    try {
      const idea = await invoke<ModelTradeIdea>('get_volume_trade_idea', { snapshot: volSnapRef.current })
      setVolIdea(idea)
    } catch (e) {
      setVolIdea({
        provider: 'Claude Volume', model: '', ok: false, bias: null, strategy: null,
        entry_low: null, entry_high: null, stop: null, target1: null, target2: null,
        rationale: null, error: String(e), duration_ms: 0,
      })
    } finally {
      setVolLoading(false)
    }
  }

  const toggleVolume = async () => {
    if (volOn) {
      setVolOn(false)
      clearVolumeLevels()
      setVolLevels([]); setVolIdea(null); setVolError(null)
      volSnapRef.current = null; volLevelsRef.current = []
      if (volPollRef.current) { window.clearInterval(volPollRef.current); volPollRef.current = null }
      return
    }
    setVolOn(true)
    await refreshVolLevels()
    askVolumeClaude()  // fire-and-forget; the lines are already drawn
    volPollRef.current = window.setInterval(refreshVolLevels, 60_000)  // live refresh
  }

  // Redraw levels after the chart is rebuilt (a TF switch recreates the series).
  useEffect(() => {
    if (volOn) { priceLinesRef.current = []; drawVolumeLevels(volLevelsRef.current) }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chartGen])

  // Stop the live-refresh timer on unmount.
  useEffect(() => () => { if (volPollRef.current) window.clearInterval(volPollRef.current) }, [])

  // Seed VWAP session refs from a candle array. cumPV/cumV cover CLOSED bars in
  // the current session day (everything in the day except the last bar);
  // liveBarV is seeded with the last bar's snapshot volume so the initial VWAP
  // is continuous with computeOverlays' result.
  const initVwapSession = (candles: Candle[]) => {
    if (candles.length === 0) {
      sessionDayRef.current = -1
      sessionCumPVRef.current = 0
      sessionCumVRef.current = 0
      liveBarVRef.current = 0
      return
    }
    const last = candles[candles.length - 1]
    const day = sessionDay(last.time)
    const dayStart = sessionDayStart(day)
    let cumPV = 0, cumV = 0
    for (let i = 0; i < candles.length - 1; i++) {
      const c = candles[i]
      if (c.time < dayStart) continue
      cumPV += ((c.high + c.low + c.close) / 3) * c.volume
      cumV  += c.volume
    }
    sessionDayRef.current = day
    sessionCumPVRef.current = cumPV
    sessionCumVRef.current = cumV
    liveBarVRef.current = last.volume
  }

  // Recreate the chart on every symbol *or* timeframe change. The chart
  // instance carries hidden zoom/scroll state that survives setData and resists
  // applyOptions/resetTimeScale, so the only way to guarantee each timeframe
  // starts with its own clean (or saved) zoom is to throw away the old chart
  // and build a new one. Cache hits keep this near-instant.
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      const { createChart, CandlestickSeries, LineSeries } = await import('lightweight-charts')
      if (cancelled || !containerRef.current) return

      // Tear down any previous chart for this symbol/TF.
      if (chartRef.current) {
        try { chartRef.current.remove() } catch { /* ignore */ }
        chartRef.current = null
        seriesRef.current = null
        vwapSeriesRef.current = null
        emaSeriesRef.current = null
      }

      const chart = createChart(containerRef.current, {
        layout: {
          background: { color: '#0e0e10' },
          textColor: '#d4d4d8',
          attributionLogo: false,  // hide the TradingView watermark
        },
        grid: {
          vertLines: { color: '#1f2128' },
          horzLines: { color: '#1f2128' },
        },
        timeScale: {
          timeVisible: true,
          secondsVisible: false,
          borderColor: '#25272d',
          rightOffset: 8,        // a few empty bars on the right for breathing room
          barSpacing: 6,          // pixels per bar — TradingView default
          // Display X-axis tick marks in the user's *local* timezone (matches
          // cTrader's default). lightweight-charts defaults to UTC. We feed it
          // UTC unix seconds (unchanged); only the formatter shifts the display.
          tickMarkFormatter: (time: any) => {
            const d = new Date((time as number) * 1000)
            // Hour:Minute for intraday density (M1-H1); fall back to short
            // date for daily-and-above tick density.
            if (timeframe === 'D1' || timeframe === 'W1' || timeframe === 'MN1') {
              return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
            }
            return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
          },
        },
        localization: {
          // Crosshair time tooltip — also local timezone.
          timeFormatter: (time: any) => {
            const d = new Date((time as number) * 1000)
            return d.toLocaleString(undefined, {
              year: 'numeric', month: '2-digit', day: '2-digit',
              hour: '2-digit', minute: '2-digit',
            })
          },
        },
        rightPriceScale: { borderColor: '#25272d' },
        autoSize: true,
      })
      const series = chart.addSeries(CandlestickSeries, {
        upColor: '#2dd47b',  downColor: '#f87171',
        borderUpColor: '#2dd47b', borderDownColor: '#f87171',
        wickUpColor: '#2dd47b',   wickDownColor: '#f87171',
        priceFormat: { type: 'price', precision: decimals, minMove },
      })
      // VWAP (yellow) + 8 EMA (blue) overlays. Always create the series so
      // the lifecycle is identical across TFs; hide on non-intraday TFs by
      // pushing empty data.
      // `plain` (e.g. US30): no overlays — index trendbars can carry 0 volume,
      // which makes the client-side VWAP NaN/0 and stretches the price scale,
      // squashing the candles. Showing just candles keeps the scale correct.
      const vwapSeries = plain ? null : chart.addSeries(LineSeries, {
        color: '#f4c430',          // VWAP — gold/yellow
        lineWidth: 2,
        priceLineVisible: false,
        lastValueVisible: false,
        title: 'VWAP',
      })
      const emaSeries = plain ? null : chart.addSeries(LineSeries, {
        color: '#3b82f6',          // 8 EMA — blue
        lineWidth: 2,
        priceLineVisible: false,
        lastValueVisible: false,
        title: '8 EMA',
      })
      chartRef.current = chart
      seriesRef.current = series
      vwapSeriesRef.current = vwapSeries
      emaSeriesRef.current = emaSeries
      setChartGen(g => g + 1)
    })()
    return () => {
      cancelled = true
      if (chartRef.current) {
        try { chartRef.current.remove() } catch { /* ignore */ }
        chartRef.current = null
        seriesRef.current = null
      }
    }
  }, [symbol, timeframe])

  // Reload historical bars whenever timeframe or symbol changes.
  useEffect(() => {
    if (!seriesRef.current) {
      // The chart isn't ready yet; this effect will rerun once the init effect
      // sets seriesRef. We trigger that by depending on a small timer below.
    }
    let cancelled = false
    setError(null)
    // Clear in-memory state so a tick that arrives during fetch doesn't draw
    // a candle using the previous TF's bucket size or last candle. We do NOT
    // clear the chart's series.setData here — the previous TF's bars stay
    // visible until the new ones arrive, so cache hits feel like an instant swap.
    lastCandleRef.current = null
    allCandlesRef.current = []
    // Only show the loading overlay if the fetch is actually slow — cache hits
    // typically return in <100ms and we'd rather not flash the overlay then.
    const loadingTimer = window.setTimeout(() => { if (!cancelled) setLoading(true) }, 250)

    const tryLoad = async () => {
      // Wait briefly for chart init if needed.
      for (let i = 0; i < 20 && !seriesRef.current; i++) {
        await new Promise(r => setTimeout(r, 50))
      }
      if (cancelled || !seriesRef.current) return
      try {
        const count = BARS_PER_TF[timeframe]
        const candles = await invoke<Candle[]>('get_trendbars', { symbol, timeframe, count })
        if (cancelled) return
        // Defensive: sort ascending + dedupe + filter outliers. Lightweight-charts
        // requires strictly-increasing timestamps; cTrader occasionally returns
        // dupes or bars with broken delta-decoded lows (e.g., low ≈ 0).
        const sorted = [...candles].sort((a, b) => a.time - b.time)
        const deduped: Candle[] = []
        for (const c of sorted) {
          // Drop suspicious bars: low < 50% of open ⇒ delta-decoding glitch.
          if (c.open > 0 && c.low > 0 && c.low < c.open * 0.5) continue
          if (deduped.length === 0 || deduped[deduped.length - 1].time !== c.time) {
            deduped.push(c)
          } else {
            deduped[deduped.length - 1] = c  // last write wins
          }
        }
        console.log(`[chart] ${timeframe} fetched ${candles.length}, sorted+deduped+filtered to ${deduped.length}`)
        allCandlesRef.current = deduped
        noMoreDataRef.current = false
        loadingMoreRef.current = false
        try {
          seriesRef.current.setData(deduped.map(c => ({
            time: c.time as any,
            open: c.open, high: c.high, low: c.low, close: c.close,
          })))
        } catch (chartErr) {
          console.error('[chart] setData failed', chartErr, deduped.slice(0, 3))
          setError(`Chart render failed: ${String(chartErr)}`)
          setLoading(false)
          return
        }
        // Overlays: VWAP (daily anchor) + 8 EMA. Only on intraday TFs.
        if (isIndicatorTf(timeframe)) {
          const { vwap, ema } = computeOverlays(deduped)
          vwapDataRef.current = vwap
          emaDataRef.current = ema
          vwapSeriesRef.current?.setData(vwap.map(p => ({ time: p.time as any, value: p.value })))
          emaSeriesRef.current?.setData(ema.map(p => ({ time: p.time as any, value: p.value })))
          initVwapSession(deduped)
        } else {
          vwapDataRef.current = []
          emaDataRef.current = []
          vwapSeriesRef.current?.setData([])
          emaSeriesRef.current?.setData([])
          initVwapSession([])
        }
        if (deduped.length === 0) {
          setError(`No ${timeframe} bars available for this window. Try a different timeframe or wait for market hours.`)
        }
        lastCandleRef.current = deduped.length > 0 ? deduped[deduped.length - 1] : null
        // cTrader's historical trendbar close doesn't always track the current
        // bid (often a half-spread or full-spread offset depending on the broker's
        // bar-builder). Rebase the last bar's close to the latest tick.bid so the
        // chart's price label matches the sidebar bid from the moment the chart
        // opens — subsequent ticks will keep it in sync.
        const liveTick = tickRef.current
        if (lastCandleRef.current && liveTick && liveTick.symbol === symbol) {
          const last = lastCandleRef.current
          last.close = liveTick.bid
          last.high = Math.max(last.high, liveTick.bid)
          last.low = Math.min(last.low, liveTick.bid)
          seriesRef.current.update({
            time: last.time as any,
            open: last.open, high: last.high, low: last.low, close: last.close,
          })
        }
        setLoading(false)
        if (deduped.length > 0) {
          // The chart is fresh (recreated on every TF change), so no need to
          // reset zoom — just apply the saved range for this TF, or the
          // default 120-bar TradingView-style window.
          const ts = chartRef.current?.timeScale()
          const saved = zoomByTfRef.current[timeframe]
          if (ts) {
            if (saved) {
              try { ts.setVisibleLogicalRange({ from: saved.from, to: saved.to }) }
              catch { /* ignore */ }
            } else {
              const n = deduped.length
              const visibleCount = Math.min(120, n)
              ts.setVisibleLogicalRange({ from: n - visibleCount, to: n + 8 })
            }
          }
        }
      } catch (e) {
        if (!cancelled) {
          console.error('[chart] invoke failed', e)
          setError(String(e))
          setLoading(false)
        }
      }
    }
    tryLoad().finally(() => {
      window.clearTimeout(loadingTimer)
      if (!cancelled) setLoading(false)
    })
    return () => { cancelled = true; window.clearTimeout(loadingTimer) }
  }, [symbol, timeframe])

  // Subscribe to pan-left so we can lazy-load older bars from cTrader on demand.
  // Lazy on drag only — no preload after the initial 1000.
  useEffect(() => {
    if (!chartRef.current) return
    const ts = chartRef.current.timeScale()
    const handler = (range: any) => {
      if (!range) return
      console.log('[chart] visible range', range.from?.toFixed(1), '->', range.to?.toFixed(1))
      if (loadingMoreRef.current || noMoreDataRef.current) return
      // `range.from` is a fractional logical index. < 50 means user has panned
      // within 50 bars of the leftmost loaded candle (or past it into empty space).
      if (range.from < 50) {
        console.log('[chart] triggering load-more (range.from=' + range.from?.toFixed(1) + ')')
        loadMoreHistory()
      }
    }
    ts.subscribeVisibleLogicalRangeChange(handler)
    return () => { try { ts.unsubscribeVisibleLogicalRangeChange(handler) } catch {} }
  }, [timeframe, symbol, chartGen])

  const loadMoreHistory = async () => {
    if (loadingMoreRef.current || noMoreDataRef.current) return
    if (allCandlesRef.current.length === 0 || !seriesRef.current) return
    loadingMoreRef.current = true
    setLoadingMore(true)
    try {
      const oldestSec = allCandlesRef.current[0].time
      const oldestMs = oldestSec * 1000

      // Retry with widening windows so weekend/holiday gaps don't permanently
      // strand us. Start at 1× requested bars, then 2×, 4×, 8× — covers up to
      // ~8000 minutes / ~5.5 days, more than enough for a weekend gap.
      let older: Candle[] = []
      const baseCount = BARS_PER_TF[timeframe]
      for (const mult of [1, 2, 4, 8]) {
        const tryCount = baseCount * mult
        older = await invoke<Candle[]>('get_trendbars', {
          symbol, timeframe, count: tryCount, toMs: oldestMs,
        })
        if (older.length > 0) {
          if (mult > 1) console.log(`[chart] empty windows skipped, succeeded at ${mult}×`)
          break
        }
        console.log(`[chart] empty window at ${mult}× — widening`)
      }
      if (older.length === 0) {
        console.log('[chart] no older data found after 4 attempts — reached start of history')
        noMoreDataRef.current = true
        return
      }
      // Merge: older bars + existing, dedupe overlap, filter outliers.
      const merged = [...older, ...allCandlesRef.current]
      merged.sort((a, b) => a.time - b.time)
      const deduped: Candle[] = []
      for (const c of merged) {
        if (c.open > 0 && c.low > 0 && c.low < c.open * 0.5) continue
        if (deduped.length === 0 || deduped[deduped.length - 1].time !== c.time) deduped.push(c)
        else deduped[deduped.length - 1] = c
      }
      // If the merge didn't actually grow the list (cTrader keeps returning bars
      // we already have because we're at the start of history), latch noMoreData
      // and DON'T re-render or shift the visible range — that would compress
      // the chart to the right edge over repeated useless pans.
      const grew = deduped.length > allCandlesRef.current.length
      if (!grew) {
        console.log('[chart] no new bars added — reached start of history, latching')
        noMoreDataRef.current = true
        return
      }
      const prevLength = allCandlesRef.current.length
      const newBars = deduped.length - prevLength
      allCandlesRef.current = deduped
      console.log(`[chart] +${newBars} new older bars, total ${deduped.length}`)
      // Preserve current visible range so the chart doesn't snap.
      const tsApi = chartRef.current.timeScale()
      const range = tsApi.getVisibleLogicalRange()
      seriesRef.current.setData(deduped.map(c => ({
        time: c.time as any,
        open: c.open, high: c.high, low: c.low, close: c.close,
      })))
      // Recompute overlays — prepending older bars can shift VWAP day-anchors
      // and the EMA seed, so a full recompute is the safe path.
      if (isIndicatorTf(timeframe)) {
        const { vwap, ema } = computeOverlays(deduped)
        vwapDataRef.current = vwap
        emaDataRef.current = ema
        vwapSeriesRef.current?.setData(vwap.map(p => ({ time: p.time as any, value: p.value })))
        emaSeriesRef.current?.setData(ema.map(p => ({ time: p.time as any, value: p.value })))
        initVwapSession(deduped)
      }
      if (range && newBars > 0) {
        // Shift the visible range right by the actual number of newly prepended bars.
        tsApi.setVisibleLogicalRange({
          from: range.from + newBars,
          to: range.to + newBars,
        })
      }
    } catch (e) {
      console.error('[chart] load-more failed', e)
    } finally {
      loadingMoreRef.current = false
      setLoadingMore(false)
    }
  }

  // Live tick → update the current candle of the selected timeframe.
  // Anchored to the last historical bar's time so calendar TFs (W1/MN1) align
  // with cTrader's bar convention instead of arbitrary epoch-floored buckets.
  useEffect(() => {
    if (!tick || !seriesRef.current || tick.symbol !== symbol) return
    if (loading || !lastCandleRef.current) return
    // Use the bid as the live close so chart price matches the sidebar bid
    // (standard trading-platform convention).
    const mid = tick.bid
    const secsPerBar = SECONDS_PER_BAR[timeframe]
    const nowSec = Math.floor(Date.now() / 1000)
    const prev = lastCandleRef.current
    // Anchor bucket math to prev.time so calendar TFs (W1 anchored to Sun 21:00
    // UTC, MN1 anchored to broker-month start) keep their alignment instead of
    // landing on arbitrary epoch-floored boundaries.
    let cycles = Math.max(0, Math.floor((nowSec - prev.time) / secsPerBar))
    let bucket = prev.time + cycles * secsPerBar
    // MN1 sanity: months are 28-31 days but we approximate with 30 → can overshoot
    // by a month. Walk cycles back until bucket <= now.
    while (bucket > nowSec && cycles > 0) {
      cycles -= 1
      bucket = prev.time + cycles * secsPerBar
    }

    if (timeframe === 'W1' || timeframe === 'MN1') {
      console.log(`[chart] tick ${timeframe}: prev=${prev.time}, bucket=${bucket}, cycles=${cycles}, mid=${mid.toFixed(2)}, same=${prev.time === bucket}`)
    }
    if (prev.time === bucket) {
      // Same bucket → update OHLC of the in-progress candle.
      prev.high = Math.max(prev.high, mid)
      prev.low = Math.min(prev.low, mid)
      prev.close = mid
      seriesRef.current.update({
        time: prev.time as any,
        open: prev.open, high: prev.high, low: prev.low, close: prev.close,
      })
      // Tick within the live bar: refresh EMA from the new close, and refresh
      // VWAP by adding +1 of synthetic volume to the live bar and re-deriving
      // the running session average.
      if (isIndicatorTf(timeframe)) {
        if (emaDataRef.current.length >= 2) {
          const ema = emaDataRef.current
          const alpha = 2 / (EMA_PERIOD + 1)
          const prevEma = ema[ema.length - 2].value
          const newEma = alpha * mid + (1 - alpha) * prevEma
          ema[ema.length - 1] = { time: prev.time, value: newEma }
          emaSeriesRef.current?.update({ time: prev.time as any, value: newEma })
        }
        liveBarVRef.current += 1
        const liveTypical = (prev.high + prev.low + prev.close) / 3
        const cumPV = sessionCumPVRef.current + liveTypical * liveBarVRef.current
        const cumV  = sessionCumVRef.current  + liveBarVRef.current
        const vwapVal = cumV > 0 ? cumPV / cumV : liveTypical
        if (vwapDataRef.current.length > 0) {
          vwapDataRef.current[vwapDataRef.current.length - 1] = { time: prev.time, value: vwapVal }
          vwapSeriesRef.current?.update({ time: prev.time as any, value: vwapVal })
        }
      }
    } else if (bucket > prev.time) {
      // Bucket advanced — open a new live bar at `bucket`. If cTrader's historical
      // ended several buckets before "now" there will be a visible time gap
      // between the last historical bar and this live one; that's accurate
      // (we don't have data for those missing buckets) and the chart price
      // tracks the sidebar.
      const newCandle: Candle = { time: bucket, open: mid, high: mid, low: mid, close: mid, volume: 0 }
      seriesRef.current.update({ time: bucket as any, open: mid, high: mid, low: mid, close: mid })
      lastCandleRef.current = newCandle
      // Also push into allCandlesRef so pan-left math stays consistent.
      allCandlesRef.current = [...allCandlesRef.current, newCandle]
      // Extend the overlays with one new point for this fresh bar. VWAP rolls
      // the just-closed live bar into the session totals, resets if we crossed
      // the 21:00 UTC session boundary, then seeds the new live bar; EMA
      // chains off the previous EMA value.
      if (isIndicatorTf(timeframe)) {
        const typical = mid  // O=H=L=C on a fresh bar
        // Finalise the previous (now-closed) live bar: its synthetic V went into
        // liveBarVRef, and its OHLC is in `prev`. Add that bar's contribution to
        // the session totals if it belonged to the current session.
        const prevDay = sessionDay(prev.time)
        if (prevDay === sessionDayRef.current && liveBarVRef.current > 0) {
          const prevTypical = (prev.high + prev.low + prev.close) / 3
          sessionCumPVRef.current += prevTypical * liveBarVRef.current
          sessionCumVRef.current  += liveBarVRef.current
        }
        // Cross a session boundary? Reset session totals before seeding the new bar.
        const newDay = sessionDay(bucket)
        if (newDay !== sessionDayRef.current) {
          sessionDayRef.current = newDay
          sessionCumPVRef.current = 0
          sessionCumVRef.current = 0
        }
        // New live bar starts with 0 synthetic volume; ticks will accumulate it.
        liveBarVRef.current = 0
        const cumV = sessionCumVRef.current
        const vwapVal = cumV > 0 ? sessionCumPVRef.current / cumV : typical
        vwapDataRef.current.push({ time: bucket, value: vwapVal })
        vwapSeriesRef.current?.update({ time: bucket as any, value: vwapVal })

        const alpha = 2 / (EMA_PERIOD + 1)
        const prevEma = emaDataRef.current[emaDataRef.current.length - 1]?.value ?? typical
        const newEma = alpha * typical + (1 - alpha) * prevEma
        emaDataRef.current.push({ time: bucket, value: newEma })
        emaSeriesRef.current?.update({ time: bucket as any, value: newEma })
      }
      // Don't fitContent here — it would zoom out to fit all 1000+ bars and
      // override the "show last 120" view we set after the initial load.
      // The chart's rightOffset already leaves room for new bars on the right.
    }
    // bucket < prev.time → ignore stale tick (clock skew, etc.).
  }, [tick, symbol, timeframe, loading])

  return (
    <div className="chart-view">
      <div className="chart-header">
        <span className="chart-symbol">{symbol}</span>
        <div className="chart-tfs">
          {lockTf ? (
            <span className="chart-tf-btn active">{lockTf}</span>
          ) : TIMEFRAMES.map((t) => (
            <button
              key={t}
              className={`chart-tf-btn ${t === timeframe ? 'active' : ''}`}
              onClick={() => {
                if (t === timeframe) return
                // Snapshot the current TF's visible *logical* range so we can
                // restore exact zoom + scroll position when the user switches
                // back. Logical range drives bar spacing.
                const ts = chartRef.current?.timeScale()
                const range = ts?.getVisibleLogicalRange?.()
                if (range && typeof range.from === 'number' && typeof range.to === 'number') {
                  zoomByTfRef.current[timeframe] = { from: range.from, to: range.to }
                }
                setTimeframe(t)
              }}
              disabled={loading}
            >
              {t}
            </button>
          ))}
          {showVolume && (
            <button
              className={`chart-tf-btn ${volOn ? 'active' : ''}`}
              onClick={toggleVolume}
              disabled={loading}
              title="Volume levels — this week + last week POC / value area, live + Claude read"
            >
              {volOn ? `Vol ✓ (${volLevels.length})` : 'Vol Levels'}
            </button>
          )}
        </div>
        {tick && (
          <span className="chart-price">
            bid <strong>{tick.bid.toFixed(decimals)}</strong> · ask <strong>{tick.ask.toFixed(decimals)}</strong>
          </span>
        )}
      </div>
      <div ref={containerRef} className="chart-canvas" />
      {loading && <div className="chart-overlay">Loading {timeframe} bars…</div>}
      {error && <div className="chart-overlay err">Error: {error}</div>}
      {loadingMore && !loading && (
        <div className="chart-loadmore">Loading older bars…</div>
      )}
      {volOn && (
        <div style={{ position: 'absolute', top: 56, right: 12, width: 300, maxWidth: '42%', background: '#16181d', border: '1px solid #2a2d35', borderRadius: 8, padding: '10px 12px', zIndex: 20, fontSize: 13 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6 }}>
            <strong>Claude Volume{volIdea?.model ? ` · ${volIdea.model}` : ''}</strong>
            <span>
              <button onClick={askVolumeClaude} disabled={volLoading || volLevels.length === 0} title="Re-ask" style={{ marginRight: 8, cursor: 'pointer', background: 'none', border: 'none', color: '#d4d4d8' }}>↻</button>
              <button onClick={toggleVolume} title="Close" style={{ cursor: 'pointer', background: 'none', border: 'none', color: '#d4d4d8' }}>×</button>
            </span>
          </div>
          {volError ? (
            <div style={{ color: '#f87171' }}>error: {volError}</div>
          ) : volLevels.length === 0 ? (
            <div className="muted">fetching volume levels…</div>
          ) : volLoading && !volIdea ? (
            <div className="muted">{volLevels.length} levels drawn · analyzing…</div>
          ) : volIdea?.ok ? (
            <>
              <div style={{ fontWeight: 700, color: volIdea.bias === 'LONG' ? '#2dd47b' : volIdea.bias === 'SHORT' ? '#f87171' : '#9ca3af' }}>
                {volIdea.bias} · {volIdea.strategy}
              </div>
              {volIdea.bias !== 'FLAT' && (
                <div style={{ margin: '4px 0' }}>
                  Entry {volIdea.entry_low ?? '–'}–{volIdea.entry_high ?? '–'} · SL {volIdea.stop ?? '–'} · TP {volIdea.target1 ?? '–'}/{volIdea.target2 ?? '–'}
                </div>
              )}
              <div className="muted">{volIdea.rationale}</div>
            </>
          ) : (
            <div style={{ color: '#f87171' }}>{volIdea?.error ?? 'no response'}</div>
          )}
        </div>
      )}
    </div>
  )
}

function DashboardView({ tick }: { tick: Tick | null }) {
  const [loading, setLoading] = useState(false)
  const [progress, setProgress] = useState<{ tf: Timeframe; done: number } | null>(null)
  const [done, setDone] = useState<{ count: number; failed: string[] } | null>(null)

  const loadAll = async () => {
    setLoading(true)
    setDone(null)
    const failed: string[] = []
    for (let i = 0; i < TIMEFRAMES.length; i++) {
      const tf = TIMEFRAMES[i]
      setProgress({ tf, done: i })
      try {
        await invoke<Candle[]>('get_trendbars', {
          symbol: 'XAUUSD', timeframe: tf, count: BARS_PER_TF[tf],
          forceRefresh: true,  // bypass cache so the data is genuinely fresh
        })
      } catch (e) {
        console.error(`[loadgold] ${tf} failed`, e)
        failed.push(tf)
      }
    }
    setProgress(null)
    setLoading(false)
    setDone({ count: TIMEFRAMES.length - failed.length, failed })
  }

  return (
    <div className="dashboard">
      <div className="dashboard-card">
        <h2>Welcome to cTrader Rust Terminal</h2>
        <p className="muted">
          {tick
            ? `Live ${tick.symbol} — bid ${tick.bid.toFixed(2)} / ask ${tick.ask.toFixed(2)}`
            : 'Waiting for live ticks…'}
        </p>
        <div className="dashboard-actions">
          <button
            className="btn btn-loadgold"
            onClick={loadAll}
            disabled={loading}
          >
            {loading ? `Caching ${progress?.tf} (${(progress?.done ?? 0) + 1}/${TIMEFRAMES.length})…` : 'Load Gold'}
          </button>
          <span className="muted small">
            Pre-fetches XAUUSD bars for all 14 timeframes into the local cache so
            charts open instantly.
          </span>
        </div>
        {done && (
          <div className={`dashboard-result ${done.failed.length > 0 ? 'partial' : 'ok'}`}>
            {done.failed.length === 0
              ? `✓ All ${done.count} timeframes cached. Click XAUUSD in the sidebar to open the chart.`
              : `Cached ${done.count}/${TIMEFRAMES.length}. Failed: ${done.failed.join(', ')}`}
          </div>
        )}
        <p className="muted small">Or pick an instrument from the sidebar to jump straight to its chart.</p>
      </div>
    </div>
  )
}

// ── Archives tab: backend-management view (ported from origin/web_gold) ──────
function ArchiveView() {
  // News_Updates state
  const [newsBusy, setNewsBusy] = useState(false)
  const [newsResult, setNewsResult] = useState<NewsUpdateResult | null>(null)
  const [newsError, setNewsError] = useState<string | null>(null)

  // EC_Gold_Events_Update state
  const [ecBusy, setEcBusy] = useState(false)
  const [ecResult, setEcResult] = useState<EcGoldUpdateResult | null>(null)
  const [ecError, setEcError] = useState<string | null>(null)
  const [ecProgress, setEcProgress] = useState<EcGoldProgress | null>(null)

  // EC_Gold_events_storage state
  const [ecStoreBusy, setEcStoreBusy] = useState(false)
  const [ecStoreResult, setEcStoreResult] = useState<EcGoldStorageResult | null>(null)
  const [ecStoreError, setEcStoreError] = useState<string | null>(null)
  const [ecStoreProgress, setEcStoreProgress] = useState<EcGoldStorageProgress | null>(null)

  // Gold DB timeframe stats (auto-loads on tab mount + refresh button)
  const [tfStats, setTfStats] = useState<XauusdStatsResult | null>(null)
  const [tfStatsBusy, setTfStatsBusy] = useState(false)
  const [tfStatsError, setTfStatsError] = useState<string | null>(null)

  // Import-disk-archives state (loads on-disk JSON files back into the DB)
  const [importBusy, setImportBusy] = useState(false)
  const [importResult, setImportResult] = useState<ArchiveImportResult | null>(null)
  const [importError, setImportError] = useState<string | null>(null)

  const loadTfStats = async () => {
    setTfStatsBusy(true); setTfStatsError(null)
    try { setTfStats(await invoke<XauusdStatsResult>('get_xauusd_tf_stats')) }
    catch (e) { setTfStatsError(String(e)) }
    finally { setTfStatsBusy(false) }
  }
  useEffect(() => { loadTfStats() /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [])

  // Live backfill state: poll every 500 ms while the tab is open so the
  // user sees the chunk-by-chunk progress of the background history backfill
  // in near real-time. Auto-refreshes the TF stats panel too once backfill
  // status flips to "complete" so the new row counts appear without a
  // manual Refresh click.
  const [backfill, setBackfill] = useState<BackfillState | null>(null)
  const prevStatusRef = useRef<string | null>(null)
  useEffect(() => {
    let cancelled = false
    const tick = async () => {
      try {
        const s = await invoke<BackfillState>('get_history_backfill_state')
        if (cancelled) return
        setBackfill(s)
        // When backfill flips from running → complete, refresh the TF stats
        // panel so the user sees the final row counts immediately.
        if (prevStatusRef.current === 'running' && s.status === 'complete') {
          loadTfStats()
        }
        prevStatusRef.current = s.status
      } catch { /* ignore — backend may be starting up */ }
    }
    tick()
    const id = window.setInterval(tick, 500)
    return () => { cancelled = true; window.clearInterval(id) }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const runImport = async () => {
    setImportBusy(true); setImportError(null); setImportResult(null)
    try {
      const r = await invoke<ArchiveImportResult>('import_disk_archives')
      setImportResult(r)
      loadTfStats()  // refresh DB stats so new row counts show immediately
    }
    catch (e) { setImportError(String(e)) }
    finally { setImportBusy(false) }
  }

  const runNewsUpdate = async () => {
    setNewsBusy(true); setNewsError(null); setNewsResult(null)
    try { setNewsResult(await invoke<NewsUpdateResult>('update_news_archive')) }
    catch (e) { setNewsError(String(e)) }
    finally { setNewsBusy(false) }
  }

  const runEcGoldUpdate = async () => {
    setEcBusy(true); setEcError(null); setEcResult(null); setEcProgress(null)
    // Subscribe to chunk-by-chunk progress events from the Rust side
    // BEFORE we invoke, so we never miss the first chunk's event.
    let unlisten: UnlistenFn | null = null
    try {
      unlisten = await listen<EcGoldProgress>('ec_gold_progress', (e) => {
        setEcProgress(e.payload)
      })
      setEcResult(await invoke<EcGoldUpdateResult>('update_ec_gold_events'))
    } catch (e) {
      setEcError(String(e))
    } finally {
      if (unlisten) unlisten()
      setEcBusy(false)
      setEcProgress(null)  // hide the live bar — the result panel takes over
    }
  }

  const runEcGoldStorage = async () => {
    setEcStoreBusy(true); setEcStoreError(null); setEcStoreResult(null); setEcStoreProgress(null)
    let unlisten: UnlistenFn | null = null
    try {
      unlisten = await listen<EcGoldStorageProgress>('ec_gold_storage_progress', (e) => {
        setEcStoreProgress(e.payload)
      })
      setEcStoreResult(await invoke<EcGoldStorageResult>('store_ec_gold_events'))
    } catch (e) {
      setEcStoreError(String(e))
    } finally {
      if (unlisten) unlisten()
      setEcStoreBusy(false)
      setEcStoreProgress(null)
    }
  }

  // Derived: percent + simple ETA from the latest progress event.
  const ecPct = ecProgress
    ? Math.min(100, Math.round((ecProgress.chunks_done / ecProgress.chunks_total) * 100))
    : 0
  const ecEtaSecs = ecProgress && ecProgress.chunks_done > 0
    ? Math.round(ecProgress.elapsed_secs / ecProgress.chunks_done * (ecProgress.chunks_total - ecProgress.chunks_done))
    : null

  const ecStorePct = ecStoreProgress
    ? Math.min(100, Math.round((ecStoreProgress.files_done / ecStoreProgress.files_total) * 100))
    : 0
  const ecStoreEtaSecs = ecStoreProgress && ecStoreProgress.files_done > 0
    ? Math.round(ecStoreProgress.elapsed_secs / ecStoreProgress.files_done
                 * (ecStoreProgress.files_total - ecStoreProgress.files_done))
    : null

  return (
    <div className="archive">
      <h3>Gold DB timeframe states</h3>
      <p className="muted">
        One row per <code>xauusd_&lt;tf&gt;</code> table — bars stored, coverage span,
        and how stale the newest bar is. Tail freshness is colour-coded against the
        bar width: green if &lt; 2 bars old, amber if &lt; 10, red otherwise. The market
        state is shown above the table since "stale" is expected during weekend closure.
      </p>

      <div className="archive-actions">
        <button className="btn" onClick={loadTfStats} disabled={tfStatsBusy}>
          {tfStatsBusy ? 'Loading…' : 'Refresh DB stats'}
        </button>
        <button
          className="btn"
          onClick={async () => {
            try { await invoke<string>('start_history_backfill') }
            catch (e) { alert('Start failed: ' + String(e)) }
          }}
          disabled={backfill?.status === 'running'}
        >
          {backfill?.status === 'running'
            ? `Updating… (${backfill.current_tf?.toUpperCase() ?? '...'} in progress)`
            : 'XAUUSD_History_Update'}
        </button>
      </div>

      {tfStatsError && <div className="archive-result err">Error: {tfStatsError}</div>}

      {tfStats && (
        <div className="tf-stats-panel">
          <div className="tf-stats-meta muted small">
            Market: <strong className={tfStats.market_state === 'live' ? 'ok' : 'err'}>
              {tfStats.market_state}
            </strong>
            {' · queried at '}
            {tfStats.queried_at_utc.slice(11, 19)} UTC
          </div>
          <table className="tf-stats">
            <thead>
              <tr>
                <th>TF</th>
                <th className="right">Bars</th>
                <th>Oldest (UTC)</th>
                <th>Newest (UTC)</th>
                <th className="right">Coverage</th>
                <th className="right">Tail age</th>
              </tr>
            </thead>
            <tbody>
              {tfStats.timeframes.map(s => {
                const ageClass =
                  s.tail_age_secs == null   ? 'tail-empty' :
                  s.tail_age_secs < s.bar_secs * 2  ? 'tail-fresh' :
                  s.tail_age_secs < s.bar_secs * 10 ? 'tail-amber' :
                                                     'tail-stale'
                return (
                  <tr key={s.timeframe}>
                    <td><strong>{s.timeframe}</strong></td>
                    <td className="right">{s.rows.toLocaleString()}</td>
                    <td>{s.oldest ? s.oldest.replace('T', ' ') : '—'}</td>
                    <td>{s.newest ? s.newest.replace('T', ' ') : '—'}</td>
                    <td className="right">
                      {s.coverage_days != null
                        ? (s.coverage_days >= 365
                            ? `${(s.coverage_days / 365).toFixed(1)}y`
                            : `${s.coverage_days.toFixed(1)}d`)
                        : '—'}
                    </td>
                    <td className={`right ${ageClass}`}>
                      {s.tail_age_secs == null ? '—' : formatAge(s.tail_age_secs)}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}

      <h3 style={{ marginTop: 28 }}>Current fetching state</h3>
      <p className="muted">
        Live snapshot of the background history-backfill loop (auto-refreshed
        every 500 ms). Shows which timeframe it's currently walking, how many
        chunks have landed for that TF, and the cumulative bars across the
        whole run.
      </p>

      {backfill && (
        <div className="archive-result">
          <div>
            <strong>Status:</strong>{' '}
            <span className={
              backfill.status === 'running'  ? 'tail-amber' :
              backfill.status === 'complete' ? 'tail-fresh' :
              backfill.status === 'error'    ? 'tail-stale' : ''
            }>
              {backfill.status === 'running' && (backfill.current_tf
                ? `running — ${backfill.current_tf.toUpperCase()} (mode: ${backfill.current_mode ?? '?'})`
                : 'running — waiting to start next TF')}
              {backfill.status === 'complete' && '✓ complete'}
              {backfill.status === 'error' && `error: ${backfill.last_error ?? 'unknown'}`}
              {backfill.status === 'idle' && 'idle (no backfill in progress)'}
            </span>
          </div>

          {backfill.current_tf && (
            <>
              <div>
                <strong>Current TF chunks:</strong>{' '}
                {backfill.chunks_this_tf.toLocaleString()} chunks ·{' '}
                {backfill.bars_this_tf.toLocaleString()} bars
                {backfill.last_chunk_oldest_utc &&
                  ` · oldest bar so far: ${backfill.last_chunk_oldest_utc.replace('T', ' ')} UTC`}
              </div>
            </>
          )}

          <div>
            <strong>Overall progress:</strong>{' '}
            {backfill.tfs_completed.length + backfill.tfs_skipped.length} / {backfill.tfs_total} TFs
            {backfill.tfs_skipped.length > 0 &&
              ` (${backfill.tfs_skipped.length} skipped as already complete)`}
          </div>

          <div>
            <strong>Total bars added this run:</strong>{' '}
            {backfill.total_bars.toLocaleString()}
          </div>

          {backfill.tfs_completed.length > 0 && (
            <div className="muted small" style={{ marginTop: 4 }}>
              Completed: {backfill.tfs_completed.map(t => t.toUpperCase()).join(', ')}
            </div>
          )}
          {backfill.tfs_skipped.length > 0 && (
            <div className="muted small">
              Skipped: {backfill.tfs_skipped.map(t => t.toUpperCase()).join(', ')}
            </div>
          )}

          {backfill.started_at_utc && (
            <div className="muted small" style={{ marginTop: 6 }}>
              Started: {backfill.started_at_utc.slice(0, 19).replace('T', ' ')} UTC
              {backfill.completed_at_utc &&
                ` · Finished: ${backfill.completed_at_utc.slice(0, 19).replace('T', ' ')} UTC`}
            </div>
          )}
        </div>
      )}

      <h3 style={{ marginTop: 28 }}>Import disk archives → DB</h3>
      <p className="muted">
        Loads the on-disk per-day JSON files into the DuckDB tables the app reads:
        <code>ec_events_data/all/</code> → <code>xauusd_economic_calendar</code> and{' '}
        <code>news_data/all/</code> → <code>news_historical</code> (article bodies preserved).
        Use this after copying archive files from another branch/clone. Idempotent —
        upserts by primary key, never overwrites an existing news body with null.
      </p>

      <div className="archive-actions">
        <button className="btn" onClick={runImport} disabled={importBusy}>
          {importBusy ? 'Importing…' : 'Import_Disk_Archives'}
        </button>
      </div>

      {importError && <div className="archive-result err">Error: {importError}</div>}

      {importResult && (
        <div className="archive-result ok">
          <div>
            <strong>EC:</strong> {importResult.ec_rows.toLocaleString()} rows from{' '}
            {importResult.ec_files.toLocaleString()} files
          </div>
          <div>
            <strong>News:</strong> {importResult.news_rows.toLocaleString()} rows from{' '}
            {importResult.news_files.toLocaleString()} files{' '}
            ({importResult.news_bodies.toLocaleString()} with bodies)
          </div>
          {importResult.message && <div className="small">{importResult.message}</div>}
        </div>
      )}

      <h3 style={{ marginTop: 28 }}>News Archives</h3>
      <p className="muted">
        Detects the newest day file under <code>news_data/all/</code>, reads its latest
        article time, then fetches every newer article from FXStreet and (re)writes
        one JSON file per affected day up to today.
      </p>

      <div className="archive-actions">
        <button className="btn" onClick={runNewsUpdate} disabled={newsBusy}>
          {newsBusy ? 'Updating… (this can take a minute)' : 'News_Updates'}
        </button>
      </div>

      {newsError && <div className="archive-result err">Error: {newsError}</div>}

      {newsResult && (
        <div className={`archive-result ${newsResult.rate_limited ? 'err' : 'ok'}`}>
          <div>
            <strong>Last archive day:</strong> {newsResult.last_archive_day ?? '(none — first run)'}
          </div>
          <div>
            <strong>Cutoff used:</strong> {newsResult.cutoff}
          </div>
          <div>
            <strong>Articles fetched:</strong> {newsResult.articles_fetched}
          </div>
          <div>
            <strong>Bodies:</strong> {newsResult.bodies_fetched} fetched
            {newsResult.bodies_empty > 0 && `, ${newsResult.bodies_empty} empty`}
            {newsResult.bodies_failed > 0 && `, ${newsResult.bodies_failed} failed`}
          </div>
          <div>
            <strong>Day files written:</strong> {newsResult.days_written.length}
          </div>
          {newsResult.days_written.length > 0 && (
            <ul className="archive-days">
              {newsResult.days_written.map(([day, count]) => (
                <li key={day}><code>{day}.json</code> — {count} articles</li>
              ))}
            </ul>
          )}
          {newsResult.message && <div className="muted small">{newsResult.message}</div>}
        </div>
      )}

      <h3 style={{ marginTop: 28 }}>Gold EC Events</h3>
      <p className="muted">
        Creates <code>xauusd_economic_calendar</code> if it doesn't exist and walks
        FXStreet from 2009-01-01 → today, upserting every event for
        <strong> USD, EUR, GBP, JPY, CHF, AUD, CNY</strong> (all impact levels: low,
        medium, high). On a re-click, resumes from the last stored timestamp + 1 day.
        First run takes ~2 minutes; subsequent clicks are near-instant.
      </p>

      <div className="archive-actions">
        <button className="btn" onClick={runEcGoldUpdate} disabled={ecBusy || ecStoreBusy}>
          {ecBusy ? 'Updating… (first run takes ~2 min)' : 'EC_Gold_Events_Update'}
        </button>
        <button className="btn" onClick={runEcGoldStorage} disabled={ecBusy || ecStoreBusy}>
          {ecStoreBusy ? 'Writing files…' : 'EC_Gold_events_storage'}
        </button>
      </div>

      {ecBusy && ecProgress && (
        <div className="archive-result">
          <div>
            <strong>Chunk {ecProgress.chunks_done} / {ecProgress.chunks_total}</strong>
            {' — '}{ecProgress.current_chunk_start} → {ecProgress.current_chunk_end}
          </div>
          <div className="progress-bar">
            <div className="progress-fill" style={{ width: `${ecPct}%` }} />
            <div className="progress-label">{ecPct}%</div>
          </div>
          <div className="muted small">
            {ecProgress.rows_added_so_far.toLocaleString()} rows added · {ecProgress.elapsed_secs}s elapsed
            {ecEtaSecs != null && ` · ~${ecEtaSecs}s remaining`}
          </div>
        </div>
      )}

      {ecError && <div className="archive-result err">Error: {ecError}</div>}

      {ecResult && (
        <div className={`archive-result ${ecResult.error ? 'err' : 'ok'}`}>
          <div>
            <strong>Table existed before run:</strong> {ecResult.table_existed ? 'yes' : 'no — created this run'}
          </div>
          <div>
            <strong>Walked:</strong> {ecResult.walk_start} → {ecResult.walk_end}
          </div>
          <div>
            <strong>Chunks processed:</strong> {ecResult.chunks_processed}
            {' '}({(ecResult.duration_ms / 1000).toFixed(1)}s wallclock)
          </div>
          <div>
            <strong>Rows added this click:</strong> {ecResult.rows_added_this_call.toLocaleString()}
          </div>
          <div>
            <strong>Total rows in table:</strong> {ecResult.total_rows.toLocaleString()}
          </div>
          {ecResult.oldest_in_db && (
            <div>
              <strong>Coverage:</strong> {ecResult.oldest_in_db.slice(0, 10)} → {ecResult.newest_in_db?.slice(0, 10)}
            </div>
          )}
          {ecResult.error && <div className="muted small">⚠ Partial: {ecResult.error}</div>}
        </div>
      )}

      {ecStoreBusy && ecStoreProgress && (
        <div className="archive-result">
          <div>
            <strong>Writing file {ecStoreProgress.files_done} / {ecStoreProgress.files_total}</strong>
            {' — '}{ecStoreProgress.current_day}.json
          </div>
          <div className="progress-bar">
            <div className="progress-fill" style={{ width: `${ecStorePct}%` }} />
            <div className="progress-label">{ecStorePct}%</div>
          </div>
          <div className="muted small">
            {ecStoreProgress.events_written_so_far.toLocaleString()} events ·
            {' '}{ecStoreProgress.elapsed_secs}s elapsed
            {ecStoreEtaSecs != null && ecStoreEtaSecs > 0 && ` · ~${ecStoreEtaSecs}s remaining`}
          </div>
        </div>
      )}

      {ecStoreError && <div className="archive-result err">Error: {ecStoreError}</div>}

      {ecStoreResult && (
        <div className={`archive-result ${ecStoreResult.error ? 'err' : 'ok'}`}>
          <div>
            <strong>Mode:</strong> {ecStoreResult.incremental ? 'incremental update' : 'first run (full archive)'}
            {ecStoreResult.up_to_date && ' — all files already current ✓'}
          </div>
          {ecStoreResult.disk_latest_day_before && (
            <div>
              <strong>Latest day on disk before run:</strong> {ecStoreResult.disk_latest_day_before}
            </div>
          )}
          {ecStoreResult.db_latest_day && (
            <div>
              <strong>Latest day in DB:</strong> {ecStoreResult.db_latest_day}
            </div>
          )}
          <div>
            <strong>Files written this click:</strong> {ecStoreResult.files_written.toLocaleString()}
            {ecStoreResult.events_written > 0 &&
              ` (${ecStoreResult.events_written.toLocaleString()} events)`}
          </div>
          {ecStoreResult.incremental && (
            <div>
              <strong>Days already current (skipped):</strong> {ecStoreResult.days_already_current.toLocaleString()}
            </div>
          )}
          <div>
            <strong>Duration:</strong> {(ecStoreResult.duration_ms / 1000).toFixed(2)}s
          </div>
          <div>
            <strong>Archive root:</strong> <code>{ecStoreResult.archive_root}</code>
          </div>
          {ecStoreResult.error && <div className="muted small">⚠ Partial: {ecStoreResult.error}</div>}
        </div>
      )}
    </div>
  )
}

// Human-readable age formatter used by the Archives tab's TF-stats panel.
function formatAge(secs: number): string {
  if (secs < 0) return 'future?'
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  if (secs < 86400) {
    const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60)
    return `${h}h${m ? ` ${m}m` : ''}`
  }
  const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600)
  return `${d}d${h ? ` ${h}h` : ''}`
}


function CalendarView({ events, status }: { events: EcEvent[]; status: string }) {
  if (events.length === 0) {
    return <div className="placeholder">{status || 'Waiting for EC calendar data…'}</div>
  }
  return (
    <div className="ec-list">
      <div className="ec-status">{status}</div>
      <table className="ec-table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Cur</th>
            <th>Vol</th>
            <th>Event</th>
            <th>Actual</th>
            <th>Fcst</th>
            <th>Prev</th>
          </tr>
        </thead>
        <tbody>
          {events.map((e, i) => (
            <tr key={i} className={`vol-${Math.min(3, Math.max(0, e.volatility))}`}>
              <td>{formatTime(e.ts)}</td>
              <td>{e.currency}</td>
              <td><span className="vol-dot" /></td>
              <td className="ec-name" title={e.name}>{e.name}</td>
              <td>{fmtNum(e.actual)}</td>
              <td>{fmtNum(e.forecast)}</td>
              <td>{fmtNum(e.previous)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// MyFXBook calendar (today's events). Same column layout as CalendarView; the
// event name's tooltip shows the country. Source values are the raw display
// strings parsed from MyFXBook (numbers when parseable).
function MyFXBookView({ events, status }: { events: MfbEvent[]; status: string }) {
  if (events.length === 0) {
    return <div className="placeholder">{status || 'Waiting for MyFXBook calendar data…'}</div>
  }
  return (
    <div className="ec-list">
      <div className="ec-status">{status}</div>
      <table className="ec-table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Cur</th>
            <th>Vol</th>
            <th>Event</th>
            <th>Actual</th>
            <th>Fcst</th>
            <th>Prev</th>
          </tr>
        </thead>
        <tbody>
          {events.map((e, i) => (
            <tr key={i} className={`vol-${Math.min(3, Math.max(0, e.volatility))}`}>
              <td>{formatTime(e.ts)}</td>
              <td>{e.currency}</td>
              <td><span className="vol-dot" /></td>
              <td className="ec-name" title={`${e.name}${e.country ? ' · ' + e.country : ''}`}>{e.name}</td>
              <td>{fmtNum(e.actual)}</td>
              <td>{fmtNum(e.forecast)}</td>
              <td>{fmtNum(e.previous)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function NewsView({
  articles, status, onOpen,
}: { articles: NewsArticle[]; status: string; onOpen: (a: NewsArticle) => void }) {
  if (articles.length === 0) {
    return <div className="placeholder">{status || 'Waiting for news headlines…'}</div>
  }
  const now = new Date()
  const fmtAgo = (iso: string) => {
    try {
      const d = new Date(iso.endsWith('Z') ? iso : iso + 'Z')
      const mins = Math.floor((now.getTime() - d.getTime()) / 60000)
      if (mins < 1) return 'just now'
      if (mins < 60) return `${mins}m ago`
      const h = Math.floor(mins / 60), m = mins % 60
      return `${h}h${m}m ago`
    } catch { return '' }
  }
  const fmtTime = (iso: string) => {
    try {
      const d = new Date(iso.endsWith('Z') ? iso : iso + 'Z')
      return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    } catch { return iso.slice(11, 16) }
  }
  return (
    <div className="news-list">
      <div className="news-status">{status}</div>
      <div className="news-header">--- TODAY ({articles.length} articles) ---</div>
      <ul>
        {articles.map((a) => (
          <li key={a.article_id} className="news-item" onClick={() => onOpen(a)}>
            <span className="news-time">{fmtTime(a.published_utc)}</span>
            <span className="news-ago">({fmtAgo(a.published_utc)})</span>
            <span className="news-title">{a.title}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}

// MyFXBook news, grouped into the three categories (News / Analysis / Press
// Release). Each item links to its MyFXBook article and shows source + time.
function MyFXBookNewsView({ items, status, onOpen }: { items: MfbNewsItem[]; status: string; onOpen: (i: MfbNewsItem) => void }) {
  if (items.length === 0) {
    return <div className="placeholder">{status || 'Waiting for MyFXBook news…'}</div>
  }
  const fmtTime = (iso: string) => {
    try {
      const d = new Date(iso.endsWith('Z') ? iso : iso + 'Z')
      return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    } catch { return iso.slice(11, 16) }
  }
  const SECTIONS: { key: string; label: string }[] = [
    { key: 'news', label: 'News' },
    { key: 'analysis', label: 'Analysis' },
    { key: 'press-release', label: 'Press Release' },
  ]
  return (
    <div className="news-list">
      <div className="news-status">{status}</div>
      {SECTIONS.map(({ key, label }) => {
        const group = items.filter((i) => i.category === key)
        if (group.length === 0) return null
        return (
          <div key={key}>
            <div className="news-header">--- {label.toUpperCase()} ({group.length}) ---</div>
            <ul>
              {group.map((i) => (
                <li key={i.article_id} className="news-item" onClick={() => onOpen(i)} title={i.summary}>
                  <span className="news-time">{fmtTime(i.published_utc)}</span>
                  {i.source && <span className="news-ago">({i.source})</span>}
                  <span className="news-title">{i.title}</span>
                </li>
              ))}
            </ul>
          </div>
        )
      })}
    </div>
  )
}

// MyFXBook news card — fetches the full article body on demand (already
// backfilled into the DB; the command fills it in if missing). Same modal
// styling as the FXStreet ArticleModal.
function MyFXBookNewsModal({ item, onClose }: { item: MfbNewsItem; onClose: () => void }) {
  const [body, setBody] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  useEffect(() => {
    setLoading(true); setBody(null)
    invoke<string | null>('get_mfb_news_body', { articleId: item.article_id, url: item.url })
      .then((b) => setBody(b ?? ''))
      .catch(() => setBody(''))
      .finally(() => setLoading(false))
  }, [item.article_id, item.url])
  useEffect(() => {
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose])
  const date = new Date(item.published_utc.endsWith('Z') ? item.published_utc : item.published_utc + 'Z')
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <button className="modal-close" onClick={onClose} aria-label="Close">×</button>
        <h2 className="modal-title">{item.title}</h2>
        <div className="modal-meta">
          <span>{date.toLocaleString()}</span>
          {item.source && <span> · {item.source}</span>}
          {item.category && <span className="modal-tags"> · {item.category}</span>}
        </div>
        {item.summary && <div className="modal-summary">{item.summary}</div>}
        <div className="modal-body">
          {loading && <div className="muted">Fetching full article…</div>}
          {!loading && body === '' && <div className="muted">No body available — open the original for the full article.</div>}
          {body && body.length > 0 && body.split(/\n{2,}/).map((p, i) => <p key={i}>{p}</p>)}
        </div>
        {item.url && (
          <a className="modal-url" href={item.url} target="_blank" rel="noreferrer">
            Read full article on myfxbook.com ↗
          </a>
        )}
      </div>
    </div>
  )
}

// ForexFactory news — flat time-sorted list; clicking an item opens a card.
function FFNewsView({ items, status, onOpen }: { items: FfNewsItem[]; status: string; onOpen: (i: FfNewsItem) => void }) {
  if (items.length === 0) {
    return <div className="placeholder">{status || 'Waiting for ForexFactory news…'}</div>
  }
  const fmtTime = (iso: string) => {
    try {
      const d = new Date(iso.endsWith('Z') ? iso : iso + 'Z')
      return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    } catch { return iso.slice(11, 16) }
  }
  return (
    <div className="news-list">
      <div className="news-status">{status}</div>
      <ul>
        {items.map((i) => (
          <li key={i.article_id} className="news-item" onClick={() => onOpen(i)} title={i.preview}>
            <span className="news-time">{fmtTime(i.published_utc)}</span>
            {i.source && <span className="news-ago">({i.source})</span>}
            <span className="news-title">{i.title}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}

// Forex Factory news card — same modal styling as the FXStreet ArticleModal.
// FF aggregates and links out to the original source, so the card shows the
// preview plus a link to the full article rather than a fetched body.
function FFNewsModal({ item, onClose }: { item: FfNewsItem; onClose: () => void }) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose])
  const date = new Date(item.published_utc.endsWith('Z') ? item.published_utc : item.published_utc + 'Z')
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <button className="modal-close" onClick={onClose} aria-label="Close">×</button>
        <h2 className="modal-title">{item.title}</h2>
        <div className="modal-meta">
          <span>{date.toLocaleString()}</span>
          {item.source && <span> · {item.source}</span>}
        </div>
        <div className="modal-body">
          {item.preview
            ? item.preview.split(/\n{2,}/).map((p, i) => <p key={i}>{p}</p>)
            : <div className="muted">No preview available — open the original for the full article.</div>}
        </div>
        {item.url && (
          <a className="modal-url" href={item.url} target="_blank" rel="noreferrer">
            Read full article on forexfactory.com ↗
          </a>
        )}
      </div>
    </div>
  )
}

// ForexFactory calendar — same table layout as the FXStreet/MyFXBook views.
// actual/forecast/previous are raw display strings (e.g. "150K").
function FFCalendarView({ events, status }: { events: FfCalEvent[]; status: string }) {
  if (events.length === 0) {
    return <div className="placeholder">{status || 'Waiting for ForexFactory calendar…'}</div>
  }
  return (
    <div className="ec-list">
      <div className="ec-status">{status}</div>
      <table className="ec-table">
        <thead>
          <tr><th>Time</th><th>Cur</th><th>Vol</th><th>Event</th><th>Actual</th><th>Fcst</th><th>Prev</th></tr>
        </thead>
        <tbody>
          {events.map((e, i) => (
            <tr key={i} className={`vol-${Math.min(3, Math.max(0, e.volatility))}`}>
              <td>{formatTime(e.ts)}</td>
              <td>{e.currency}</td>
              <td><span className="vol-dot" /></td>
              <td className="ec-name" title={`${e.name}${e.country ? ' · ' + e.country : ''}`}>{e.name}</td>
              <td>{e.actual ?? '—'}</td>
              <td>{e.forecast ?? '—'}</td>
              <td>{e.previous ?? '—'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ── Market Predictor · Daily Analysis (news sentiment) ───────────────────────
type GoldSentiment = {
  disposition: string            // BULLISH | BEARISH | NEUTRAL
  intensity: number              // -100..100
  conviction: number             // 0..1
  headline: string
  summary: string
  mood: string[]
  drivers: { factor: string; lean: string; note: string }[]
  forward: { lean: string; trajectory: string; base_case: string; watch: string[] }
  updated_utc: string
  news_count: number
}

const dispColor = (d: string) =>
  d === 'BULLISH' ? '#2dd47b' : d === 'BEARISH' ? '#f87171' : '#cbb26b'
const leanColor = (l: string) =>
  l === 'BUY' || l === 'BULLISH' ? '#2dd47b' :
  l === 'SELL' || l === 'BEARISH' ? '#f87171' :
  l === 'COILED' ? '#d8a657' : '#9ca3af'

// Elapsed timer (seconds) while a slow opus call runs. No real progress signal
// from the CLI, so the bar is an honest estimate; this returns the true elapsed.
function useElapsed(active: boolean) {
  const [elapsed, setElapsed] = useState(0)
  useEffect(() => {
    if (!active) { setElapsed(0); return }
    const start = Date.now()
    const id = window.setInterval(() => setElapsed((Date.now() - start) / 1000), 250)
    return () => window.clearInterval(id)
  }, [active])
  return elapsed
}
const estProgress = (elapsed: number) => Math.min(96, Math.round((1 - Math.exp(-elapsed / 75)) * 130))

function LoadingBar({ elapsed, label }: { elapsed: number; label: string }) {
  return (
    <div className="sent-loading">
      <div className="progress-bar"><div className="progress-fill" style={{ width: `${estProgress(elapsed)}%` }} /></div>
      <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>{label} {elapsed.toFixed(0)}s</div>
    </div>
  )
}

function DailyAnalysisView() {
  // News sentiment
  const [sent, setSent] = useState<GoldSentiment | null>(null)
  const [sLoading, setSLoading] = useState(false)
  const [sError, setSError] = useState<string | null>(null)
  const sElapsed = useElapsed(sLoading)
  // EC events
  const [ec, setEc] = useState<GoldEc | null>(null)
  const [eLoading, setELoading] = useState(false)
  const [eError, setEError] = useState<string | null>(null)
  const eElapsed = useElapsed(eLoading)

  // Load the last stored results so the panels persist until the next click.
  useEffect(() => {
    invoke<GoldSentiment | null>('get_last_gold_sentiment').then((s) => { if (s) setSent(s) }).catch(() => {})
    invoke<GoldEc | null>('get_last_gold_ec_analysis').then((s) => { if (s) setEc(s) }).catch(() => {})
  }, [])

  const runSent = async () => {
    setSLoading(true); setSError(null)
    try { setSent(await invoke<GoldSentiment>('get_gold_sentiment')) }
    catch (e) { setSError(String(e)) }
    finally { setSLoading(false) }
  }
  const runEc = async () => {
    setELoading(true); setEError(null)
    try { setEc(await invoke<GoldEc>('get_gold_ec_analysis')) }
    catch (e) { setEError(String(e)) }
    finally { setELoading(false) }
  }

  return (
    <div>
      <div className="archive-actions">
        <button className="btn" onClick={runSent} disabled={sLoading}>
          {sLoading ? 'Analyzing…' : "Today’s Sentiment"}
        </button>
        <button className="btn" onClick={runEc} disabled={eLoading}>
          {eLoading ? 'Analyzing…' : "Today’s EC Events"}
        </button>
      </div>

      {sLoading && <LoadingBar elapsed={sElapsed} label="Reading today’s news with opus…" />}
      {sError && <div className="archive-result err">Error: {sError}</div>}
      {sent
        ? <GoldSentimentPanel s={sent} />
        : !sLoading && !sError && <div className="placeholder">Click “Today’s Sentiment” to read the market from today’s news.</div>}

      {eLoading && <LoadingBar elapsed={eElapsed} label="Reading today’s economic calendar with opus…" />}
      {eError && <div className="archive-result err">Error: {eError}</div>}
      {ec && <GoldEcPanel s={ec} />}
    </div>
  )
}

function GoldSentimentPanel({ s }: { s: GoldSentiment }) {
  const pct = Math.max(-100, Math.min(100, s.intensity))
  const markerLeft = (pct + 100) / 2 // 0..100
  const updated = (() => {
    try { return new Date(s.updated_utc).toLocaleString() } catch { return s.updated_utc }
  })()
  return (
    <div className="sent-panel">
      {/* Headline disposition */}
      <div className="sent-head">
        <span className="sent-badge" style={{ background: dispColor(s.disposition) }}>{s.disposition}</span>
        <span className="sent-intensity" style={{ color: dispColor(s.disposition) }}>
          {pct > 0 ? '+' : ''}{pct}
        </span>
        <span className="sent-conviction muted">conviction {Math.round((s.conviction ?? 0) * 100)}%</span>
      </div>

      {/* Gauge */}
      <div className="sent-gauge">
        <div className="sent-gauge-track">
          <div className="sent-gauge-mid" />
          <div className="sent-gauge-marker" style={{ left: `${markerLeft}%`, background: dispColor(s.disposition) }} />
        </div>
        <div className="sent-gauge-labels"><span>Bearish</span><span>Neutral</span><span>Bullish</span></div>
      </div>

      {s.headline && <div className="sent-headline">{s.headline}</div>}
      {s.summary && <div className="sent-summary">{s.summary}</div>}

      {s.mood?.length > 0 && (
        <div className="sent-chips">{s.mood.map((m, i) => <span key={i} className="sent-chip">{m}</span>)}</div>
      )}

      {s.drivers?.length > 0 && (
        <div className="sent-section">
          <div className="sent-section-title">Drivers</div>
          <ul className="sent-drivers">
            {s.drivers.map((d, i) => (
              <li key={i}>
                <span className="sent-lean" style={{ color: leanColor(d.lean) }}>{d.lean}</span>
                <strong>{d.factor}</strong>{d.note ? <span className="muted"> — {d.note}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      )}

      {s.forward && (
        <div className="sent-forward">
          <div className="sent-section-title">
            Forward outlook ·{' '}
            <span style={{ color: leanColor(s.forward.lean) }}>{s.forward.lean}</span>
            {s.forward.trajectory && <span className="muted"> · {s.forward.trajectory.toLowerCase()}</span>}
          </div>
          {s.forward.base_case && <div className="sent-summary">{s.forward.base_case}</div>}
          {s.forward.watch?.length > 0 && (
            <div className="sent-watch muted">Watch: {s.forward.watch.join(' · ')}</div>
          )}
        </div>
      )}

      <div className="sent-foot muted">
        Updated {updated} · {s.news_count} news items
      </div>
    </div>
  )
}

// ── Market Predictor · Today's EC Events (economic-calendar impact) ──────────
type GoldEc = {
  bias: string                   // BULLISH | BEARISH | NEUTRAL
  intensity: number              // -100..100
  conviction: number
  headline: string
  summary: string
  released: { event: string; currency: string; surprise: string; gold_impact: string; note: string }[]
  upcoming: { time: string; currency: string; importance: string; event: string; why: string }[]
  forward: { base_case: string; watch: string[] }
  updated_utc: string
  events_count: number
}

function GoldEcPanel({ s }: { s: GoldEc }) {
  const pct = Math.max(-100, Math.min(100, s.intensity))
  const markerLeft = (pct + 100) / 2
  const updated = (() => { try { return new Date(s.updated_utc).toLocaleString() } catch { return s.updated_utc } })()
  return (
    <div className="sent-panel">
      <div className="sent-head">
        <span className="sent-badge" style={{ background: dispColor(s.bias) }}>{s.bias}</span>
        <span className="sent-intensity" style={{ color: dispColor(s.bias) }}>{pct > 0 ? '+' : ''}{pct}</span>
        <span className="sent-conviction muted">conviction {Math.round((s.conviction ?? 0) * 100)}%</span>
      </div>
      <div className="sent-gauge">
        <div className="sent-gauge-track">
          <div className="sent-gauge-mid" />
          <div className="sent-gauge-marker" style={{ left: `${markerLeft}%`, background: dispColor(s.bias) }} />
        </div>
        <div className="sent-gauge-labels"><span>Bearish</span><span>Neutral</span><span>Bullish</span></div>
      </div>

      {s.headline && <div className="sent-headline">{s.headline}</div>}
      {s.summary && <div className="sent-summary">{s.summary}</div>}

      {s.released?.length > 0 && (
        <div className="sent-section">
          <div className="sent-section-title">Released</div>
          <ul className="sent-drivers">
            {s.released.map((r, i) => (
              <li key={i}>
                <span className="sent-lean" style={{ color: leanColor(r.gold_impact) }}>{r.gold_impact}</span>
                <strong>{r.currency} {r.event}</strong>
                {r.surprise && <span className="muted"> · {r.surprise.toLowerCase()}</span>}
                {r.note ? <span className="muted"> — {r.note}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      )}

      {s.upcoming?.length > 0 && (
        <div className="sent-section">
          <div className="sent-section-title">Upcoming today</div>
          <ul className="sent-drivers">
            {s.upcoming.map((u, i) => (
              <li key={i}>
                <span className="sent-lean muted">{u.time}</span>
                <strong>{u.currency} {u.event}</strong>
                {u.importance && <span className="muted"> · {u.importance.toLowerCase()}</span>}
                {u.why ? <span className="muted"> — {u.why}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      )}

      {s.forward && (s.forward.base_case || s.forward.watch?.length > 0) && (
        <div className="sent-forward">
          <div className="sent-section-title">Forward</div>
          {s.forward.base_case && <div className="sent-summary">{s.forward.base_case}</div>}
          {s.forward.watch?.length > 0 && <div className="sent-watch muted">Watch: {s.forward.watch.join(' · ')}</div>}
        </div>
      )}

      <div className="sent-foot muted">Updated {updated} · {s.events_count} events</div>
    </div>
  )
}

// ── Market Predictor · Current Week & Next Week (weekly EC outlook) ──────────
type GoldWeek = {
  bias: string
  intensity: number
  conviction: number
  headline: string
  summary: string
  happened: { when: string; currency: string; event: string; surprise: string; gold_impact: string; note: string }[]
  upcoming: { when: string; currency: string; event: string; importance: string; why: string }[]
  forward: { base_case: string; watch: string[] }
  updated_utc: string
  events_count: number
}

function WeeklyAnalysisView() {
  const [wk, setWk] = useState<GoldWeek | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const elapsed = useElapsed(loading)

  useEffect(() => {
    invoke<GoldWeek | null>('get_last_gold_week_analysis').then((s) => { if (s) setWk(s) }).catch(() => {})
  }, [])

  const run = async () => {
    setLoading(true); setError(null)
    try { setWk(await invoke<GoldWeek>('get_gold_week_analysis')) }
    catch (e) { setError(String(e)) }
    finally { setLoading(false) }
  }

  return (
    <div>
      <div className="archive-actions">
        <button className="btn" onClick={run} disabled={loading}>
          {loading ? 'Analyzing…' : 'This Week'}
        </button>
      </div>
      {loading && <LoadingBar elapsed={elapsed} label="Reading this & next week’s calendar with opus…" />}
      {error && <div className="archive-result err">Error: {error}</div>}
      {wk
        ? <WeeklyEcPanel s={wk} />
        : !loading && !error && <div className="placeholder">Click “This Week” for the week’s gold outlook — what happened and what’s coming.</div>}
    </div>
  )
}

function WeeklyEcPanel({ s }: { s: GoldWeek }) {
  const pct = Math.max(-100, Math.min(100, s.intensity))
  const markerLeft = (pct + 100) / 2
  const updated = (() => { try { return new Date(s.updated_utc).toLocaleString() } catch { return s.updated_utc } })()
  return (
    <div className="sent-panel">
      <div className="sent-head">
        <span className="sent-badge" style={{ background: dispColor(s.bias) }}>{s.bias}</span>
        <span className="sent-intensity" style={{ color: dispColor(s.bias) }}>{pct > 0 ? '+' : ''}{pct}</span>
        <span className="sent-conviction muted">conviction {Math.round((s.conviction ?? 0) * 100)}%</span>
      </div>
      <div className="sent-gauge">
        <div className="sent-gauge-track">
          <div className="sent-gauge-mid" />
          <div className="sent-gauge-marker" style={{ left: `${markerLeft}%`, background: dispColor(s.bias) }} />
        </div>
        <div className="sent-gauge-labels"><span>Bearish</span><span>Neutral</span><span>Bullish</span></div>
      </div>

      {s.headline && <div className="sent-headline">{s.headline}</div>}
      {s.summary && <div className="sent-summary">{s.summary}</div>}

      {s.happened?.length > 0 && (
        <div className="sent-section">
          <div className="sent-section-title">Happened this week</div>
          <ul className="sent-drivers">
            {s.happened.map((h, i) => (
              <li key={i}>
                <span className="sent-lean" style={{ color: leanColor(h.gold_impact) }}>{h.gold_impact}</span>
                <span className="muted">{h.when} · </span>
                <strong>{h.currency} {h.event}</strong>
                {h.surprise && <span className="muted"> · {h.surprise.toLowerCase()}</span>}
                {h.note ? <span className="muted"> — {h.note}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      )}

      {s.upcoming?.length > 0 && (
        <div className="sent-section">
          <div className="sent-section-title">Coming up (this & next week)</div>
          <ul className="sent-drivers">
            {s.upcoming.map((u, i) => (
              <li key={i}>
                <span className="sent-lean muted" style={{ minWidth: 90 }}>{u.when}</span>
                <strong>{u.currency} {u.event}</strong>
                {u.importance && <span className="muted"> · {u.importance.toLowerCase()}</span>}
                {u.why ? <span className="muted"> — {u.why}</span> : null}
              </li>
            ))}
          </ul>
        </div>
      )}

      {s.forward && (s.forward.base_case || s.forward.watch?.length > 0) && (
        <div className="sent-forward">
          <div className="sent-section-title">Forward</div>
          {s.forward.base_case && <div className="sent-summary">{s.forward.base_case}</div>}
          {s.forward.watch?.length > 0 && <div className="sent-watch muted">Watch: {s.forward.watch.join(' · ')}</div>}
        </div>
      )}

      <div className="sent-foot muted">Updated {updated} · {s.events_count} events</div>
    </div>
  )
}

function ArticleModal({ article, onClose }: { article: NewsArticle; onClose: () => void }) {
  const [body, setBody] = useState<string | null>(article.body ?? null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setBody(article.body ?? null)
    setError(null)
    // Auto-fetch on open if body is missing
    if (!article.body) {
      setLoading(true)
      invoke<{ body: string | null; status: string; message: string | null }>(
        'fetch_article_body_on_demand', { articleId: article.article_id }
      ).then((r) => {
        if (r.status === 'ok' && r.body) setBody(r.body)
        else if (r.status === 'empty') setBody('')
        else setError(r.message || `Fetch ${r.status}`)
      }).catch((e) => setError(String(e)))
        .finally(() => setLoading(false))
    }
  }, [article.article_id])

  // Close on Esc
  useEffect(() => {
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose])

  const date = new Date(article.published_utc.endsWith('Z') ? article.published_utc : article.published_utc + 'Z')
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <button className="modal-close" onClick={onClose} aria-label="Close">×</button>
        <h2 className="modal-title">{article.title}</h2>
        <div className="modal-meta">
          <span>{date.toLocaleString()}</span>
          {article.author && <span> · {article.author}</span>}
          {article.tags && <span className="modal-tags"> · {article.tags}</span>}
        </div>
        {article.summary && (
          <div className="modal-summary">{article.summary}</div>
        )}
        <div className="modal-body">
          {loading && <div className="muted">Fetching full article…</div>}
          {error && <div className="err">{error}</div>}
          {body === '' && !loading && !error && (
            <div className="muted">No body available — FXStreet didn't publish text for this article (title-only data flash).</div>
          )}
          {body && body.length > 0 && body.split(/\n{2,}/).map((p, i) => (
            <p key={i}>{p}</p>
          ))}
        </div>
        {article.url && (
          <a className="modal-url" href={article.url} target="_blank" rel="noreferrer">
            Open original on fxstreet.com ↗
          </a>
        )}
      </div>
    </div>
  )
}

// One button per model. Each pulls a fresh live snapshot and runs only that
// model on demand (no fan-out), so a slow model never blocks the others.
const PROVIDER_BLURB: Record<string, string> = {
  'Claude': 'multi-strategy',
  'Gemini': 'VWAP+8EMA',
  'Claude Blitz': '1m scalper',
}

function TradeIdeasView() {
  const [open, setOpen] = useState(false)
  const [results, setResults] = useState<Record<string, ModelTradeIdea | null>>({})
  const [running, setRunning] = useState<Record<string, boolean>>({})

  const runOne = async (provider: string) => {
    setResults(prev => ({ ...prev, [provider]: null }))   // show that card in loading state
    setOpen(true)
    setRunning(prev => ({ ...prev, [provider]: true }))
    try {
      const r = await invoke<ModelTradeIdea>('get_gold_trade_idea', { provider })
      setResults(prev => ({ ...prev, [provider]: r }))
    } catch (err) {
      setResults(prev => ({
        ...prev,
        [provider]: {
          provider, model: '', ok: false, bias: null, strategy: null,
          entry_low: null, entry_high: null, stop: null, target1: null, target2: null,
          rationale: null, error: String(err), duration_ms: 0,
        },
      }))
    } finally {
      setRunning(prev => ({ ...prev, [provider]: false }))
    }
  }

  const closeOne = (p: string) => setResults(prev => {
    const next = { ...prev }
    delete next[p]
    return next
  })
  const closeAll = () => { setOpen(false); setResults({}) }

  return (
    <div className="archive">
      <h3>XAUUSD Trade Ideas</h3>
      <p className="muted">
        Pull a single live setup from any model on demand — each button fetches a
        fresh snapshot and runs just that model (bias · entry · SL · TP).
        Read-only.
      </p>
      <div className="archive-actions">
        {TRADER_PROVIDERS.map(p => (
          <button key={p} className="btn" onClick={() => runOne(p)} disabled={running[p]}>
            {running[p] ? `${p}…` : `${p} · ${PROVIDER_BLURB[p] ?? ''}`}
          </button>
        ))}
      </div>
      {open && (
        <MultiModelPopups results={results} onCloseOne={closeOne} onCloseAll={closeAll} />
      )}
    </div>
  )
}

function MultiModelPopups({ results, onCloseOne, onCloseAll }: {
  results: Record<string, ModelTradeIdea | null>
  onCloseOne: (p: string) => void
  onCloseAll: () => void
}) {
  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === 'Escape') onCloseAll() }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onCloseAll])

  const shown = TRADER_PROVIDERS.filter(p => p in results)
  if (shown.length === 0) return null

  return (
    <div className="popups-backdrop" onClick={onCloseAll}>
      <div className="popups-row" onClick={e => e.stopPropagation()}>
        {shown.map((p, i) => (
          <ModelPopupCard key={p} provider={p} idea={results[p] ?? null} index={i} onClose={() => onCloseOne(p)} />
        ))}
      </div>
    </div>
  )
}

function ModelPopupCard({ provider, idea, onClose }: {
  provider: string; idea: ModelTradeIdea | null; index: number; onClose: () => void
}) {
  const [qty, setQty] = useState(1)
  const [confirming, setConfirming] = useState(false)
  const [placing, setPlacing] = useState(false)
  const [order, setOrder] = useState<OrderResult | null>(null)
  // Editable order levels — seeded from the idea, then the trader can adjust.
  const [entryPx, setEntryPx] = useState<number | null>(null)
  const [slPx, setSlPx] = useState<number | null>(null)
  const [tpPx, setTpPx] = useState<number | null>(null)

  const loading = idea == null
  const bias = (idea?.bias || '').toUpperCase()
  const tradeable = bias === 'LONG' || bias === 'SHORT'
  const side = bias === 'LONG' ? 'BUY' : 'SELL'
  const sideClass = side === 'BUY' ? 'value-gain' : 'value-loss'
  const r2 = (n: number) => Math.round(n * 100) / 100
  // The idea's original entry (zone midpoint) — anchor for SL/TP shifting.
  const origEntry = idea && idea.entry_low != null && idea.entry_high != null
    ? (idea.entry_low + idea.entry_high) / 2
    : (idea?.entry_low ?? idea?.entry_high ?? null)
  const entryZone = idea && (idea.entry_low != null || idea.entry_high != null)
    ? (idea.entry_low != null && idea.entry_high != null && idea.entry_low !== idea.entry_high
        ? `${fmtNum(idea.entry_low)} – ${fmtNum(idea.entry_high)}`
        : fmtNum(idea.entry_low ?? idea.entry_high))
    : '—'

  // Seed the editable levels once the model's setup arrives.
  useEffect(() => {
    if (!idea) return
    setEntryPx(origEntry != null ? r2(origEntry) : null)
    setSlPx(idea.stop)
    setTpPx(idea.target1)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idea])

  // Moving the entry shifts SL & TP by the same delta (keeps the idea's R:R).
  const onEntryChange = (v: number) => {
    setEntryPx(v)
    if (origEntry != null && Number.isFinite(v)) {
      const delta = v - origEntry
      if (idea?.stop != null) setSlPx(r2(idea.stop + delta))
      if (idea?.target1 != null) setTpPx(r2(idea.target1 + delta))
    }
  }

  const confirmOrder = async () => {
    setPlacing(true)
    try {
      const r = await invoke<OrderResult>('place_gold_order', {
        side: bias, oz: qty, entry: entryPx, stop: slPx, target1: tpPx,
      })
      setOrder(r)
    } catch (e) {
      setOrder({
        sent: false, side, symbol: 'XAUUSD', oz: qty, ctrader_volume: 0,
        order_type: 'MARKET', entry: null, sl: null, tp: null, status: null, error: String(e),
      })
    } finally {
      setPlacing(false)
      setConfirming(false)
    }
  }

  return (
    <div className="model-popup">
      <div className="model-popup-head">
        <span className="model-popup-name">{provider}</span>
        {idea && idea.model && <span className="model-popup-model">{idea.model}</span>}
        <button className="modal-close sm" onClick={onClose} aria-label="Close">×</button>
      </div>
      {loading ? (
        <div className="model-popup-loading"><div className="spinner sm" /><span>thinking…</span></div>
      ) : idea.ok ? (
        <>
          <div className="model-popup-bias">
            <span className={`bias-badge ${biasClassOf(bias)}`}>{bias || 'FLAT'}</span>
            {idea.strategy && <span className="strategy-pill">{idea.strategy}</span>}
          </div>
          <div className="model-levels">
            <div className="lvl"><span className="lvl-k">Entry</span><span className="lvl-v">{entryZone}</span></div>
            <div className="lvl"><span className="lvl-k">SL</span><span className="lvl-v value-loss">{fmtNum(idea.stop)}</span></div>
            <div className="lvl"><span className="lvl-k">TP1</span><span className="lvl-v value-gain">{fmtNum(idea.target1)}</span></div>
            <div className="lvl"><span className="lvl-k">TP2</span><span className="lvl-v value-gain">{fmtNum(idea.target2)}</span></div>
          </div>
          {idea.rationale && <div className="model-popup-rationale">{idea.rationale}</div>}

          {tradeable && (
            <div className="model-order">
              {order ? (
                <div className="order-result">
                  <div className={order.sent ? 'value-gain' : 'err'}>
                    {order.sent ? `✓ order placed${order.status ? ' · ' + order.status : ''}` : '✗ not placed'}
                  </div>
                  <div className="small">
                    <span className={sideClass}>{order.side}</span> {order.oz} oz · vol {order.ctrader_volume} · {order.order_type}
                    {order.entry != null && <> @ {fmtNum(order.entry)}</>}
                  </div>
                  {(order.sl != null || order.tp != null) && (
                    <div className="small">SL {fmtNum(order.sl)} · TP {fmtNum(order.tp)}</div>
                  )}
                  {order.error && <div className="err small">{order.error}</div>}
                  <button className="btn-sm" onClick={() => setOrder(null)}>New order</button>
                </div>
              ) : confirming ? (
                <div className="order-confirm">
                  <div className="small">
                    ⚠ <strong>LIVE</strong> order — <strong className={sideClass}>{side}</strong> {qty} oz XAUUSD
                    {entryPx != null ? <> @ entry {fmtNum(entryPx)} (pending)</> : <> at market</>}
                    {slPx != null && <> · SL {fmtNum(slPx)}</>}
                    {tpPx != null && <> · TP {fmtNum(tpPx)}</>}
                  </div>
                  <div className="order-confirm-actions">
                    <button className="btn-sm btn-go" onClick={confirmOrder} disabled={placing}>
                      {placing ? 'Placing…' : 'Confirm & send'}
                    </button>
                    <button className="btn-sm" onClick={() => setConfirming(false)} disabled={placing}>Cancel</button>
                  </div>
                </div>
              ) : (
                <div className="order-place">
                  <div className="order-fields">
                    <label className="order-fld small">
                      Entry
                      <input type="number" step="0.01" value={entryPx ?? ''}
                        onChange={e => onEntryChange(parseFloat(e.target.value))} />
                    </label>
                    <label className="order-fld small">
                      SL
                      <input type="number" step="0.01" value={slPx ?? ''}
                        onChange={e => setSlPx(e.target.value === '' ? null : parseFloat(e.target.value))} />
                    </label>
                    <label className="order-fld small">
                      TP
                      <input type="number" step="0.01" value={tpPx ?? ''}
                        onChange={e => setTpPx(e.target.value === '' ? null : parseFloat(e.target.value))} />
                    </label>
                    <label className="order-fld small">
                      Qty (oz)
                      <select value={qty} onChange={e => setQty(Number(e.target.value))}>
                        {Array.from({ length: 10 }, (_, i) => i + 1).map(n => (
                          <option key={n} value={n}>{n}</option>
                        ))}
                      </select>
                    </label>
                  </div>
                  <button className={`btn-sm order-go ${side === 'BUY' ? 'btn-buy' : 'btn-sell'}`} onClick={() => setConfirming(true)}>
                    {side} {qty} oz {entryPx != null ? `@ ${fmtNum(entryPx)}` : 'market'}
                  </button>
                </div>
              )}
            </div>
          )}

          <div className="model-popup-foot muted small">{(idea.duration_ms / 1000).toFixed(1)}s</div>
        </>
      ) : (
        <div className="model-popup-error">
          <div className="err">⚠ {idea.error ?? 'failed'}</div>
          <div className="muted small">{(idea.duration_ms / 1000).toFixed(1)}s</div>
        </div>
      )}
    </div>
  )
}

function biasClassOf(b: string): string {
  const x = (b || '').toUpperCase()
  return x === 'LONG' ? 'bias-long' : x === 'SHORT' ? 'bias-short' : 'bias-flat'
}

// Live positions + pending orders, fed by the cTrader reconcile/execution stream.
// P/L is computed client-side from the current bid so it ticks in real time.
function PositionsView({ positions, orders, tick, auto }: {
  positions: OpenPosition[]; orders: PendingOrder[]; tick: Tick | null; auto: AutoState
}) {
  const px = tick?.bid ?? null
  const [autoOz, setAutoOz] = useState(1)
  const setAutoTrade = (enabled: boolean, oz: number) => {
    invoke('set_auto_trade', { enabled, oz }).catch(() => {})
  }
  const pnlOf = (p: OpenPosition): number | null => {
    if (px == null || p.entry == null) return null
    return (px - p.entry) * p.oz * (p.side === 'BUY' ? 1 : -1)
  }
  // Live distance from current price to a pending order's entry, in price units.
  const distOf = (o: PendingOrder): number | null => {
    if (px == null || o.price == null) return null
    return Math.abs(px - o.price)
  }

  // "Is this order still relevant?" review flow.
  const [reviewOrder, setReviewOrder] = useState<PendingOrder | null>(null)
  const [review, setReview] = useState<OrderReview | null>(null)
  const [reviewBusy, setReviewBusy] = useState(false)
  const [cancelBusy, setCancelBusy] = useState(false)
  const closeReview = () => { setReviewOrder(null); setReview(null); setReviewBusy(false); setCancelBusy(false) }
  const startReview = async (o: PendingOrder) => {
    setReviewOrder(o); setReview(null); setReviewBusy(true)
    try {
      const r = await invoke<OrderReview>('review_pending_order', {
        orderId: o.id, side: o.side, entry: o.price, stop: o.sl, target1: o.tp,
      })
      setReview(r)
    } catch (e) {
      setReview({ ok: false, recommendation: null, confidence: null, reason: null, error: String(e), duration_ms: 0 })
    } finally { setReviewBusy(false) }
  }
  const doCancel = async () => {
    if (!reviewOrder) return
    setCancelBusy(true)
    try {
      await invoke<string>('cancel_order', { orderId: reviewOrder.id })
      closeReview()
    } catch (e) {
      setReview(r => r ? { ...r, error: 'Cancel failed: ' + String(e) } : r)
      setCancelBusy(false)
    }
  }

  // Open-position analysis flow (HOLD / ADJUST SL-TP / CLOSE).
  const [analyzePos, setAnalyzePos] = useState<OpenPosition | null>(null)
  const [posReview, setPosReview] = useState<PositionReview | null>(null)
  const [posBusy, setPosBusy] = useState(false)
  const [actionBusy, setActionBusy] = useState(false)
  const closeAnalyze = () => { setAnalyzePos(null); setPosReview(null); setPosBusy(false); setActionBusy(false) }
  const startAnalyze = async (p: OpenPosition) => {
    setAnalyzePos(p); setPosReview(null); setPosBusy(true)
    try {
      const r = await invoke<PositionReview>('review_position', {
        positionId: p.id, side: p.side, entry: p.entry, oz: p.oz, stop: p.sl, target1: p.tp,
      })
      setPosReview(r)
    } catch (e) {
      setPosReview({ ok: false, action: null, new_sl: null, new_tp: null, confidence: null, reason: null, error: String(e), duration_ms: 0 })
    } finally { setPosBusy(false) }
  }
  const doClosePosition = async () => {
    if (!analyzePos) return
    setActionBusy(true)
    try {
      await invoke<string>('close_position', { positionId: analyzePos.id, oz: analyzePos.oz })
      closeAnalyze()
    } catch (e) {
      setPosReview(r => r ? { ...r, error: 'Close failed: ' + String(e) } : r)
      setActionBusy(false)
    }
  }
  const doApplySltp = async () => {
    if (!analyzePos || !posReview) return
    setActionBusy(true)
    try {
      await invoke<string>('amend_position_sltp', {
        positionId: analyzePos.id,
        stopLoss: posReview.new_sl ?? analyzePos.sl,
        takeProfit: posReview.new_tp ?? analyzePos.tp,
      })
      closeAnalyze()
    } catch (e) {
      setPosReview(r => r ? { ...r, error: 'Amend failed: ' + String(e) } : r)
      setActionBusy(false)
    }
  }
  const actionBadge = (a: string | null) =>
    a === 'CLOSE' ? 'bias-short' : a === 'ADJUST' ? 'bias-flat' : 'bias-long'
  const sideCls = (s: string) => s === 'BUY' ? 'value-gain' : 'value-loss'
  const empty = positions.length === 0 && orders.length === 0
  return (
    <div className="positions-view">
      <div className={`auto-panel ${auto.enabled ? 'on' : ''}`}>
        <div className="auto-row">
          <label className="auto-toggle">
            <input type="checkbox" checked={auto.enabled} onChange={e => setAutoTrade(e.target.checked, autoOz)} />
            <span>Auto-trade <strong>{auto.enabled ? 'ON' : 'OFF'}</strong></span>
          </label>
          <label className="order-fld small">
            Size (oz)
            <select value={autoOz} onChange={e => { const v = Number(e.target.value); setAutoOz(v); if (auto.enabled) setAutoTrade(true, v) }}>
              {Array.from({ length: 10 }, (_, i) => i + 1).map(n => <option key={n} value={n}>{n}</option>)}
            </select>
          </label>
          <span className="auto-status small">{auto.status}</span>
        </div>
        <p className="muted small">
          Keeps one XAUUSD position at a time: when flat, asks Claude for a setup and places a
          {' '}{autoOz}-oz pending order at its entry; cancels orders unfilled &gt;15 min; regenerates after each
          position closes. <strong>Live money.</strong>
        </p>
      </div>

      {empty && <div className="muted small" style={{ margin: '8px 2px' }}>No open positions or pending orders.</div>}

      <h3>Open positions <span className="muted small">({positions.length})</span></h3>
      {positions.length === 0 ? <div className="muted small">none</div> : (
        <table className="positions-table">
          <thead><tr>
            <th>Symbol</th><th>Side</th><th>Oz</th><th>Entry</th><th>SL</th><th>TP</th><th>P/L (USD)</th><th></th>
          </tr></thead>
          <tbody>
            {positions.map(p => {
              const pnl = pnlOf(p)
              return (
                <tr key={p.id}>
                  <td>{p.symbol}</td>
                  <td><span className={sideCls(p.side)}>{p.side}</span></td>
                  <td>{p.oz}</td>
                  <td>{fmtNum(p.entry)}</td>
                  <td className="value-loss">{fmtNum(p.sl)}</td>
                  <td className="value-gain">{fmtNum(p.tp)}</td>
                  <td className={pnl == null ? '' : pnl >= 0 ? 'value-gain' : 'value-loss'}>
                    {pnl == null ? '—' : `${pnl >= 0 ? '+' : ''}${pnl.toFixed(2)}`}
                  </td>
                  <td><button className="btn-sm" onClick={() => startAnalyze(p)}>Analyze</button></td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}

      {analyzePos && (
        <div className="modal-backdrop" onClick={actionBusy ? undefined : closeAnalyze}>
          <div className="modal review-modal" onClick={e => e.stopPropagation()}>
            <div className="model-popup-head">
              <span className="model-popup-name">Analyze position #{analyzePos.id}</span>
              <button className="modal-close sm" onClick={closeAnalyze} aria-label="Close">×</button>
            </div>
            <div className="review-body">
              <div className="small">
                <span className={sideCls(analyzePos.side)}>{analyzePos.side}</span> {analyzePos.oz} oz
                {' '}· entry {fmtNum(analyzePos.entry)} · now {px == null ? '—' : px.toFixed(2)}
                {(() => { const pl = pnlOf(analyzePos); return pl == null ? null : (
                  <> · P/L <span className={pl >= 0 ? 'value-gain' : 'value-loss'}>{pl >= 0 ? '+' : ''}{pl.toFixed(2)}</span></>
                )})()}
              </div>
              <div className="small">SL {fmtNum(analyzePos.sl)} · TP {fmtNum(analyzePos.tp)}</div>

              {posBusy ? (
                <div className="model-popup-loading"><div className="spinner sm" /><span>Claude is analyzing…</span></div>
              ) : posReview ? (
                posReview.ok ? (
                  <>
                    <div className="review-verdict">
                      <span className={`bias-badge ${actionBadge(posReview.action)}`}>{posReview.action ?? '?'}</span>
                      {posReview.confidence && <span className="conv-pill conv-medium">conviction: {posReview.confidence}</span>}
                    </div>
                    {posReview.action === 'ADJUST' && (posReview.new_sl != null || posReview.new_tp != null) && (
                      <div className="small">
                        Suggested: {posReview.new_sl != null ? <>SL <span className="value-loss">{fmtNum(posReview.new_sl)}</span></> : 'SL —'}
                        {' · '}{posReview.new_tp != null ? <>TP <span className="value-gain">{fmtNum(posReview.new_tp)}</span></> : 'TP —'}
                      </div>
                    )}
                    {posReview.reason && <div className="review-reason">{posReview.reason}</div>}
                  </>
                ) : (
                  <div className="err small">⚠ {posReview.error ?? 'analysis failed'}</div>
                )
              ) : null}

              <div className="order-confirm-actions" style={{ marginTop: 12, flexWrap: 'wrap' }}>
                <button className="btn-sm btn-buy" onClick={closeAnalyze} disabled={actionBusy}>Keep</button>
                {posReview?.ok && posReview.action === 'ADJUST' && (posReview.new_sl != null || posReview.new_tp != null) && (
                  <button className="btn-sm btn-go" onClick={doApplySltp} disabled={actionBusy || posBusy}>
                    {actionBusy ? 'Applying…' : 'Apply SL/TP'}
                  </button>
                )}
                <button className="btn-sm btn-sell" onClick={doClosePosition} disabled={actionBusy || posBusy}>
                  {actionBusy ? 'Closing…' : 'Close position'}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      <h3 style={{ marginTop: 24 }}>Pending orders <span className="muted small">({orders.length})</span></h3>
      {orders.length === 0 ? <div className="muted small">none</div> : (
        <table className="positions-table">
          <thead><tr>
            <th>Symbol</th><th>Side</th><th>Type</th><th>Oz</th><th>Order</th><th>Now</th><th>Distance</th><th>SL</th><th>TP</th><th></th>
          </tr></thead>
          <tbody>
            {orders.map(o => {
              const dist = distOf(o)
              return (
                <tr key={o.id}>
                  <td>{o.symbol}</td>
                  <td><span className={sideCls(o.side)}>{o.side}</span></td>
                  <td>{o.type}</td>
                  <td>{o.oz}</td>
                  <td>{fmtNum(o.price)}</td>
                  <td>{px == null ? '—' : px.toFixed(2)}</td>
                  <td>{dist == null ? '—' : dist.toFixed(2)}</td>
                  <td className="value-loss">{fmtNum(o.sl)}</td>
                  <td className="value-gain">{fmtNum(o.tp)}</td>
                  <td><button className="btn-sm" onClick={() => startReview(o)}>Review</button></td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
      <p className="muted small" style={{ marginTop: 16 }}>
        Live from cTrader · P/L uses the current bid · refreshes on every fill / SL / TP.
        Manage or close positions on the cTrader platform.
      </p>

      {reviewOrder && (
        <div className="modal-backdrop" onClick={cancelBusy ? undefined : closeReview}>
          <div className="modal review-modal" onClick={e => e.stopPropagation()}>
            <div className="model-popup-head">
              <span className="model-popup-name">Review order #{reviewOrder.id}</span>
              <button className="modal-close sm" onClick={closeReview} aria-label="Close">×</button>
            </div>
            <div className="review-body">
              <div className="small">
                <span className={sideCls(reviewOrder.side)}>{reviewOrder.side}</span> {reviewOrder.type} {reviewOrder.oz} oz
                {' '}@ {fmtNum(reviewOrder.price)} · now {px == null ? '—' : px.toFixed(2)}
                {' '}· dist {(() => { const d = distOf(reviewOrder); return d == null ? '—' : d.toFixed(2) })()}
              </div>
              <div className="small">SL {fmtNum(reviewOrder.sl)} · TP {fmtNum(reviewOrder.tp)}</div>

              {reviewBusy ? (
                <div className="model-popup-loading"><div className="spinner sm" /><span>Claude is reviewing…</span></div>
              ) : review ? (
                review.ok ? (
                  <>
                    <div className="review-verdict">
                      <span className={`bias-badge ${review.recommendation === 'CANCEL' ? 'bias-short' : 'bias-long'}`}>
                        {review.recommendation ?? '?'}
                      </span>
                      {review.confidence && <span className="conv-pill conv-medium">conviction: {review.confidence}</span>}
                    </div>
                    {review.reason && <div className="review-reason">{review.reason}</div>}
                  </>
                ) : (
                  <div className="err small">⚠ {review.error ?? 'review failed'}</div>
                )
              ) : null}

              <div className="order-confirm-actions" style={{ marginTop: 12 }}>
                <button className="btn-sm btn-buy" onClick={closeReview} disabled={cancelBusy}>Keep order</button>
                <button className="btn-sm btn-sell" onClick={doCancel} disabled={cancelBusy || reviewBusy}>
                  {cancelBusy ? 'Cancelling…' : 'Cancel order'}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

function formatTime(isoUtc: string): string {
  try {
    const d = new Date(isoUtc.endsWith('Z') ? isoUtc : isoUtc + 'Z')
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  } catch { return isoUtc }
}

function fmtNum(n: number | null): string {
  if (n == null) return '—'
  return Math.abs(n) >= 1000 ? n.toFixed(0) : n.toFixed(2)
}

export default App
