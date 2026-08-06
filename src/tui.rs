use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io::{self, stdout};

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

pub fn init(mouse: bool) -> io::Result<Tui> {
    if mouse {
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    terminal::enable_raw_mode()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

pub fn restore() -> io::Result<()> {
    // Hand stderr back to the real terminal so an intentional subprocess
    // (OIDC browser login, tfctl) can print to the screen.
    crate::logging::stderr_to_terminal();
    terminal::disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Re-enter TUI mode after suspending for a subprocess (kubectl exec, an
/// OIDC browser login, a context switch, …) and force a clean full redraw.
///
/// We rebuild the `Terminal` rather than calling `terminal.clear()`.
/// ratatui's `clear()` first issues a DSR cursor-position query, which can
/// fail or race right after resuming (the subprocess may have left the tty
/// in an odd state). When it fails, `clear()` returns early *without*
/// resetting ratatui's diff buffer, so the next frame diffs against the
/// pre-suspend frame — static cells (borders, panel titles, the logo, tab
/// boxes) are treated as unchanged and never repainted, leaving a
/// half-drawn UI. A fresh `Terminal` starts with an empty back buffer, so
/// the next draw is guaranteed to repaint every cell.
pub fn resume(terminal: &mut Tui, mouse: bool) -> io::Result<()> {
    if mouse {
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    terminal::enable_raw_mode()?;
    // Clear the physical screen (no cursor round-trip), then swap in a fresh
    // Terminal whose empty back buffer forces a complete repaint next draw.
    execute!(stdout(), Clear(ClearType::All))?;
    *terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    // Re-capture child-process stderr into the log now that we're back on the
    // alternate screen.
    crate::logging::stderr_to_log();
    Ok(())
}

pub fn set_mouse_capture(enabled: bool) -> io::Result<()> {
    if enabled {
        execute!(stdout(), EnableMouseCapture)
    } else {
        execute!(stdout(), DisableMouseCapture)
    }
}
