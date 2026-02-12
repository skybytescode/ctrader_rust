//! Theme constants for cTrader-style UI
//! Colors, sizing, and font definitions

/// Color palette matching cTrader dark theme
pub mod colors {
    use bevy::prelude::Color;

    // Background colors (darkest to lightest)
    pub const BG_DARKEST: Color = Color::srgb(0.059, 0.071, 0.090);      // #0F1217
    pub const BG_DARK: Color = Color::srgb(0.078, 0.086, 0.110);         // #141622
    pub const BG_PANEL: Color = Color::srgb(0.118, 0.118, 0.137);        // #1E1E23
    pub const BG_SIDEBAR: Color = Color::srgb(0.137, 0.149, 0.188);      // #232630
    pub const BG_HOVER: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);       // White 6% overlay
    pub const BG_SELECTED: Color = Color::srgb(0.176, 0.196, 0.255);     // #2D3241
    pub const BG_BUTTON: Color = Color::srgb(0.118, 0.118, 0.118);       // #1E1E1E
    pub const BG_BUTTON_ACTIVE: Color = Color::srgb(0.157, 0.157, 0.235);// #28283C

    // Text colors
    pub const TEXT_PRIMARY: Color = Color::WHITE;
    pub const TEXT_SECONDARY: Color = Color::srgb(0.600, 0.620, 0.651);  // #999EA6
    pub const TEXT_MUTED: Color = Color::srgb(0.471, 0.471, 0.471);      // #787878
    pub const TEXT_LABEL: Color = Color::srgb(0.588, 0.588, 0.588);      // #969696

    // Trading colors
    pub const BULLISH: Color = Color::srgb(0.0, 0.651, 0.451);           // #00A673 (teal green)
    pub const BEARISH: Color = Color::srgb(0.922, 0.451, 0.078);         // #EB7314 (orange)

    // Accent and UI colors
    pub const ACCENT_BLUE: Color = Color::srgb(0.314, 0.471, 0.784);     // #5078C8
    pub const BORDER: Color = Color::srgb(0.235, 0.243, 0.275);          // #3C3E46
    pub const SEPARATOR: Color = Color::srgb(0.149, 0.161, 0.200);       // #262933

    // Axis colors
    pub const AXIS_TEXT: Color = Color::srgb(0.600, 0.620, 0.651);       // #999EA6
    pub const GRID_LINE: Color = Color::srgb(0.149, 0.161, 0.200);       // #262933
}

/// Size constants for UI layout
pub mod sizing {
    /// Width of the narrow icon bar (leftmost)
    pub const ICON_BAR_WIDTH: f32 = 50.0;

    /// Width of the expandable sidebar
    pub const SIDEBAR_WIDTH: f32 = 280.0;

    /// Height of the top panel (title bar)
    pub const TOP_PANEL_HEIGHT: f32 = 36.0;

    /// Height of the chart toolbar
    pub const TOOLBAR_HEIGHT: f32 = 32.0;

    /// Width of the price axis (right side)
    pub const PRICE_AXIS_WIDTH: f32 = 80.0;

    /// Height of the time axis (bottom)
    pub const TIME_AXIS_HEIGHT: f32 = 30.0;

    /// Height of the horizontal scrollbar
    pub const SCROLLBAR_HEIGHT: f32 = 20.0;

    /// Height of each instrument row in watchlist
    pub const INSTRUMENT_ROW_HEIGHT: f32 = 32.0;

    /// Height of tab buttons
    pub const TAB_HEIGHT: f32 = 28.0;

    /// Size of icon buttons in icon bar
    pub const ICON_BUTTON_SIZE: f32 = 40.0;

    /// Standard button padding
    pub const BUTTON_PADDING: f32 = 8.0;

    /// Standard spacing between elements
    pub const SPACING: f32 = 4.0;

    /// Larger spacing for section separation
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
