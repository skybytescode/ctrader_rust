//! Bevy systems for native chart rendering
//!
//! Systems that manage spawning, updating, and despawning of chart entities.

use bevy::prelude::*;
use bevy_prototype_lyon::prelude::*;

use super::components::*;
use crate::ui::chart::state::{ChartState, TickDirection};
use crate::ui::app::AppState;

/// System to setup the chart camera (runs once at startup)
pub fn setup_chart_camera(mut commands: Commands) {
    // Spawn a 2D camera for the chart
    // We'll position it based on the chart viewport and use viewport clipping
    commands.spawn((
        Camera2d,
        ChartCamera,
        Camera {
            // Set a lower order so it renders behind the egui camera
            order: -1,
            clear_color: ClearColorConfig::Custom(chart_colors::BACKGROUND),
            // Viewport will be set dynamically based on ChartViewport
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 1000.0),
    ));
}

/// System to update chart viewport based on ChartState and visible data
pub fn update_chart_viewport(
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    mut viewport: ResMut<ChartViewport>,
    windows: Query<&Window>,
) {
    // Get window dimensions
    let Ok(window) = windows.get_single() else {
        return;
    };

    // Get selected instrument and timeframe
    let Some(symbol) = &chart_state.selected_instrument else {
        return;
    };
    let Some(instrument) = app_state.instruments.get(symbol) else {
        return;
    };

    let candles = instrument
        .candles
        .get(&chart_state.selected_timeframe)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    if candles.is_empty() {
        return;
    }

    // Calculate chart area (leaving space for sidebars and toolbars)
    // These values should match the egui layout
    let sidebar_width = 330.0; // icon_bar + content_sidebar
    let top_panel_height = 30.0;
    let toolbar_height = 30.0;
    let scrollbar_height = 20.0;
    let price_axis_width = 80.0;
    let time_axis_height = 30.0;

    let chart_left = sidebar_width;
    let chart_top = top_panel_height + toolbar_height;
    let chart_width = window.width() - sidebar_width - price_axis_width;
    let chart_height = window.height() - chart_top - scrollbar_height - time_axis_height;

    viewport.position = Vec2::new(chart_left, chart_top);
    viewport.size = Vec2::new(chart_width.max(100.0), chart_height.max(100.0));
    viewport.decimal_places = instrument.decimal_places;
    viewport.trades_weekends = instrument.trades_weekends;

    // Calculate candle width based on zoom
    let candle_base_width = 12.0;
    viewport.candle_width = candle_base_width / chart_state.zoom_level as f32;

    // Set timeframe interval for timestamp-based positioning
    let interval = chart_state.selected_timeframe.seconds();
    viewport.timeframe_interval = interval;

    // Get the full timestamp range from all candles
    let oldest_timestamp = candles.first().map(|c| c.timestamp).unwrap_or(0);
    let newest_timestamp = candles.last().map(|c| c.timestamp).unwrap_or(0);

    if oldest_timestamp == 0 || newest_timestamp == 0 {
        return;
    }

    // Calculate how many slots fit on screen
    let visible_slots = chart_state.visible_slot_count(viewport.size.x, viewport.candle_width) as i64;

    use chrono::{Datelike, TimeZone, Utc};

    // Helper to check if timestamp is on weekend
    let is_weekend = |ts: i64| -> bool {
        if let Some(dt) = Utc.timestamp_opt(ts, 0).single() {
            let weekday = dt.weekday();
            weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun
        } else {
            false
        }
    };

    // Helper to count N trading slots forward from a timestamp (skipping weekends for forex)
    let count_slots_forward = |start_ts: i64, slots: i64, skip_weekends: bool| -> i64 {
        if !skip_weekends {
            return start_ts + (slots * interval);
        }
        let mut ts = start_ts;
        let mut counted = 0i64;
        while counted < slots {
            ts += interval;
            if !is_weekend(ts) {
                counted += 1;
            }
        }
        ts
    };

    // Helper to count N trading slots backward from a timestamp (skipping weekends for forex)
    let count_slots_backward = |end_ts: i64, slots: i64, skip_weekends: bool, min_ts: i64| -> i64 {
        if !skip_weekends {
            return end_ts - (slots * interval);
        }
        let mut ts = end_ts;
        let mut counted = 0i64;
        while counted < slots && ts > min_ts {
            ts -= interval;
            if !is_weekend(ts) {
                counted += 1;
            }
        }
        ts
    };

    let skip_weekends = !instrument.trades_weekends;

    // pan_offset determines which part of the time range to show:
    // - pan_offset = 0: show newest data (right edge at newest_timestamp)
    // - pan_offset > 0: show older data (scrolled left into history)
    // - pan_offset < 0: show future space (scrolled right past current)

    // Calculate visible_end_ts based on pan_offset (skip weekends for forex)
    let visible_end_ts = if chart_state.pan_offset >= 0 {
        // Panning into history: count backwards from newest
        count_slots_backward(newest_timestamp, chart_state.pan_offset, skip_weekends, oldest_timestamp)
    } else {
        // Panning into future: count forwards from newest
        count_slots_forward(newest_timestamp, -chart_state.pan_offset, skip_weekends)
    };

    // Calculate visible_start_ts by counting backwards from visible_end
    let visible_start_ts = count_slots_backward(
        visible_end_ts,
        visible_slots,
        skip_weekends,
        oldest_timestamp - (visible_slots * interval * 2) // Safety margin
    );

    // Clamp to reasonable bounds (also accounting for weekends)
    let visible_start_ts = visible_start_ts.max(
        count_slots_backward(oldest_timestamp, visible_slots / 2, skip_weekends, 0)
    );
    let max_future = count_slots_forward(newest_timestamp, visible_slots, skip_weekends);
    let visible_end_ts = visible_end_ts.min(max_future);

    viewport.time_start = visible_start_ts;
    viewport.time_end = visible_end_ts;

    // Calculate total_slots - for forex, count only trading slots
    viewport.total_slots = if instrument.trades_weekends {
        ((visible_end_ts - visible_start_ts) / interval) as usize + 1
    } else {
        viewport.count_trading_slots(visible_start_ts, visible_end_ts) as usize + 1
    };

    // Calculate future_slots (empty space past the newest candle)
    viewport.future_slots = if visible_end_ts > newest_timestamp {
        if instrument.trades_weekends {
            ((visible_end_ts - newest_timestamp) / interval) as usize
        } else {
            viewport.count_trading_slots(newest_timestamp, visible_end_ts) as usize
        }
    } else {
        0
    };

    // Calculate price range from ALL candles (or just those in visible time range)
    // For simplicity, use all candles for price range calculation
    let (min_price, max_price) = candles.iter().fold(
        (f64::MAX, f64::MIN),
        |(min, max), c| (min.min(c.low), max.max(c.high)),
    );

    // Apply vertical zoom
    let range = max_price - min_price;
    let padding = range * 0.05;
    let base_min = min_price - padding;
    let base_max = max_price + padding;
    let base_range = base_max - base_min;

    let zoomed_range = base_range / chart_state.vertical_zoom;
    let center = chart_state.price_center.unwrap_or((base_min + base_max) / 2.0);

    viewport.price_min = center - zoomed_range / 2.0;
    viewport.price_max = center + zoomed_range / 2.0;
}

/// System to update camera position and viewport to match chart area
pub fn update_chart_camera(
    viewport: Res<ChartViewport>,
    mut camera_query: Query<(&mut Transform, &mut OrthographicProjection, &mut Camera), With<ChartCamera>>,
    windows: Query<&Window>,
) {
    let Ok((mut transform, mut projection, mut camera)) = camera_query.get_single_mut() else {
        return;
    };
    let Ok(window) = windows.get_single() else {
        return;
    };

    // Set camera viewport to clip rendering to the chart area
    // Viewport uses physical pixels and screen coordinates (origin at top-left)
    let scale_factor = window.scale_factor();
    camera.viewport = Some(bevy::render::camera::Viewport {
        physical_position: UVec2::new(
            (viewport.position.x * scale_factor) as u32,
            (viewport.position.y * scale_factor) as u32,
        ),
        physical_size: UVec2::new(
            (viewport.size.x * scale_factor) as u32,
            (viewport.size.y * scale_factor) as u32,
        ),
        ..default()
    });

    // Position camera at origin (viewport handles positioning)
    transform.translation.x = 0.0;
    transform.translation.y = 0.0;

    // Set orthographic projection to match chart size
    projection.scaling_mode = bevy::render::camera::ScalingMode::Fixed {
        width: viewport.size.x,
        height: viewport.size.y,
    };
}

/// System to spawn/update candlestick entities based on visible data
pub fn spawn_candle_entities(
    mut commands: Commands,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_wicks: Query<Entity, With<CandleWick>>,
    existing_bodies: Query<Entity, With<CandleBody>>,
) {
    // Clear ALL existing candle entities first (wicks and bodies)
    for entity in existing_wicks.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_bodies.iter() {
        commands.entity(entity).despawn();
    }

    // Get candle data
    let Some(symbol) = &chart_state.selected_instrument else {
        return;
    };

    let Some(instrument) = app_state.instruments.get(symbol) else {
        return;
    };

    let candles = instrument
        .candles
        .get(&chart_state.selected_timeframe)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    if candles.is_empty() {
        return;
    }

    // Filter candles by visible time range (timestamp-based)
    let visible_candles: Vec<_> = candles
        .iter()
        .enumerate()
        .filter(|(_, c)| c.timestamp >= viewport.time_start && c.timestamp <= viewport.time_end)
        .collect();

    // Spawn new candle entities
    let gap = 2.0;
    let body_width = (viewport.candle_width - gap).max(3.0);

    for (idx, candle) in visible_candles.iter() {
        let is_bullish = candle.close >= candle.open;
        let color = if is_bullish { chart_colors::BULLISH } else { chart_colors::BEARISH };

        // Calculate positions - use timestamp-based positioning to handle gaps
        let x = viewport.timestamp_to_x(candle.timestamp);
        let body_top = viewport.price_to_y(candle.open.max(candle.close));
        let body_bottom = viewport.price_to_y(candle.open.min(candle.close));
        let wick_top = viewport.price_to_y(candle.high);
        let wick_bottom = viewport.price_to_y(candle.low);

        let body_height = (body_top - body_bottom).abs().max(1.0);
        let body_center_y = (body_top + body_bottom) / 2.0;

        // Spawn wick (line from high to low) using a thin rectangle
        let wick_height = (wick_top - wick_bottom).abs().max(1.0);
        let wick_center_y = (wick_top + wick_bottom) / 2.0;
        let wick_shape = shapes::Rectangle {
            extents: Vec2::new(1.0, wick_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&wick_shape),
                transform: Transform::from_xyz(x, wick_center_y, 0.0),
                ..default()
            },
            Fill::color(color),
            CandleWick { candle_index: *idx },
            ChartEntity,
        ));

        // Spawn body (rectangle)
        let body_shape = shapes::Rectangle {
            extents: Vec2::new(body_width, body_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&body_shape),
                transform: Transform::from_xyz(x, body_center_y, 1.0),
                ..default()
            },
            Fill::color(color),
            CandleBody { candle_index: *idx },
            ChartEntity,
        ));
    }
}

/// Helper function to spawn a dashed vertical line
fn spawn_dashed_vertical_line(
    commands: &mut Commands,
    x: f32,
    total_height: f32,
    color: bevy::prelude::Color,
    dash_length: f32,
    gap_length: f32,
    line_width: f32,
    timestamp: i64,
    is_day_separator: bool,
) {
    let mut y = -total_height / 2.0;

    while y < total_height / 2.0 {
        let end_y = (y + dash_length).min(total_height / 2.0);
        let segment_height = end_y - y;

        let dash_shape = shapes::Rectangle {
            extents: Vec2::new(line_width, segment_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        if is_day_separator {
            commands.spawn((
                ShapeBundle {
                    path: GeometryBuilder::build_as(&dash_shape),
                    transform: Transform::from_xyz(x, y + segment_height / 2.0, 0.5),
                    ..default()
                },
                Fill::color(color),
                DaySeparator { timestamp },
                ChartEntity,
            ));
        } else {
            commands.spawn((
                ShapeBundle {
                    path: GeometryBuilder::build_as(&dash_shape),
                    transform: Transform::from_xyz(x, y + segment_height / 2.0, 0.5),
                    ..default()
                },
                Fill::color(color),
                WeekSeparator { timestamp },
                ChartEntity,
            ));
        }

        y += dash_length + gap_length;
    }
}

/// System to spawn day and week separator lines
/// Iterates through time slots in the visible range up to the newest candle (not in future)
pub fn spawn_time_separators(
    mut commands: Commands,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_day_seps: Query<Entity, With<DaySeparator>>,
    existing_week_seps: Query<Entity, With<WeekSeparator>>,
) {
    use chrono::{Datelike, TimeZone, Utc};

    // Clear existing separators
    for entity in existing_day_seps.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_week_seps.iter() {
        commands.entity(entity).despawn();
    }

    // Need valid time range
    if viewport.time_start == 0 || viewport.time_end == 0 || viewport.timeframe_interval == 0 {
        return;
    }

    // Get the newest candle timestamp to avoid drawing separators in the future
    let newest_timestamp = if let Some(symbol) = &chart_state.selected_instrument {
        if let Some(instrument) = app_state.instruments.get(symbol) {
            instrument.candles
                .get(&chart_state.selected_timeframe)
                .and_then(|candles| candles.last())
                .map(|c| c.timestamp)
                .unwrap_or(viewport.time_end)
        } else {
            viewport.time_end
        }
    } else {
        viewport.time_end
    };

    let line_height = viewport.size.y;
    let interval = viewport.timeframe_interval;

    // Track previous slot's day/week for detecting boundaries
    let mut prev_day: Option<u32> = None;
    let mut prev_week: Option<u32> = None;

    // Iterate through time slots from start to newest candle (not into future)
    let mut timestamp = viewport.time_start;
    while timestamp <= viewport.time_end && timestamp <= newest_timestamp {
        let dt = Utc.timestamp_opt(timestamp, 0).single();
        if let Some(dt) = dt {
            // Skip weekends for forex instruments (trades_weekends = false)
            if !viewport.trades_weekends {
                let weekday = dt.weekday();
                if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
                    timestamp += interval;
                    continue;
                }
            }

            let current_day = dt.ordinal(); // Day of year (1-366)
            let current_week = dt.iso_week().week(); // Week number (1-53)

            // Use timestamp-based positioning
            let x = viewport.timestamp_to_x(timestamp);

            // Check for week boundary first (takes precedence over day)
            let is_week_boundary = if let Some(pw) = prev_week {
                current_week != pw
            } else {
                false
            };

            // Check for day boundary (skip if it's also a week boundary)
            if !is_week_boundary {
                if let Some(pd) = prev_day {
                    if current_day != pd {
                        // Draw day separator as dashed yellow line
                        let sep_x = x - viewport.candle_width / 2.0;
                        spawn_dashed_vertical_line(
                            &mut commands,
                            sep_x,
                            line_height,
                            chart_colors::DAY_SEPARATOR,
                            8.0,  // dash length
                            6.0,  // gap length
                            2.0,  // line width
                            timestamp,
                            true, // is_day_separator
                        );
                    }
                }
            }

            // Draw week boundary (double dashed gold lines)
            if is_week_boundary {
                let sep_x = x - viewport.candle_width / 2.0;

                // First dashed line
                spawn_dashed_vertical_line(
                    &mut commands,
                    sep_x - 3.0,
                    line_height,
                    chart_colors::WEEK_SEPARATOR,
                    8.0,  // dash length
                    6.0,  // gap length
                    2.0,  // line width
                    timestamp,
                    false, // is_day_separator (false = week)
                );

                // Second dashed line (parallel)
                spawn_dashed_vertical_line(
                    &mut commands,
                    sep_x + 3.0,
                    line_height,
                    chart_colors::WEEK_SEPARATOR,
                    8.0,
                    6.0,
                    2.0,
                    timestamp,
                    false,
                );
            }

            prev_day = Some(current_day);
            prev_week = Some(current_week);
        }

        timestamp += interval;
    }
}

/// System to spawn/update volume bar entities
pub fn spawn_volume_entities(
    mut commands: Commands,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_volumes: Query<Entity, With<VolumeBar>>,
) {
    // Clear existing volume bars
    for entity in existing_volumes.iter() {
        commands.entity(entity).despawn_recursive();
    }

    if !chart_state.show_volume {
        return;
    }

    let Some(symbol) = &chart_state.selected_instrument else {
        return;
    };
    let Some(instrument) = app_state.instruments.get(symbol) else {
        return;
    };

    let candles = instrument
        .candles
        .get(&chart_state.selected_timeframe)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    if candles.is_empty() {
        return;
    }

    // Filter candles by visible time range (timestamp-based)
    let visible_candles: Vec<_> = candles
        .iter()
        .enumerate()
        .filter(|(_, c)| c.timestamp >= viewport.time_start && c.timestamp <= viewport.time_end)
        .collect();

    // Calculate max volume for scaling
    let max_volume = visible_candles.iter().map(|(_, c)| c.volume).max().unwrap_or(1) as f64;
    if max_volume <= 0.0 {
        return;
    }

    // Volume area is at the bottom 15% of the chart
    let volume_height = viewport.size.y * 0.15;
    let volume_bottom = -viewport.size.y / 2.0;

    let gap = 2.0;
    let body_width = (viewport.candle_width - gap).max(3.0);

    for (idx, candle) in visible_candles.iter() {
        let is_bullish = candle.close >= candle.open;
        let color = if is_bullish { chart_colors::BULLISH_ALPHA } else { chart_colors::BEARISH_ALPHA };

        // Use timestamp-based positioning to handle gaps
        let x = viewport.timestamp_to_x(candle.timestamp);
        let height_ratio = candle.volume as f32 / max_volume as f32;
        let bar_height = (height_ratio * volume_height).max(1.0);

        let volume_shape = shapes::Rectangle {
            extents: Vec2::new(body_width, bar_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        // Position at bottom + half height (center origin)
        let y = volume_bottom + bar_height / 2.0;

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&volume_shape),
                transform: Transform::from_xyz(x, y, 0.0),
                ..default()
            },
            Fill::color(color),
            VolumeBar {
                candle_index: *idx,
                volume: candle.volume,
            },
            ChartEntity,
        ));
    }
}

/// System to spawn/update grid lines
pub fn spawn_grid_entities(
    mut commands: Commands,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_grid_h: Query<Entity, With<GridLineH>>,
    existing_grid_v: Query<Entity, With<GridLineV>>,
) {
    // Clear existing grid lines
    for entity in existing_grid_h.iter() {
        commands.entity(entity).despawn();
    }
    for entity in existing_grid_v.iter() {
        commands.entity(entity).despawn();
    }

    if !chart_state.show_grid {
        return;
    }

    // Calculate nice grid intervals for horizontal lines
    let price_range = viewport.price_max - viewport.price_min;
    if price_range <= 0.0 {
        return;
    }

    let target_lines = 6;
    let raw_interval = price_range / target_lines as f64;
    let magnitude = 10f64.powf(raw_interval.log10().floor());
    let normalized = raw_interval / magnitude;
    let nice_interval = if normalized <= 1.5 {
        magnitude
    } else if normalized <= 3.0 {
        2.0 * magnitude
    } else if normalized <= 7.0 {
        5.0 * magnitude
    } else {
        10.0 * magnitude
    };

    // Spawn horizontal grid lines
    let first_line = (viewport.price_min / nice_interval).ceil() * nice_interval;
    let mut price = first_line;

    while price <= viewport.price_max {
        let y = viewport.price_to_y(price);

        // Use a thin rectangle for the line
        let line_shape = shapes::Rectangle {
            extents: Vec2::new(viewport.size.x, 1.0),
            origin: RectangleOrigin::Center,
            ..default()
        };

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&line_shape),
                transform: Transform::from_xyz(0.0, y, -1.0), // Behind candles
                ..default()
            },
            Fill::color(chart_colors::GRID),
            GridLineH { price },
            ChartEntity,
        ));

        price += nice_interval;
    }
}

/// System to spawn the live price line
pub fn spawn_live_price_line(
    mut commands: Commands,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_lines: Query<Entity, With<LivePriceLine>>,
) {
    // Clear existing live price line
    for entity in existing_lines.iter() {
        commands.entity(entity).despawn();
    }

    let Some(symbol) = &chart_state.selected_instrument else {
        return;
    };
    let Some(instrument) = app_state.instruments.get(symbol) else {
        return;
    };

    // Use mid-price (average of bid and ask)
    let mid_price = instrument.mid_price();
    if mid_price <= 0.0 || !viewport.is_price_visible(mid_price) {
        return;
    }

    let y = viewport.price_to_y(mid_price);

    // Color based on tick direction: green for up, orange for down
    let line_color = match instrument.tick_direction {
        TickDirection::Up => chart_colors::BULLISH,
        TickDirection::Down => chart_colors::BEARISH,
    };

    // Create dashed line using multiple small rectangles
    let dash_length = 5.0;
    let gap_length = 3.0;
    let line_height = 1.5;
    let mut x = -viewport.size.x / 2.0;

    while x < viewport.size.x / 2.0 {
        let end_x = (x + dash_length).min(viewport.size.x / 2.0);
        let segment_width = end_x - x;

        let dash_shape = shapes::Rectangle {
            extents: Vec2::new(segment_width, line_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&dash_shape),
                transform: Transform::from_xyz(x + segment_width / 2.0, y, 2.0),
                ..default()
            },
            Fill::color(line_color),
            LivePriceLine { price: mid_price },
            ChartEntity,
        ));

        x += dash_length + gap_length;
    }
}

/// System to spawn the "NOW" line when showing future space
pub fn spawn_now_line(
    mut commands: Commands,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    viewport: Res<ChartViewport>,
    existing_lines: Query<Entity, With<NowLine>>,
) {
    // Clear existing NOW line
    for entity in existing_lines.iter() {
        commands.entity(entity).despawn();
    }

    if viewport.future_slots == 0 {
        return;
    }

    let Some(symbol) = &chart_state.selected_instrument else {
        return;
    };
    let Some(instrument) = app_state.instruments.get(symbol) else {
        return;
    };

    let candles = instrument
        .candles
        .get(&chart_state.selected_timeframe)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    if candles.is_empty() {
        return;
    }

    let (start_idx, end_idx, _) = chart_state.visible_range(
        candles.len(),
        viewport.size.x,
        viewport.candle_width,
    );
    let visible_count = end_idx - start_idx;

    // NOW line is at the position after the last visible candle
    let now_x = viewport.index_to_x(visible_count);

    // Create dashed vertical line using multiple small rectangles
    let dash_length = 8.0;
    let gap_length = 4.0;
    let line_width = 1.5;
    let mut y = -viewport.size.y / 2.0;

    while y < viewport.size.y / 2.0 {
        let end_y = (y + dash_length).min(viewport.size.y / 2.0);
        let segment_height = end_y - y;

        let dash_shape = shapes::Rectangle {
            extents: Vec2::new(line_width, segment_height),
            origin: RectangleOrigin::Center,
            ..default()
        };

        commands.spawn((
            ShapeBundle {
                path: GeometryBuilder::build_as(&dash_shape),
                transform: Transform::from_xyz(now_x, y + segment_height / 2.0, 2.0),
                ..default()
            },
            Fill::color(chart_colors::NOW_LINE),
            NowLine,
            ChartEntity,
        ));

        y += dash_length + gap_length;
    }
}

/// System to clear all chart entities (used when switching instruments)
pub fn clear_chart_entities(
    mut commands: Commands,
    entities: Query<Entity, With<ChartEntity>>,
) {
    for entity in entities.iter() {
        commands.entity(entity).despawn_recursive();
    }
}
