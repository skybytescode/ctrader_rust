//! Volume-by-price profile for XAUUSD intraday trading.
//!
//! Builds a volume histogram (tick-volume binned by price) and extracts the
//! Point of Control (POC = highest-volume price) and the 70% Value Area
//! (VAH/VAL) for each trading day and for the this-week / last-week composites.
//! Each level is tagged with provenance ("Today", "Yesterday", "Tue (last wk)")
//! so the chart and the volume agent can tell where a level came from.

use crate::db::Candle;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use serde::Serialize;
use std::collections::BTreeMap;

/// One drawable volume level.
#[derive(Serialize, Clone, Debug)]
pub struct VolumeLevel {
    pub price: f64,
    pub kind: String,   // "POC" | "VAH" | "VAL"
    pub scope: String,  // "today" | "day" | "week"
    pub week: String,   // "this" | "last"
    pub label: String,  // provenance, e.g. "Today", "Yesterday", "Tue (last wk)", "This week"
    pub volume: f64,    // bucket volume for a POC (relative strength); 0 for VAH/VAL
}

struct Profile {
    poc: f64,
    vah: f64,
    val: f64,
    poc_vol: f64,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Build a volume profile from a set of bars: bin each bar's volume into a
/// price bucket (by typical price HLC3), find the POC, then expand outward from
/// the POC until 70% of the total volume is enclosed → Value Area High/Low.
fn build_profile(bars: &[Candle], bucket: f64) -> Option<Profile> {
    if bars.is_empty() || bucket <= 0.0 {
        return None;
    }
    let key = |p: f64| (p / bucket).round() as i64;
    let mut hist: BTreeMap<i64, f64> = BTreeMap::new();
    let mut total = 0.0;
    for b in bars {
        let typical = (b.high + b.low + b.close) / 3.0;
        let v = b.volume.max(0) as f64;
        *hist.entry(key(typical)).or_insert(0.0) += v;
        total += v;
    }
    if total <= 0.0 || hist.is_empty() {
        return None;
    }

    let keys: Vec<i64> = hist.keys().copied().collect();
    let vol_at = |k: i64| *hist.get(&k).unwrap_or(&0.0);

    // POC = bucket with the most volume.
    let poc_k = *keys
        .iter()
        .max_by(|a, b| vol_at(**a).partial_cmp(&vol_at(**b)).unwrap())
        .unwrap();
    let poc_idx = keys.iter().position(|&k| k == poc_k).unwrap();
    let poc_vol = vol_at(poc_k);

    // Expand the value area from the POC, always taking the heavier adjacent
    // bucket, until ≥70% of total volume is covered.
    let target = total * 0.70;
    let mut lo = poc_idx;
    let mut hi = poc_idx;
    let mut acc = poc_vol;
    while acc < target && (lo > 0 || hi < keys.len() - 1) {
        let down = if lo > 0 { vol_at(keys[lo - 1]) } else { -1.0 };
        let up = if hi < keys.len() - 1 { vol_at(keys[hi + 1]) } else { -1.0 };
        if up >= down && hi < keys.len() - 1 {
            hi += 1;
            acc += vol_at(keys[hi]);
        } else if lo > 0 {
            lo -= 1;
            acc += vol_at(keys[lo]);
        } else if hi < keys.len() - 1 {
            hi += 1;
            acc += vol_at(keys[hi]);
        } else {
            break;
        }
    }

    let price_of = |k: i64| k as f64 * bucket;
    Some(Profile {
        poc: round2(price_of(poc_k)),
        vah: round2(price_of(keys[hi])),
        val: round2(price_of(keys[lo])),
        poc_vol,
    })
}

fn weekday_short(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

/// Compute all volume levels for the this-week + last-week window:
///   - Per-day POC for every day (provenance-tagged).
///   - Full POC + VAH + VAL for today (developing) and yesterday.
///   - Full POC + VAH + VAL composites for this week and last week.
/// `bars` should be M5 (or finer) candles spanning ~2 calendar weeks.
pub fn compute_levels(bars: &[Candle], now: DateTime<Utc>, bucket: f64) -> Vec<VolumeLevel> {
    let today = now.date_naive();
    let yesterday = today - Duration::days(1);
    let this_mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let last_mon = this_mon - Duration::days(7);

    // Group bars by UTC calendar day.
    let mut by_day: BTreeMap<NaiveDate, Vec<Candle>> = BTreeMap::new();
    for b in bars {
        if let Some(dt) = DateTime::<Utc>::from_timestamp(b.timestamp, 0) {
            by_day.entry(dt.date_naive()).or_default().push(*b);
        }
    }

    let mut out = Vec::new();
    let mut this_week: Vec<Candle> = Vec::new();
    let mut last_week: Vec<Candle> = Vec::new();

    for (&d, day_bars) in &by_day {
        let week = if d >= this_mon {
            "this"
        } else if d >= last_mon {
            "last"
        } else {
            continue; // older than the 2-week window
        };
        if week == "this" {
            this_week.extend(day_bars.iter().copied());
        } else {
            last_week.extend(day_bars.iter().copied());
        }

        let Some(p) = build_profile(day_bars, bucket) else { continue };
        let is_today = d == today;
        let full = is_today || d == yesterday; // VAH/VAL only for today + yesterday
        let label = if is_today {
            "Today".to_string()
        } else if d == yesterday {
            "Yesterday".to_string()
        } else if week == "last" {
            format!("{} (last wk)", weekday_short(d.weekday()))
        } else {
            weekday_short(d.weekday()).to_string()
        };
        let scope = if is_today { "today" } else { "day" };

        out.push(VolumeLevel {
            price: p.poc, kind: "POC".into(), scope: scope.into(),
            week: week.into(), label: label.clone(), volume: p.poc_vol,
        });
        if full {
            out.push(VolumeLevel {
                price: p.vah, kind: "VAH".into(), scope: scope.into(),
                week: week.into(), label: label.clone(), volume: 0.0,
            });
            out.push(VolumeLevel {
                price: p.val, kind: "VAL".into(), scope: scope.into(),
                week: week.into(), label, volume: 0.0,
            });
        }
    }

    // Weekly composites (POC + value area).
    for (wbars, wk, lbl) in [
        (&this_week, "this", "This week"),
        (&last_week, "last", "Last week"),
    ] {
        if let Some(p) = build_profile(wbars, bucket) {
            out.push(VolumeLevel { price: p.poc, kind: "POC".into(), scope: "week".into(), week: wk.into(), label: lbl.into(), volume: p.poc_vol });
            out.push(VolumeLevel { price: p.vah, kind: "VAH".into(), scope: "week".into(), week: wk.into(), label: lbl.into(), volume: 0.0 });
            out.push(VolumeLevel { price: p.val, kind: "VAL".into(), scope: "week".into(), week: wk.into(), label: lbl.into(), volume: 0.0 });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(ts: i64, price: f64, vol: i64) -> Candle {
        Candle::new(ts, price, price + 0.2, price - 0.2, price, vol)
    }

    #[test]
    fn poc_is_highest_volume_price() {
        // Most volume traded at 2000 → POC ≈ 2000.
        let mut bars = vec![bar(0, 1995.0, 100), bar(60, 2005.0, 100)];
        for i in 0..20 {
            bars.push(bar(120 + i * 60, 2000.0, 500));
        }
        let p = build_profile(&bars, 0.5).unwrap();
        assert!((p.poc - 2000.0).abs() < 1.0, "poc {}", p.poc);
        assert!(p.vah >= p.poc && p.val <= p.poc);
    }

    #[test]
    fn empty_or_zero_volume_is_none() {
        assert!(build_profile(&[], 0.5).is_none());
        assert!(build_profile(&[bar(0, 2000.0, 0)], 0.5).is_none());
    }
}
