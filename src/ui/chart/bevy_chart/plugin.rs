//! Bevy plugin for native chart rendering
//!
//! This plugin sets up all the systems needed for GPU-accelerated candlestick chart rendering.

use bevy::prelude::*;
use bevy_prototype_lyon::prelude::*;

use super::components::ChartViewport;
use super::systems::*;

/// Plugin that adds Bevy native chart rendering capabilities
pub struct BevyChartPlugin;

impl Plugin for BevyChartPlugin {
    fn build(&self, app: &mut App) {
        app
            // Add the lyon shape plugin for 2D rendering
            .add_plugins(ShapePlugin)
            // Initialize chart viewport resource
            .init_resource::<ChartViewport>()
            // Startup system to create chart camera
            .add_systems(Startup, setup_chart_camera)
            // Update systems in order
            .add_systems(
                Update,
                (
                    // First, update the viewport based on state
                    update_chart_viewport,
                    // Then update camera to match viewport
                    update_chart_camera,
                    // Then spawn/update entities based on new viewport
                    spawn_candle_entities,
                    spawn_time_separators,
                    spawn_volume_entities,
                    spawn_grid_entities,
                    spawn_live_price_line,
                    spawn_now_line,
                )
                    .chain(), // Run in sequence
            );
    }
}
