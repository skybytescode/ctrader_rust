//! Price update systems with change detection

use bevy::prelude::*;
use crate::ui::AppState;
use crate::ui::bevy_ui::{
    BidPrice, AskPrice, SpreadPrice, ConnectionStatusLabel,
    PriceFormatCache,
    theme::colors,
};

/// Update bid price text when prices change
pub fn update_bid_prices(
    app_state: Res<AppState>,
    mut query: Query<(&BidPrice, &mut Text)>,
    mut cache: ResMut<PriceFormatCache>,
) {
    // Only run when AppState changes
    if !app_state.is_changed() {
        return;
    }

    for (bid_price, mut text) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&bid_price.symbol) {
            let formatted = cache.format(instrument.bid, instrument.decimal_places);
            if text.0 != formatted {
                text.0 = formatted;
            }
        }
    }
}

/// Update ask price text when prices change
pub fn update_ask_prices(
    app_state: Res<AppState>,
    mut query: Query<(&AskPrice, &mut Text)>,
    mut cache: ResMut<PriceFormatCache>,
) {
    if !app_state.is_changed() {
        return;
    }

    for (ask_price, mut text) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&ask_price.symbol) {
            let formatted = cache.format(instrument.ask, instrument.decimal_places);
            if text.0 != formatted {
                text.0 = formatted;
            }
        }
    }
}

/// Update bid price colors based on tick direction
pub fn update_bid_colors(
    app_state: Res<AppState>,
    mut query: Query<(&BidPrice, &mut TextColor)>,
) {
    if !app_state.is_changed() {
        return;
    }

    for (bid_price, mut text_color) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&bid_price.symbol) {
            let new_color = if instrument.bid > instrument.prev_bid {
                colors::BULLISH
            } else if instrument.bid < instrument.prev_bid {
                colors::BEARISH
            } else {
                colors::TEXT_PRIMARY
            };

            if text_color.0 != new_color {
                text_color.0 = new_color;
            }
        }
    }
}

/// Update ask price colors based on tick direction
pub fn update_ask_colors(
    app_state: Res<AppState>,
    mut query: Query<(&AskPrice, &mut TextColor)>,
) {
    if !app_state.is_changed() {
        return;
    }

    for (ask_price, mut text_color) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&ask_price.symbol) {
            let new_color = if instrument.ask > instrument.prev_ask {
                colors::BULLISH
            } else if instrument.ask < instrument.prev_ask {
                colors::BEARISH
            } else {
                colors::TEXT_PRIMARY
            };

            if text_color.0 != new_color {
                text_color.0 = new_color;
            }
        }
    }
}

/// Update spread display when prices change
pub fn update_spread_prices(
    app_state: Res<AppState>,
    mut query: Query<(&SpreadPrice, &mut Text)>,
) {
    if !app_state.is_changed() {
        return;
    }

    for (spread_price, mut text) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&spread_price.symbol) {
            let pip_mult = if instrument.decimal_places <= 2 {
                10f64.powi(instrument.decimal_places as i32)
            } else {
                10f64.powi(instrument.decimal_places as i32 - 1)
            };
            let spread = (instrument.ask - instrument.bid) * pip_mult;
            let dp = if instrument.decimal_places <= 2 { 0 } else { 1 };
            let formatted = format!("{:.dp$}", spread, dp = dp);
            if text.0 != formatted {
                text.0 = formatted;
            }
        }
    }
}

/// Update spread color: red if spread widened, green if narrowed, white if same
pub fn update_spread_colors(
    app_state: Res<AppState>,
    mut query: Query<(&SpreadPrice, &mut TextColor)>,
) {
    if !app_state.is_changed() {
        return;
    }

    for (spread_price, mut text_color) in query.iter_mut() {
        if let Some(instrument) = app_state.instruments.get(&spread_price.symbol) {
            let current_spread = instrument.ask - instrument.bid;
            let prev_spread = instrument.prev_ask - instrument.prev_bid;

            let new_color = if current_spread > prev_spread {
                colors::BEARISH  // spread widened = bad = red/orange
            } else if current_spread < prev_spread {
                colors::BULLISH  // spread narrowed = good = green
            } else {
                colors::TEXT_PRIMARY  // unchanged = white
            };

            if text_color.0 != new_color {
                text_color.0 = new_color;
            }
        }
    }
}

/// Update connection status label
pub fn update_connection_status(
    app_state: Res<AppState>,
    mut query: Query<&mut Text, With<ConnectionStatusLabel>>,
) {
    if !app_state.is_changed() {
        return;
    }

    for mut text in query.iter_mut() {
        let new_text = format!("Status: {}", app_state.connection_status);
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Periodically clear price format cache to prevent unbounded growth
pub fn clear_price_cache(
    mut cache: ResMut<PriceFormatCache>,
    time: Res<Time>,
    mut last_clear: Local<f32>,
) {
    // Clear every 60 seconds
    if time.elapsed_secs() - *last_clear > 60.0 {
        cache.clear();
        *last_clear = time.elapsed_secs();
    }
}
