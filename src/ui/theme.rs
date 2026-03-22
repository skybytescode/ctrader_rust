//! Theme constants for cTrader-style UI (egui)
//! Colors, sizing, and font definitions

/// Color palette matching cTrader dark theme
pub mod colors {
    use egui::Color32;

    // Background colors (darkest to lightest)
    pub const BG_DARKEST: Color32 = Color32::from_rgb(15, 18, 23);       // #0F1217
    pub const BG_DARK: Color32 = Color32::from_rgb(20, 22, 34);          // #141622
    pub const BG_PANEL: Color32 = Color32::from_rgb(30, 30, 35);         // #1E1E23
    pub const BG_SIDEBAR: Color32 = Color32::from_rgb(35, 38, 48);       // #232630
    pub const BG_HOVER: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 15); // White 6%
    pub const BG_SELECTED: Color32 = Color32::from_rgb(45, 50, 65);      // #2D3241
    pub const BG_BUTTON: Color32 = Color32::from_rgb(30, 30, 30);        // #1E1E1E
    pub const BG_BUTTON_ACTIVE: Color32 = Color32::from_rgb(40, 40, 60); // #28283C

    // Text colors
    pub const TEXT_PRIMARY: Color32 = Color32::WHITE;
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(153, 158, 166); // #999EA6
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(120, 120, 120);     // #787878
    pub const TEXT_LABEL: Color32 = Color32::from_rgb(150, 150, 150);     // #969696

    // Trading colors
    pub const BULLISH: Color32 = Color32::from_rgb(0, 166, 115);         // #00A673
    pub const BEARISH: Color32 = Color32::from_rgb(235, 115, 20);        // #EB7314

    // Accent and UI colors
    pub const ACCENT_BLUE: Color32 = Color32::from_rgb(80, 120, 200);    // #5078C8
    pub const BORDER: Color32 = Color32::from_rgb(60, 62, 70);           // #3C3E46
    pub const SEPARATOR: Color32 = Color32::from_rgb(38, 41, 51);        // #262933
}

/// Size constants for UI layout
pub mod sizing {
    pub const ICON_BAR_WIDTH: f32 = 50.0;
    pub const SIDEBAR_WIDTH: f32 = 280.0;
    pub const TOP_PANEL_HEIGHT: f32 = 36.0;
    pub const INSTRUMENT_ROW_HEIGHT: f32 = 32.0;
    pub const ICON_BUTTON_SIZE: f32 = 40.0;
    pub const BUTTON_PADDING: f32 = 8.0;
    pub const SPACING: f32 = 4.0;
    pub const SPACING_LARGE: f32 = 10.0;
}

/// Font size constants
pub mod fonts {
    pub const SIZE_TINY: f32 = 9.0;
    pub const SIZE_SMALL: f32 = 10.0;
    pub const SIZE_NORMAL: f32 = 12.0;
    pub const SIZE_MEDIUM: f32 = 14.0;
    pub const SIZE_LARGE: f32 = 16.0;
    pub const SIZE_HEADING: f32 = 20.0;
}

/// Apply the cTrader dark theme to an egui context
pub fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Dark background
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = colors::BG_DARKEST;
    style.visuals.window_fill = colors::BG_PANEL;
    style.visuals.extreme_bg_color = colors::BG_DARK;
    style.visuals.faint_bg_color = colors::BG_SIDEBAR;

    // Widget colors
    style.visuals.widgets.noninteractive.bg_fill = colors::BG_PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_SECONDARY);
    style.visuals.widgets.inactive.bg_fill = colors::BG_BUTTON;
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors::TEXT_PRIMARY);
    style.visuals.widgets.hovered.bg_fill = colors::BG_HOVER;
    style.visuals.widgets.active.bg_fill = colors::BG_BUTTON_ACTIVE;

    // Separator / border
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors::BORDER);

    // Spacing
    style.spacing.item_spacing = egui::vec2(sizing::SPACING, sizing::SPACING);

    ctx.set_style(style);
}
