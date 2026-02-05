//! Bevy native chart rendering module
//!
//! This module provides GPU-accelerated candlestick chart rendering using Bevy's
//! native 2D rendering system instead of egui's immediate mode rendering.
//!
//! ## Architecture
//!
//! - **Components**: ECS components for chart elements (candles, grid, volume bars)
//! - **Systems**: Bevy systems that spawn and update chart entities
//! - **Plugin**: Bundles everything into a reusable Bevy plugin
//!
//! ## Benefits over egui rendering
//!
//! 1. **GPU-accelerated**: Shapes are rendered using GPU batching
//! 2. **Parallel systems**: Multiple systems can update chart elements in parallel
//! 3. **Entity reuse**: Changed entities are updated in-place, not regenerated
//! 4. **Scalable**: Can render thousands of candles efficiently

pub mod components;
pub mod systems;
pub mod plugin;

pub use components::*;
pub use plugin::BevyChartPlugin;
pub use systems::*;
