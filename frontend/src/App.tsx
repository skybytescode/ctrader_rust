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

type ArchiveResult = {
  status: 'written' | 'done' | 'pending' | 'error' | 'rate_limited' | 'stalled'
  day: string | null
  count: number
  path: string | null
  remaining_days: number
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
            Archive
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
  const [timeframe, setTimeframe] = useState<Timeframe>('M1')
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Bumped each time the chart is (re)initialised so dependent effects re-run
  // and pick up the now-valid chartRef/seriesRef. Refs themselves don't trigger
  // effect re-runs in React.
  const [chartGen, setChartGen] = useState(0)

  // Init the chart once per symbol mount.
  useEffect(() => {
    let cancelled = false
    ;(async () => {
      const { createChart, CandlestickSeries } = await import('lightweight-charts')
      if (cancelled || !containerRef.current) return

      const chart = createChart(containerRef.current, {
        layout: { background: { color: '#0e0e10' }, textColor: '#d4d4d8' },
        grid: {
          vertLines: { color: '#1f2128' },
          horzLines: { color: '#1f2128' },
        },
        timeScale: { timeVisible: true, secondsVisible: false, borderColor: '#25272d' },
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
        chartRef.current.remove()
        chartRef.current = null
        seriesRef.current = null
      }
    }
  }, [symbol])

  // Reload historical bars whenever timeframe or symbol changes.
  useEffect(() => {
    if (!seriesRef.current) {
      // The chart isn't ready yet; this effect will rerun once the init effect
      // sets seriesRef. We trigger that by depending on a small timer below.
    }
    let cancelled = false
    setLoading(true)
    setError(null)
    // Clear in-memory state so a tick that arrives during fetch doesn't draw
    // a candle using the previous TF's bucket size or last candle.
    lastCandleRef.current = null
    allCandlesRef.current = []
    if (seriesRef.current) {
      try { seriesRef.current.setData([]) } catch {}
    }

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
        setLoading(false)
        if (deduped.length > 0) {
          chartRef.current?.timeScale().fitContent()
        }
      } catch (e) {
        if (!cancelled) {
          console.error('[chart] invoke failed', e)
          setError(String(e))
          setLoading(false)
        }
      }
    }
    tryLoad()
    return () => { cancelled = true }
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
      // Bring the new bar into view — fitContent's old visible range stopped at
      // the last historical bar, which is now several buckets to the left of
      // the live bar.
      try {
        chartRef.current?.timeScale().fitContent()
      } catch {}
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
              onClick={() => setTimeframe(t)}
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
  return (
    <div className="placeholder">
      {tick
        ? `Live ${tick.symbol} — bid ${tick.bid.toFixed(2)} / ask ${tick.ask.toFixed(2)}`
        : 'Waiting for live ticks…'}
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

type BackfillResult = {
  fetched: number
  inserted_estimate: number
  duration_ms: number
  error: string | null
}

function ArchiveView() {
  const [status, setStatus] = useState<ArchiveResult | null>(null)
  const [lastWritten, setLastWritten] = useState<ArchiveResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [batchMode, setBatchMode] = useState<'idle' | 'running'>('idle')
  const [batchCount, setBatchCount] = useState(0)
  const [backfillBusy, setBackfillBusy] = useState(false)
  const [backfillResult, setBackfillResult] = useState<BackfillResult | null>(null)
  const [yearStatus, setYearStatus] = useState<ArchiveResult | null>(null)
  const [yearMode, setYearMode] = useState<'idle' | 'running'>('idle')
  const [yearCount, setYearCount] = useState(0)
  const [yearLast, setYearLast] = useState<ArchiveResult | null>(null)
  const stopYearRef = useRef(false)
  const stopRef = useRef(false)

  const loadStatus = async () => {
    try {
      const s = await invoke<ArchiveResult>('archive_status')
      setStatus(s)
    } catch (e) {
      setStatus({ status: 'error', day: null, count: 0, path: null, remaining_days: 0, message: String(e) })
    }
  }

  useEffect(() => { loadStatus() }, [])

  const writeNext = async () => {
    setBusy(true)
    try {
      const r = await invoke<ArchiveResult>('write_next_archive_day')
      setLastWritten(r)
      setStatus({ ...r, status: r.remaining_days > 0 ? 'pending' : 'done' })
      return r
    } catch (e) {
      setLastWritten({ status: 'error', day: null, count: 0, path: null, remaining_days: 0, message: String(e) })
      return null
    } finally {
      setBusy(false)
    }
  }

  const runBatch = async () => {
    stopRef.current = false
    setBatchMode('running')
    setBatchCount(0)
    while (!stopRef.current) {
      const r = await writeNext()
      if (!r) break
      if (r.status === 'done' || r.status === 'error' || r.status === 'rate_limited' || r.status === 'stalled') break
      setBatchCount(c => c + 1)
    }
    setBatchMode('idle')
  }

  const stopBatch = () => { stopRef.current = true }

  const loadYearStatus = async (year: string) => {
    try {
      const s = await invoke<ArchiveResult>('archive_status_for_year', { year })
      setYearStatus(s)
    } catch (e) {
      setYearStatus({ status: 'error', day: null, count: 0, path: null, remaining_days: 0, message: String(e) })
    }
  }

  const writeNextForYear = async (year: string): Promise<ArchiveResult | null> => {
    try {
      const r = await invoke<ArchiveResult>('write_next_archive_day_for_year', { year })
      setYearLast(r)
      await loadYearStatus(year)
      return r
    } catch (e) {
      setYearLast({ status: 'error', day: null, count: 0, path: null, remaining_days: 0, message: String(e) })
      return null
    }
  }

  const runYearBatch = async (year: string) => {
    stopYearRef.current = false
    setYearMode('running')
    setYearCount(0)
    await loadYearStatus(year)
    while (!stopYearRef.current) {
      const r = await writeNextForYear(year)
      if (!r) break
      if (r.status === 'done' || r.status === 'error' || r.status === 'rate_limited' || r.status === 'stalled') break
      setYearCount(c => c + 1)
    }
    setYearMode('idle')
  }

  const stopYearBatch = () => { stopYearRef.current = true }

  const backfillNews = async () => {
    setBackfillBusy(true)
    setBackfillResult(null)
    try {
      const r = await invoke<BackfillResult>('backfill_news_from_api')
      setBackfillResult(r)
      await loadStatus()  // refresh remaining days
    } catch (e) {
      setBackfillResult({ fetched: 0, inserted_estimate: 0, duration_ms: 0, error: String(e) })
    } finally {
      setBackfillBusy(false)
    }
  }

  return (
    <div className="archive">
      <h3>News archive</h3>
      <p className="muted">
        Writes one JSON file per day under <code>news_data/all/YYYY-MM/YYYY-MM-DD.json</code>,
        containing every article we have for that day (crypto already excluded by the ingest filter).
      </p>

      {status && (
        <div className="archive-status">
          {status.status === 'done' ? (
            <span className="ok">✓ All days in the local DB are archived.</span>
          ) : (
            <>
              <span>Next day to write: <strong>{status.day ?? '—'}</strong></span>
              <span className="muted"> &middot; remaining: <strong>{status.remaining_days}</strong></span>
            </>
          )}
        </div>
      )}

      <div className="archive-actions">
        <button
          className="btn"
          onClick={writeNext}
          disabled={busy || batchMode === 'running' || status?.status === 'done'}
        >
          {busy ? 'Writing…' : 'Fetch next day'}
        </button>

        {batchMode === 'idle' ? (
          <button
            className="btn btn-secondary"
            onClick={runBatch}
            disabled={busy || status?.status === 'done'}
          >
            Fetch all remaining
          </button>
        ) : (
          <button className="btn btn-danger" onClick={stopBatch}>
            Stop ({batchCount} written)
          </button>
        )}

        <button className="btn btn-ghost" onClick={loadStatus} disabled={busy}>
          Refresh
        </button>
      </div>

      <div className="backfill-section">
        <h4 className="section-h">Fetch bodies for a specific year</h4>
        <p className="muted">
          Walks only that year's days. Each click fetches one day's missing bodies
          via the proxy API and rewrites the corresponding JSON file with the bodies
          filled in.
        </p>
        {yearStatus && (
          <div className="archive-status">
            {yearStatus.status === 'done' ? (
              <span className="ok">✓ All 2026 days are fully bodied.</span>
            ) : (
              <>
                <span>Next 2026 day to write: <strong>{yearStatus.day ?? '—'}</strong></span>
                <span className="muted"> &middot; remaining: <strong>{yearStatus.remaining_days}</strong></span>
              </>
            )}
          </div>
        )}
        <div className="archive-actions">
          {yearMode === 'idle' ? (
            <button className="btn" onClick={() => runYearBatch('2026')} disabled={busy}>
              Fetch all 2026 bodies
            </button>
          ) : (
            <button className="btn btn-danger" onClick={stopYearBatch}>
              Stop ({yearCount} written)
            </button>
          )}
          <button className="btn btn-ghost" onClick={() => loadYearStatus('2026')} disabled={yearMode === 'running'}>
            Refresh
          </button>
        </div>
        {yearLast && (
          <div className={`archive-result ${yearLast.status}`}>
            {yearLast.status === 'written' && (
              <>
                <div><strong>Wrote {yearLast.day}</strong> — {yearLast.count} articles</div>
                {yearLast.message && <div className="muted small">{yearLast.message}</div>}
              </>
            )}
            {yearLast.status === 'stalled' && (
              <div className="err">Stalled: {yearLast.message}</div>
            )}
            {yearLast.status === 'rate_limited' && (
              <div className="err">Rate-limited: {yearLast.message}</div>
            )}
            {yearLast.status === 'done' && (
              <div className="ok">✓ All 2026 days bodied.</div>
            )}
            {yearLast.status === 'error' && (
              <div className="err">Error: {yearLast.message}</div>
            )}
          </div>
        )}
      </div>

      <div className="backfill-section">
        <h4 className="section-h">Fill missing days from FXStreet</h4>
        <p className="muted">
          For days where the app wasn't running (e.g. the Apr–May 2026 gap), there are no
          articles in the local DB so the archive button has nothing to write. This button
          walks up to 200 pages of the FXStreet feed and upserts what it finds. Takes ~1–3 min.
        </p>
        <div className="archive-actions">
          <button className="btn btn-secondary" onClick={backfillNews} disabled={backfillBusy}>
            {backfillBusy ? 'Pulling from FXStreet…' : 'Backfill missing news'}
          </button>
        </div>
        {backfillResult && (
          <div className={`archive-result ${backfillResult.error ? 'error' : 'written'}`}>
            {backfillResult.error ? (
              <div className="err">Error: {backfillResult.error}</div>
            ) : (
              <>
                <div>
                  Fetched <strong>{backfillResult.fetched}</strong> articles from the API,
                  added <strong>~{backfillResult.inserted_estimate}</strong> new rows
                  in {(backfillResult.duration_ms / 1000).toFixed(1)}s.
                </div>
                <div className="muted small">
                  Click "Fetch next day" or "Fetch all remaining" to write archive files for the newly-filled days.
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {lastWritten && (
        <div className={`archive-result ${lastWritten.status}`}>
          {lastWritten.status === 'written' && (
            <>
              <div><strong>Wrote {lastWritten.day}</strong> — {lastWritten.count} articles</div>
              <div className="muted small">{lastWritten.path}</div>
            </>
          )}
          {lastWritten.status === 'done' && (
            <div className="ok">✓ {lastWritten.message ?? 'All days archived.'}</div>
          )}
          {lastWritten.status === 'error' && (
            <div className="err">Error: {lastWritten.message}</div>
          )}
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
