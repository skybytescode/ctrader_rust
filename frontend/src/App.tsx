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

type Msg = Tick | StatusMsg | EcStatusMsg | EcTodayMsg | NewsStatusMsg | NewsTodayMsg
  | PositionsMsg | TradeEventMsg | AutoStatusMsg

type ConnState = 'connecting' | 'connected' | 'disconnected' | 'error'
type Tab = 'dashboard' | 'calendar' | 'news' | 'trade-ideas' | 'positions'

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
const TRADER_PROVIDERS = ['Claude', 'Gemini'] as const
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

function App() {
  const [tick, setTick] = useState<Tick | null>(null)
  const [prevBid, setPrevBid] = useState<number | null>(null)
  const [conn, setConn] = useState<ConnState>('connecting')
  const [serverStatus, setServerStatus] = useState('')
  const [ecStatus, setEcStatus] = useState('')
  const [ecEvents, setEcEvents] = useState<EcEvent[]>([])
  const [newsStatus, setNewsStatus] = useState('')
  const [newsArticles, setNewsArticles] = useState<NewsArticle[]>([])
  const [openArticle, setOpenArticle] = useState<NewsArticle | null>(null)
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
              setTick((cur) => {
                if (cur) setPrevBid(cur.bid)
                return msg
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
            case 'news_status':
              setNewsStatus(msg.value)
              break
            case 'news_today':
              setNewsArticles(msg.articles)
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

  const spread = tick ? tick.ask - tick.bid : null
  const tickDir = tick && prevBid != null
    ? (tick.bid > prevBid ? 'up' : tick.bid < prevBid ? 'down' : 'flat')
    : 'flat'

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">cT</span>
          <span className="brand-name">Rust Terminal</span>
        </div>

        <h2 className="section">Trading Bots</h2>
        <ul className="instruments">
          <li
            className={`instrument ${selectedSymbol === 'XAUUSD' ? 'selected' : 'active'}`}
            onClick={() => { setSelectedSymbol('XAUUSD'); setTab('dashboard'); }}
          >
            <div className="sym">{tick?.symbol ?? 'XAUUSD'}</div>
            <div className={`row dir-${tickDir}`}>
              <span className="label">Bid</span>
              <span className="val">{tick ? tick.bid.toFixed(2) : '—'}</span>
            </div>
            <div className="row">
              <span className="label">Ask</span>
              <span className="val">{tick ? tick.ask.toFixed(2) : '—'}</span>
            </div>
            <div className="row spread">
              <span className="label">Spread</span>
              <span className="val">{spread != null ? spread.toFixed(2) : '—'}</span>
            </div>
          </li>
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
          <button className={tab === 'calendar' ? 'tab active' : 'tab'} onClick={() => setTab('calendar')}>
            Calendar
            {ecEvents.length > 0 && <span className="badge">{ecEvents.length}</span>}
          </button>
          <button className={tab === 'news' ? 'tab active' : 'tab'} onClick={() => setTab('news')}>
            News
            {newsArticles.length > 0 && <span className="badge">{newsArticles.length}</span>}
          </button>
          <button className={tab === 'trade-ideas' ? 'tab active' : 'tab'} onClick={() => setTab('trade-ideas')}>
            Trade Ideas
          </button>
          <button className={tab === 'positions' ? 'tab active' : 'tab'} onClick={() => setTab('positions')}>
            Positions
            {(positions.length + orders.length) > 0 && <span className="badge">{positions.length + orders.length}</span>}
          </button>
        </nav>

        {tradeBanner && (
          <div className={`trade-banner ${tradeBanner.pnl >= 0 ? 'ok' : 'loss'}`}>
            <span><strong>{tradeBanner.reason}</strong> · {tradeBanner.side} {tradeBanner.oz} oz {tradeBanner.symbol} @ {tradeBanner.close.toFixed(2)} · P/L <strong>{tradeBanner.pnl >= 0 ? '+' : ''}{tradeBanner.pnl.toFixed(2)} USD</strong></span>
            <button className="trade-banner-x" onClick={() => setTradeBanner(null)} aria-label="Dismiss">×</button>
          </div>
        )}

        <section className="panel">
          {tab === 'dashboard' && (
            selectedSymbol === 'XAUUSD'
              ? <ChartView symbol="XAUUSD" tick={tick} />
              : <DashboardView tick={tick} />
          )}
          {tab === 'calendar' && <CalendarView events={ecEvents} status={ecStatus} />}
          {tab === 'news' && (
            <NewsView articles={newsArticles} status={newsStatus} onOpen={setOpenArticle} />
          )}
          {tab === 'trade-ideas' && <TradeIdeasView />}
          {tab === 'positions' && <PositionsView positions={positions} orders={orders} tick={tick} auto={auto} />}
        </section>

        {openArticle && (
          <ArticleModal article={openArticle} onClose={() => setOpenArticle(null)} />
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

function ChartView({ symbol, tick }: { symbol: string; tick: Tick | null }) {
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
  const [timeframe, setTimeframe] = useState<Timeframe>('M1')
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
      })
      // VWAP (yellow) + 8 EMA (blue) overlays. Always create the series so
      // the lifecycle is identical across TFs; hide on non-intraday TFs by
      // pushing empty data.
      const vwapSeries = chart.addSeries(LineSeries, {
        color: '#f4c430',          // VWAP — gold/yellow
        lineWidth: 2,
        priceLineVisible: false,
        lastValueVisible: false,
        title: 'VWAP',
      })
      const emaSeries = chart.addSeries(LineSeries, {
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
          {TIMEFRAMES.map((t) => (
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
        </div>
        {tick && (
          <span className="chart-price">
            bid <strong>{tick.bid.toFixed(2)}</strong> · ask <strong>{tick.ask.toFixed(2)}</strong>
          </span>
        )}
      </div>
      <div ref={containerRef} className="chart-canvas" />
      {loading && <div className="chart-overlay">Loading {timeframe} bars…</div>}
      {error && <div className="chart-overlay err">Error: {error}</div>}
      {loadingMore && !loading && (
        <div className="chart-loadmore">Loading older bars…</div>
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

function TradeIdeasView() {
  const [open, setOpen] = useState(false)
  const [results, setResults] = useState<Record<string, ModelTradeIdea | null>>({})
  const [running, setRunning] = useState(false)

  const run = async () => {
    // Open all 4 popups immediately in a loading state, then fill each as its
    // `trade_idea_model` event arrives (fast models show first).
    setResults({ Claude: null, Gemini: null, DeepSeek: null, Qwen: null })
    setOpen(true)
    setRunning(true)
    let unlisten: UnlistenFn | null = null
    try {
      unlisten = await listen<ModelTradeIdea>('trade_idea_model', (e) => {
        const r = e.payload
        setResults(prev => ({ ...prev, [r.provider]: r }))
      })
      const all = await invoke<ModelTradeIdea[]>('get_gold_trade_ideas_multi')
      // Fallback: make sure every provider is filled even if an event was missed.
      setResults(prev => {
        const next = { ...prev }
        for (const r of all) next[r.provider] = r
        return next
      })
    } catch (err) {
      setResults(prev => {
        const next = { ...prev }
        for (const p of TRADER_PROVIDERS) {
          if (!next[p]) {
            next[p] = {
              provider: p, model: '', ok: false, bias: null, strategy: null,
              entry_low: null, entry_high: null, stop: null, target1: null, target2: null,
              rationale: null, error: String(err), duration_ms: 0,
            }
          }
        }
        return next
      })
    } finally {
      if (unlisten) unlisten()
      setRunning(false)
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
      <h3>XAUUSD Trade Ideas — 4-model day plan</h3>
      <p className="muted">
        Sends the same live VWAP + 8 EMA snapshot to four models in parallel —
        Claude, Gemini, DeepSeek, Qwen — and pops up each one's single intraday
        setup (bias · entry · SL · TP) as it answers. Read-only · target &lt; 20s.
      </p>
      <div className="archive-actions">
        <button className="btn" onClick={run} disabled={running}>
          {running ? 'Asking 4 models…' : 'Gold_Trade_Ideas'}
        </button>
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
