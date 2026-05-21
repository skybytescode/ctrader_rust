import { useEffect, useRef, useState } from 'react'
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
type NewsTodayMsg = { type: 'news_today'; lines: string[] }
type Msg = Tick | StatusMsg | EcStatusMsg | EcTodayMsg | NewsStatusMsg | NewsTodayMsg

type ConnState = 'connecting' | 'connected' | 'disconnected' | 'error'
type Tab = 'dashboard' | 'calendar' | 'news'

function App() {
  const [tick, setTick] = useState<Tick | null>(null)
  const [prevBid, setPrevBid] = useState<number | null>(null)
  const [conn, setConn] = useState<ConnState>('connecting')
  const [serverStatus, setServerStatus] = useState('')
  const [ecStatus, setEcStatus] = useState('')
  const [ecEvents, setEcEvents] = useState<EcEvent[]>([])
  const [newsStatus, setNewsStatus] = useState('')
  const [newsLines, setNewsLines] = useState<string[]>([])
  const [tab, setTab] = useState<Tab>('dashboard')
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
              setNewsLines(msg.lines)
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
          <li className="instrument active">
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
            {newsLines.length > 0 && <span className="badge">{newsLines.length}</span>}
          </button>
        </nav>

        <section className="panel">
          {tab === 'dashboard' && <DashboardView tick={tick} />}
          {tab === 'calendar' && <CalendarView events={ecEvents} status={ecStatus} />}
          {tab === 'news' && <NewsView lines={newsLines} status={newsStatus} />}
        </section>
      </main>
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

function NewsView({ lines, status }: { lines: string[]; status: string }) {
  if (lines.length === 0) {
    return <div className="placeholder">{status || 'Waiting for news headlines…'}</div>
  }
  return (
    <div className="news-list">
      <div className="news-status">{status}</div>
      <ul>
        {lines.map((line, i) => <li key={i}>{line}</li>)}
      </ul>
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
