use ratatui::style::{Color, Modifier, Style};

// Header bar
pub const HEADER_BAR_BG: Color = Color::Rgb(18, 18, 28);
pub const HEADER_LOGO: Style = Style::new()
    .fg(Color::Rgb(100, 180, 255))
    .bg(Color::Rgb(18, 18, 28))
    .add_modifier(Modifier::BOLD);
pub const HEADER_CONTEXT_LABEL: Style = Style::new()
    .fg(Color::Rgb(80, 85, 100))
    .bg(Color::Rgb(18, 18, 28));
pub const HEADER_CONTEXT: Style = Style::new()
    .fg(Color::Rgb(30, 30, 40))
    .bg(Color::Rgb(100, 220, 140))
    .add_modifier(Modifier::BOLD);
pub const HEADER_NS_LABEL: Style = Style::new()
    .fg(Color::Rgb(80, 85, 100))
    .bg(Color::Rgb(18, 18, 28));
pub const HEADER_NS: Style = Style::new()
    .fg(Color::Rgb(30, 30, 40))
    .bg(Color::Rgb(140, 180, 220))
    .add_modifier(Modifier::BOLD);
pub const HEADER_GECKO: Style = Style::new()
    .fg(Color::Rgb(100, 220, 140))
    .bg(Color::Rgb(18, 18, 28))
    .add_modifier(Modifier::BOLD);

// Column headers
pub const COLUMN_HEADER: Style = Style::new()
    .fg(Color::Rgb(160, 170, 200))
    .bg(Color::Rgb(35, 38, 52))
    .add_modifier(Modifier::BOLD);

// Labels (keys like "Deployment:", "Namespace:", etc.)
pub const LABEL: Style = Style::new().fg(Color::Rgb(140, 145, 165));

// Status colors
pub const STATUS_READY: Style = Style::new().fg(Color::Rgb(80, 220, 100));
pub const STATUS_NOT_READY: Style = Style::new().fg(Color::Rgb(240, 80, 80));
pub const STATUS_PENDING: Style = Style::new().fg(Color::Rgb(240, 200, 60));
pub const STATUS_UNKNOWN: Style = Style::new().fg(Color::Rgb(140, 145, 165));
/// Ready=False with reason=Progressing — controller is actively
/// reconciling, not a real failure. Muted cyan distinguishes it from
/// the alarming red used for true failures.
pub const STATUS_RECONCILING: Style = Style::new().fg(Color::Rgb(120, 200, 230));

// Table rows
pub const SELECTED_ROW: Style = Style::new()
    .bg(Color::Rgb(45, 50, 70))
    .add_modifier(Modifier::BOLD);

// Flash messages
pub const FLASH_SUCCESS: Style = Style::new()
    .fg(Color::Rgb(80, 220, 100))
    .add_modifier(Modifier::BOLD);
pub const FLASH_ERROR: Style = Style::new()
    .fg(Color::Rgb(240, 80, 80))
    .add_modifier(Modifier::BOLD);

// Dialog
pub const DIALOG_BORDER: Style = Style::new().fg(Color::Rgb(240, 200, 60));

// Special values
pub const SUSPENDED: Style = Style::new().fg(Color::Rgb(240, 200, 60));
/// Bulk-selection marker in the leftmost list column. Distinct enough
/// from SUSPENDED's yellow that the two indicators don't run together.
pub const BULK_SELECTED: Style = Style::new()
    .fg(Color::Rgb(120, 200, 230))
    .add_modifier(Modifier::BOLD);
/// Marker for rows that were just acted on (recently dispatched
/// reconcile / suspend / approve / …). Muted slate-blue so it doesn't
/// compete visually with the bulk-selection marker.
pub const RECENTLY_ACTED: Style = Style::new().fg(Color::Rgb(140, 160, 200));
/// Marker for rows whose underlying resource has a non-empty
/// `metadata.deletionTimestamp` — i.e. delete was requested but the
/// finalizer chain hasn't completed yet. Bright red so it pops out of
/// the list at a glance.
pub const DELETING: Style = Style::new()
    .fg(Color::Rgb(240, 100, 100))
    .add_modifier(Modifier::BOLD);

// Plan syntax highlighting
pub const PLAN_CREATE: Style = Style::new().fg(Color::Rgb(80, 220, 100));
pub const PLAN_DESTROY: Style = Style::new().fg(Color::Rgb(240, 80, 80));
pub const PLAN_CHANGE: Style = Style::new().fg(Color::Rgb(240, 200, 60));
pub const PLAN_READ: Style = Style::new().fg(Color::Rgb(140, 200, 255));

// JSON syntax highlighting
pub const JSON_KEY: Style = Style::new()
    .fg(Color::Rgb(140, 200, 255))
    .add_modifier(Modifier::BOLD);
pub const JSON_STRING: Style = Style::new().fg(Color::Rgb(180, 230, 140));
pub const JSON_NUMBER: Style = Style::new().fg(Color::Rgb(240, 180, 100));
pub const JSON_BOOL: Style = Style::new().fg(Color::Rgb(240, 140, 200));
pub const JSON_NULL: Style = Style::new().fg(Color::Rgb(140, 145, 165));
pub const JSON_BRACE: Style = Style::new().fg(Color::Rgb(200, 200, 220));

// Status bar
pub const STATUS_BAR_BG: Color = Color::Rgb(25, 25, 35);
pub const STATUS_BAR_KEY: Style = Style::new()
    .fg(Color::Rgb(140, 200, 255))
    .bg(Color::Rgb(25, 25, 35))
    .add_modifier(Modifier::BOLD);
pub const STATUS_BAR_TEXT: Style = Style::new()
    .fg(Color::Rgb(140, 145, 165))
    .bg(Color::Rgb(25, 25, 35));
pub const STATUS_BAR_SEP: Style = Style::new()
    .fg(Color::Rgb(60, 65, 80))
    .bg(Color::Rgb(25, 25, 35));

// Panel chrome (borders, block titles). Borders are bright enough to
// read on a dark terminal without competing with the content; titles
// pop a step brighter so they read as headings. (LABEL is defined
// above and reused for kv keys here too.)
pub const BORDER: Style = Style::new().fg(Color::Rgb(90, 130, 190));
pub const BLOCK_TITLE: Style = Style::new()
    .fg(Color::Rgb(140, 200, 255))
    .add_modifier(Modifier::BOLD);
pub const INLINE_SEP: Style = Style::new().fg(Color::Rgb(70, 80, 100));
