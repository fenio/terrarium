use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
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
    terminal::disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Re-enter TUI mode after a subprocess (e.g. kubectl exec).
/// Does not create a new Terminal — the existing one is reused.
pub fn init_raw(mouse: bool) -> io::Result<()> {
    if mouse {
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    terminal::enable_raw_mode()?;
    Ok(())
}

pub fn set_mouse_capture(enabled: bool) -> io::Result<()> {
    if enabled {
        execute!(stdout(), EnableMouseCapture)
    } else {
        execute!(stdout(), DisableMouseCapture)
    }
}
