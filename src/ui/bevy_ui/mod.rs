//! Pure Bevy UI implementation for cTrader Rust Terminal
//!
//! This module replaces egui with native Bevy UI for better performance:
//! - Retained mode: entities persist, only changed elements update
//! - ECS change detection: systems run only when data changes
//! - Virtualized scrolling: only visible rows are rendered

pub mod theme;
pub mod components;
pub mod resources;
pub mod plugin;
pub mod systems;
pub mod widgets;

pub use plugin::BevyUiPlugin;
pub use components::*;
pub use resources::*;
