# Candlestick Chart Implementation Tasks

## Overview
Implementation approach: DuckDB for scalability, full zoom/pan controls, complete pipeline before live updates.

## Task Breakdown

### 📦 PHASE 1: Dependencies & Setup

#### ✅ Task 1.1: Update Cargo.toml Dependencies
Add required crates for database and data processing:
- Add `duckdb = "0.10"`
- Add `arrow = "51"`  
- Add `parquet = "51"`
- Add `csv = "1.3"`
- Verify compatibility with existing dependencies

**Files**: [`Cargo.toml`](Cargo.toml:1)

#### ✅ Task 1.2: Create Module Structure
Create new module directories and files:
- Create [`src/db/mod.rs`](src/db/mod.rs:1)
- Create [`src/db/candle_loader.rs`](src/db/candle_loader.rs:1)
- Create [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)
- Create [`src/db/models.rs`](src/db/models.rs:1)
- Create [`src/trading/mod.rs`](src/trading/mod.rs:1)
- Create [`src/trading/candle_aggregator.rs`](src/trading/candle_aggregator.rs:1)
- Create [`src/trading/timeframe.rs`](src/trading/timeframe.rs:1)
- Create [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)
- Create [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)
- Create [`src/ui/chart/controls.rs`](src/ui/chart/controls.rs:1)
- Update [`src/main.rs`](src/main.rs:1) to declare new modules

---

### 🗄️ PHASE 2: Database Layer Implementation

#### ✅ Task 2.1: Define Database Models
Create shared data structures:
- Enhance [`Candle`](src/ui/app.rs:4) struct to include `volume` field
- Move `Candle` to [`src/db/models.rs`](src/db/models.rs:1) for reuse
- Add `SymbolId` enum (BTCUSD, EURUSD)
- Update [`AppState`](src/ui/app.rs:13) to use the new models

**Files**: [`src/db/models.rs`](src/db/models.rs:1), [`src/ui/app.rs`](src/ui/app.rs:1)

#### ✅ Task 2.2: Implement Timeframe Logic
Create timeframe calculations:
- Implement `Timeframe` enum in [`src/trading/timeframe.rs`](src/trading/timeframe.rs:1)
- Add `seconds()` method for each timeframe
- Add `get_bucket_start(timestamp)` helper function
- Add tests to verify 4H boundaries (00:00, 04:00, 08:00, etc.)

**Files**: [`src/trading/timeframe.rs`](src/trading/timeframe.rs:1)

#### ✅ Task 2.3: Implement CSV Loader
Load and parse historical CSV data:
- Implement `load_csv()` function in [`src/db/candle_loader.rs`](src/db/candle_loader.rs:1)
- Parse datetime strings using `chrono::NaiveDateTime`
- Convert to Unix timestamps
- Handle UTF-8 BOM in CSV header (file starts with `﻿`)
- Return `Vec<Candle>` sorted by timestamp
- Add error handling for malformed rows

**Files**: [`src/db/candle_loader.rs`](src/db/candle_loader.rs:1)

#### ✅ Task 2.4: Implement Parquet Writer
Convert CSV data to Parquet format:
- Implement `write_parquet()` in [`src/db/candle_loader.rs`](src/db/candle_loader.rs:1)
- Use Apache Arrow RecordBatch
- Define schema: timestamp (i64), open/high/low/close (f64), volume (u64)
- Write to `instruments_db/parquet_files/eurusd/EURUSD_Hour4.parquet`
- Verify file size reduction (expect ~60% smaller)

**Files**: [`src/db/candle_loader.rs`](src/db/candle_loader.rs:1)

#### ✅ Task 2.5: Implement DuckDB Client
Create database connection and operations:
- Implement `DuckDBClient` struct in [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)
- Create `new()` to initialize connection to `instruments_db/candles.duckdb`
- Implement `create_tables()` with schema
- Implement `bulk_insert_candles()` for initial load
- Implement `insert_candle()` for single inserts
- Thread-safe with `Arc<Mutex<Connection>>`

**Files**: [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)

#### ✅ Task 2.6: Implement Query Interface
Create efficient query methods:
- Implement `query_latest_n()` to get last N candles
- Implement `query_time_range()` for specific date range
- Implement `query_around_time()` for centered view
- All queries return `Vec<Candle>` sorted by time
- Add query result caching for repeated requests
- Benchmark queries (target < 5ms for 500 candles)

**Files**: [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)

#### ✅ Task 2.7: Database Initialization Pipeline
Create one-time initialization:
- Add `initialize_database()` function in [`src/db/mod.rs`](src/db/mod.rs:1)
- Check if parquet file exists
- If not, load CSV → write parquet → load into DuckDB
- If yes, just connect to existing DuckDB
- Add migration flag file to track completion
- Log progress for debugging

**Files**: [`src/db/mod.rs`](src/db/mod.rs:1)

---

### 📊 PHASE 3: Chart UI - Basic Rendering

#### ✅ Task 3.1: Enhance AppState for Charting
Add chart-specific state:
- Add `selected_symbol: Option<SymbolId>` to [`AppState`](src/ui/app.rs:13)
- Add `eurusd_candles: Vec<Candle>` for display cache
- Add `current_eurusd_4h_candle: Option<Candle>` for live candle
- Add `chart_view_start: Option<i64>` and `chart_view_end: Option<i64>`
- Add `db_client: Arc<Mutex<DuckDBClient>>` field

**Files**: [`src/ui/app.rs`](src/ui/app.rs:13)

#### ✅ Task 3.2: Implement Basic Candlestick Rendering
Create candlestick visualization:
- Implement `render_candlestick()` in [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)
- Use `egui_plot::BoxElem` for candles
- Calculate `BoxSpread` from OHLC values
- Use timestamp as X-axis value
- Apply basic green/red colors for bull/bear
- Return collection of `BoxElem` for plotting

**Files**: [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)

#### ✅ Task 3.3: Create Chart Container Widget
Build the main chart display:
- Implement `CandlestickChart` widget in [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)
- Use `egui_plot::Plot::new()`
- Configure plot settings (aspect ratio, axes, grid)
- Render candlesticks from cached data
- Handle empty state (no data loaded)
- Add loading indicator during data fetch

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)

#### ✅ Task 3.4: Make EURUSD Clickable in Sidebar
Add chart navigation:
- Modify EURUSD section in [`src/ui/app.rs`](src/ui/app.rs:138)
- Make entire group clickable using `SelectableLabel` or `Button`
- Set `selected_symbol = Some(SymbolId::EURUSD)` on click
- Add visual indicator when selected (highlighted/border)
- Trigger data load on selection change

**Files**: [`src/ui/app.rs`](src/ui/app.rs:138)

#### ✅ Task 3.5: Display Chart in Central Panel
Replace placeholder with chart:
- Modify [`CentralPanel`](src/ui/app.rs:225) in [`app.rs`](src/ui/app.rs:1)
- Check if `selected_symbol.is_some()`
- If yes, render `CandlestickChart` widget
- Pass cached candles from AppState
- If no, show placeholder message
- Add chart title and symbol info header

**Files**: [`src/ui/app.rs`](src/ui/app.rs:225)

#### ✅ Task 3.6: Implement Chart Data Loading
Load candles on symbol selection:
- Create background task to query DuckDB
- Use tokio channel to send results to AppState
- Query last 500 candles initially
- Update `eurusd_candles` in AppState
- Trigger UI repaint after data loaded
- Handle loading states (show spinner)

**Files**: [`src/ui/app.rs`](src/ui/app.rs:1), [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)

---

### 🎨 PHASE 4: Professional Styling

#### ✅ Task 4.1: Implement Color Scheme
Apply cTrader-style colors:
- Define color constants in [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)
- Bullish: `Color32::from_rgb(38, 166, 154)` (green)
- Bearish: `Color32::from_rgb(239, 83, 80)` (red)
- Background: Dark theme `Color32::from_rgb(30, 30, 30)`
- Grid: Subtle `Color32::from_rgba_premultiplied(45, 45, 45, 128)`
- Text: Light gray `Color32::from_rgb(176, 176, 176)`

**Files**: [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)

#### ✅ Task 4.2: Style Chart Grid and Axes
Configure plot appearance:
- Set dark background color in Plot
- Configure grid line styling (color, width)
- Format X-axis labels as datetime (e.g., "Jan 15 08:00")
- Format Y-axis labels with price precision (5 decimals for FX)
- Add axis labels ("Time", "Price")
- Set appropriate font sizes

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)

#### ✅ Task 4.3: Enhance Candlestick Visual Details
Add professional touches:
- Implement thin wicks (1px) vs thicker bodies (3-5px)
- Add subtle borders to candle bodies
- Handle doji candles (close == open) specially
- Ensure minimum visible size for small body candles
- Test with various zoom levels

**Files**: [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)

#### ✅ Task 4.4: Add Chart Header Info
Display symbol and stats above chart:
- Show "EURUSD - 4 Hour" title
- Display current price with color indicator
- Show spread (bid-ask)
- Display date range of visible candles
- Add last update timestamp
- Style to match cTrader aesthetic

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)

---

### 🔧 PHASE 5: Interactive Controls

#### ✅ Task 5.1: Implement Zoom Functionality
Add zoom in/out controls:
- Implement zoom in button (show more details, fewer candles)
- Implement zoom out button (show more time, more candles)
- Update `chart_view_start` and `chart_view_end` in AppState
- Query additional data if needed (going beyond cached range)
- Limit zoom levels (min: 50 candles, max: 2000 candles)
- Use egui_plot built-in zoom as fallback

**Files**: [`src/ui/chart/controls.rs`](src/ui/chart/controls.rs:1)

#### ✅ Task 5.2: Implement Pan Functionality
Add left/right navigation:
- Implement pan left button (load older data)
- Implement pan right button (return to live)
- Calculate new time range based on current view
- Query DB for new data range
- Update cache and view bounds
- Smooth transitions (no jumps)

**Files**: [`src/ui/chart/controls.rs`](src/ui/chart/controls.rs:1)

#### ✅ Task 5.3: Implement Time Range Selector
Add preset time ranges:
- Add buttons for "1 Day", "1 Week", "1 Month", "3 Months", "1 Year", "All"
- Calculate appropriate start/end times
- Query data for selected range
- Update chart view
- Highlight active range button

**Files**: [`src/ui/chart/controls.rs`](src/ui/chart/controls.rs:1)

#### ✅ Task 5.4: Implement Dynamic Data Loading
Load data on-demand:
- Detect when user zooms/pans beyond cached range
- Spawn background task to query additional candles
- Merge new data with existing cache
- Limit total cache size (e.g., max 5000 candles)
- Use LRU or similar to evict old data
- Show loading indicator during fetch

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1), [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)

---

### ⚡ PHASE 6: Real-Time Integration

#### ✅ Task 6.1: Implement Candle Aggregator
Create 4H candle aggregation logic:
- Implement `CandleAggregator` in [`src/trading/candle_aggregator.rs`](src/trading/candle_aggregator.rs:1)
- Add `process_tick(timestamp, price)` method
- Calculate 4H bucket start time
- Update current candle OHLC
- Detect and return completed candles
- Handle edge cases (gaps, first tick)

**Files**: [`src/trading/candle_aggregator.rs`](src/trading/candle_aggregator.rs:1)

#### ✅ Task 6.2: Integrate Aggregator in Session Loop
Connect tick stream to candle formation:
- Add `eurusd_4h_aggregator` to [`run_session`](src/main.rs:32) scope
- In EURUSD tick handler (line ~274), call `aggregator.process_tick()`
- If tick returns completed candle, spawn task to save to DB
- Update `current_eurusd_4h_candle` in AppState
- Keep existing EURUSD tick handling for live price display

**Files**: [`src/main.rs`](src/main.rs:274)

#### ✅ Task 6.3: Persist Completed Candles
Save completed candles to database:
- When candle completes, spawn tokio task
- Call `db_client.insert_candle()` with completed candle
- Handle errors (log, don't crash)
- Optionally update cache to include new candle
- Verify no duplicate inserts (use timestamp as key)

**Files**: [`src/main.rs`](src/main.rs:32), [`src/db/duckdb_client.rs`](src/db/duckdb_client.rs:1)

#### ✅ Task 6.4: Merge Historical with Live Candles
Combine data sources for display:
- In chart rendering, merge `eurusd_candles` (historical/cached)
- Append `current_eurusd_4h_candle` if exists
- Ensure no duplicates (current might be in cache)
- Highlight current forming candle (optional visual distinction)
- Smooth transition when candle completes

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)

#### ✅ Task 6.5: Handle Candle Completion Transitions
Smooth UI updates on 4H boundary:
- When candle completes, move from `current_` to cache
- Reload cache from DB to include newly saved candle
- No visual "jump" in chart
- Test at actual 4H boundaries (requires live testing)
- Add logging to verify timing

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1), [`src/main.rs`](src/main.rs:1)

---

### ✨ PHASE 7: Polish & Testing

#### ✅ Task 7.1: Optimize Rendering Performance
Ensure smooth 50ms refresh:
- Profile chart rendering with 500+ candles
- Optimize candlestick drawing (batch operations)
- Use egui's repaint logic efficiently
- Measure frame times, ensure < 30ms
- Test on various screen sizes
- Verify memory stability over 1+ hour

**Files**: [`src/ui/chart/candlestick.rs`](src/ui/chart/candlestick.rs:1)

#### ✅ Task 7.2: Add Error Handling
Handle edge cases gracefully:
- DB connection failures → show error in UI
- CSV load failures → log and use empty dataset
- Query timeouts → show loading indefinitely with cancel option
- Invalid candle data → skip and log
- Network interruptions → continue with cached data
- Add user-friendly error messages

**Files**: All chart and DB files

#### ✅ Task 7.3: Add Tooltips and Hover Info
Interactive feedback:
- Show candle OHLC values on hover
- Display exact timestamp
- Show volume if available
- Position tooltip near cursor
- Use egui's hover detection
- Format numbers appropriately

**Files**: [`src/ui/chart/mod.rs`](src/ui/chart/mod.rs:1)

#### ✅ Task 7.4: Add Chart Preferences
User customization:
- Add color scheme selection (optional)
- Candle width adjustment
- Grid on/off toggle
- Volume display on/off
- Store preferences in AppState
- Persist to config file (future)

**Files**: [`src/ui/chart/controls.rs`](src/ui/chart/controls.rs:1)

#### ✅ Task 7.5: Comprehensive Testing
Validate complete system:
- Test CSV → Parquet → DB pipeline
- Verify all 10,778 candles loaded correctly
- Test query performance with various ranges
- Validate OHLC accuracy against source CSV
- Test real-time candle formation with live ticks
- Test zoom/pan with large datasets
- Memory leak testing (24h runtime)
- Visual comparison with cTrader reference

**Files**: All files

---

## Implementation Order

### Week 1: Foundation
1. Dependencies & module structure (Tasks 1.1-1.2)
2. Database models and timeframe (Tasks 2.1-2.2)
3. CSV loader and Parquet writer (Tasks 2.3-2.4)
4. DuckDB client basic operations (Tasks 2.5-2.6)
5. Database initialization pipeline (Task 2.7)

**Deliverable**: Can load CSV data and query from DuckDB

### Week 2: Chart Basics
1. AppState enhancements (Task 3.1)
2. Basic candlestick rendering (Task 3.2)
3. Chart container widget (Task 3.3)
4. Sidebar integration (Task 3.4)
5. Central panel chart display (Task 3.5)
6. Background data loading (Task 3.6)

**Deliverable**: Static historical chart works, clickable from sidebar

### Week 3: Polish & Controls
1. Professional color scheme (Task 4.1)
2. Grid and axes styling (Task 4.2)
3. Candlestick visual details (Task 4.3)
4. Chart header info (Task 4.4)
5. Zoom controls (Task 5.1)
6. Pan controls (Task 5.2)
7. Time range selector (Task 5.3)
8. Dynamic loading (Task 5.4)

**Deliverable**: Professional-looking, interactive chart

### Week 4: Real-Time & Final
1. Candle aggregator (Task 6.1)
2. Session loop integration (Task 6.2)
3. Persist completed candles (Task 6.3)
4. Merge historical + live (Task 6.4)
5. Candle completion transitions (Task 6.5)
6. Performance optimization (Task 7.1)
7. Error handling (Task 7.2)
8. Tooltips and hover (Task 7.3)
9. Chart preferences (Task 7.4)
10. Comprehensive testing (Task 7.5)

**Deliverable**: Fully functional real-time candlestick chart with professional quality

## Technical Specifications

### Database Configuration
```rust
// DuckDB in-process config
Config {
    access_mode: AccessMode::ReadWrite,
    default_order: OrderType::ASC,
    enable_external_access: false,
    threads: 2,  // Limit for background queries
}
```

### Query Examples
```sql
-- Get last 500 candles
SELECT * FROM eurusd_candles 
WHERE symbol = 'EURUSD' AND timeframe = '4H'
ORDER BY timestamp DESC 
LIMIT 500;

-- Get candles in time range
SELECT * FROM eurusd_candles 
WHERE symbol = 'EURUSD' 
  AND timeframe = '4H'
  AND timestamp BETWEEN ? AND ?
ORDER BY timestamp ASC;

-- Get candles around specific time
SELECT * FROM eurusd_candles 
WHERE symbol = 'EURUSD' 
  AND timeframe = '4H'
  AND timestamp >= ?
ORDER BY timestamp ASC 
LIMIT 250;
```

### Performance Targets

| Metric | Target | Maximum |
|--------|--------|---------|
| DB query (500 candles) | < 5ms | < 20ms |
| Chart render (500 candles) | < 10ms | < 30ms |
| UI refresh rate | 50ms (20 FPS) | 100ms (10 FPS) |
| Memory usage | < 100MB | < 200MB |
| CSV → Parquet | < 2s | < 10s |
| CSV → DuckDB | < 5s | < 30s |

### Data Volume Estimates
- Historical candles: 10,778 rows × 40 bytes = ~420KB in memory
- Cached display: 500 candles × 40 bytes = 20KB
- Parquet file: ~500KB (compressed)
- DuckDB file: ~1-2MB (with indexes)

## Risk Mitigation

### Performance Risks
1. **egui_plot performance with many candles**
   - Mitigation: Limit visible candles to 500-1000
   - Fallback: Implement LOD (level of detail) rendering

2. **DB query blocking UI**
   - Mitigation: Always query in background task
   - Show loading indicator during queries

3. **Memory growth over time**
   - Mitigation: Limit cache size, periodic cleanup
   - Monitor with debug logs

### Data Risks
1. **CSV parsing failures**
   - Mitigation: Robust error handling, skip bad rows
   - Validation of parsed values

2. **Timezone confusion (CSV vs API)**
   - Mitigation: Use UTC consistently everywhere
   - Document timezone assumptions

3. **Candle boundary off-by-one errors**
   - Mitigation: Extensive testing at 4H boundaries
   - Unit tests for bucket calculations

## Testing Strategy

### Unit Tests
- Timeframe bucket calculation for various timestamps
- CSV datetime parsing with edge cases
- Candle aggregator logic (tick sequence → OHLC)
- OHLC validation (High ≥ Open/Close, etc.)

### Integration Tests
- CSV → Parquet → DuckDB → Query round-trip
- Load all historical data and verify count
- Spot-check random candles against CSV
- Query performance benchmarks

### Manual Tests
- Visual comparison with cTrader desktop
- Live tick integration for 4+ hours
- Zoom/pan with large datasets
- Multiple symbol switching
- Memory stability over 24h

### Performance Tests
- Query 500 candles 1000x, measure p50/p95/p99
- Render 500 candles continuously for 1min, measure FPS
- Monitor memory usage over 1h of live trading
- Profile rendering with 1000+ candles

## Success Criteria

### Functionality
- ✅ All 10,778 historical candles loaded from CSV
- ✅ Chart displays professional candlesticks
- ✅ EURUSD clickable in sidebar shows chart
- ✅ Real-time 4H candle updates from ticks
- ✅ Zoom in/out works smoothly
- ✅ Pan left/right loads historical data
- ✅ Chart maintains 50ms refresh rate

### Quality
- ✅ Visual quality matches cTrader reference
- ✅ No memory leaks over 24h runtime
- ✅ All queries complete in < 20ms
- ✅ No UI freezing or stuttering
- ✅ Error handling for all failure modes
- ✅ Code is well-documented and maintainable

### User Experience
- ✅ Chart loads in < 1s on first click
- ✅ Smooth transitions and animations
- ✅ Intuitive controls (zoom, pan, time range)
- ✅ Helpful tooltips and hover information
- ✅ Responsive to user interactions
- ✅ Matches expected professional trading platform UX

## Next Steps

1. **Review Architecture**: Confirm approach aligns with your vision
2. **Clarify Requirements**: Any additional features or constraints?
3. **Begin Implementation**: Start with Phase 1 (Dependencies & Database)
4. **Iterative Development**: Complete each phase, test, then proceed
5. **Continuous Integration**: Test with live data throughout development

## Questions for Consideration

1. Should we support multiple timeframes (1H, 1D) from the start, or add later?
2. Do you want to switch between multiple symbols in the same session?
3. Should historical data updates be supported (re-downloading CSV)?
4. Any specific chart indicators to prepare for (MA, Bollinger, etc.)?
5. Export functionality needed (chart screenshot, data export)?

## Appendix: Alternative Approaches Considered

### Alternative 1: In-Memory Only (Not Chosen)
- **Pros**: Simpler, no DB dependencies, faster queries
- **Cons**: All data in RAM, not scalable, no persistence
- **Decision**: DuckDB better for scalability and future multi-symbol support

### Alternative 2: SQLite Instead of DuckDB (Not Chosen)
- **Pros**: More common, smaller binary size
- **Cons**: 10-100x slower for analytical queries, worse compression
- **Decision**: DuckDB specialized for time-series analytics

### Alternative 3: Custom Rendering instead of egui_plot (Not Chosen)
- **Pros**: Full control over appearance
- **Cons**: Much more work, reinventing the wheel
- **Decision**: egui_plot sufficient for candlesticks, can customize if needed

