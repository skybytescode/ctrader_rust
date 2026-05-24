import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
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
type Msg = Tick | StatusMsg | EcStatusMsg | EcTodayMsg | NewsStatusMsg | NewsTodayMsg

type ConnState = 'connecting' | 'connected' | 'disconnected' | 'error'
type Tab = 'dashboard' | 'calendar' | 'news' | 'archive'

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
  const reconnectRef = useRef<number | null>(null)

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
          <button className={tab === 'archive' ? 'tab active' : 'tab'} onClick={() => setTab('archive')}>
            Archives
          </button>
        </nav>

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
          {tab === 'archive' && <ArchiveView />}
        </section>

        {openArticle && (
          <ArticleModal article={openArticle} onClose={() => setOpenArticle(null)} />
        )}
      </main>
    </div>
  )
}

type Candle = { time: number; open: number; high: number; low: number; close: number; volume: number }

type Timeframe = 'M1'|'M2'|'M3'|'M4'|'M5'|'M10'|'M15'|'M30'|'H1'|'H4'|'H12'|'D1'|'W1'|'MN1'

const TIMEFRAMES: Timeframe[] = ['M1','M2','M3','M4','M5','M10','M15','M30','H1','H4','H12','D1','W1','MN1']

const SECONDS_PER_BAR: Record<Timeframe, number> = {
  M1: 60, M2: 120, M3: 180, M4: 240, M5: 300, M10: 600, M15: 900, M30: 1800,
  H1: 3600, H4: 14400, H12: 43200, D1: 86400, W1: 604800, MN1: 2592000,
}

// How many bars to request per timeframe — keep wider TFs to fewer bars so the
// from_timestamp stays within cTrader's per-period range limit.
const BARS_PER_TF: Record<Timeframe, number> = {
  M1: 1000, M2: 1000, M3: 1000, M4: 1000, M5: 1000, M10: 1000, M15: 1000, M30: 1000,
  H1: 1000, H4: 500, H12: 500, D1: 500, W1: 300, MN1: 200,
}

function ChartView({ symbol, tick }: { symbol: string; tick: Tick | null }) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const chartRef = useRef<any>(null)
  const seriesRef = useRef<any>(null)
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

  // Recreate the chart on every symbol *or* timeframe change. The chart
  // instance carries hidden zoom/scroll state that survives setData and resists
  // applyOptions/resetTimeScale, so the only way to guarantee each timeframe
  // starts with its own clean (or saved) zoom is to throw away the old chart
  // and build a new one. Cache hits keep this near-instant.
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      const { createChart, CandlestickSeries } = await import('lightweight-charts')
      if (cancelled || !containerRef.current) return

      // Tear down any previous chart for this symbol/TF.
      if (chartRef.current) {
        try { chartRef.current.remove() } catch { /* ignore */ }
        chartRef.current = null
        seriesRef.current = null
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
        },
        rightPriceScale: { borderColor: '#25272d' },
        autoSize: true,
      })
      const series = chart.addSeries(CandlestickSeries, {
        upColor: '#2dd47b',  downColor: '#f87171',
        borderUpColor: '#2dd47b', borderDownColor: '#f87171',
        wickUpColor: '#2dd47b',   wickDownColor: '#f87171',
      })
      chartRef.current = chart
      seriesRef.current = series
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

function ArchiveView() {
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<NewsUpdateResult | null>(null)
  const [error, setError] = useState<string | null>(null)

  const runUpdate = async () => {
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      const r = await invoke<NewsUpdateResult>('update_news_archive')
      setResult(r)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="archive">
      <h3>News Archives</h3>
      <p className="muted">
        Detects the newest day file under <code>news_data/all/</code>, reads its latest
        article time, then fetches every newer article from FXStreet and (re)writes
        one JSON file per affected day up to today.
      </p>

      <div className="archive-actions">
        <button className="btn" onClick={runUpdate} disabled={busy}>
          {busy ? 'Updating… (this can take a minute)' : 'News_Updates'}
        </button>
      </div>

      {error && <div className="archive-result err">Error: {error}</div>}

      {result && (
        <div className={`archive-result ${result.rate_limited ? 'err' : 'ok'}`}>
          <div>
            <strong>Last archive day:</strong> {result.last_archive_day ?? '(none — first run)'}
          </div>
          <div>
            <strong>Cutoff used:</strong> {result.cutoff}
          </div>
          <div>
            <strong>Articles fetched:</strong> {result.articles_fetched}
          </div>
          <div>
            <strong>Bodies:</strong> {result.bodies_fetched} fetched
            {result.bodies_empty > 0 && `, ${result.bodies_empty} empty`}
            {result.bodies_failed > 0 && `, ${result.bodies_failed} failed`}
          </div>
          <div>
            <strong>Day files written:</strong> {result.days_written.length}
          </div>
          {result.days_written.length > 0 && (
            <ul className="archive-days">
              {result.days_written.map(([day, count]) => (
                <li key={day}><code>{day}.json</code> — {count} articles</li>
              ))}
            </ul>
          )}
          {result.message && <div className="muted small">{result.message}</div>}
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
