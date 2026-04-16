use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Action;
use crate::state::store::{InputMode, ViewState};

pub fn handle_key(key: KeyEvent, view: &ViewState, input_mode: &InputMode) -> Action {
    // Ctrl-C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }

    // Help mode
    if matches!(input_mode, InputMode::Help) {
        return match key.code {
            KeyCode::Char('?') | KeyCode::Esc => Action::ToggleHelp,
            _ => Action::None,
        };
    }

    // List search mode
    if matches!(input_mode, InputMode::Search) {
        return match key.code {
            KeyCode::Esc => Action::SearchCancel,
            KeyCode::Enter => Action::SearchConfirm,
            KeyCode::Backspace => Action::SearchPop,
            KeyCode::Char(c) => Action::SearchPush(c),
            _ => Action::None,
        };
    }

    // Viewer search mode
    if matches!(input_mode, InputMode::ViewerSearch) {
        return match key.code {
            KeyCode::Esc => Action::ViewerSearchCancel,
            KeyCode::Enter => Action::ViewerSearchConfirm,
            KeyCode::Backspace => Action::ViewerSearchPop,
            KeyCode::Char(c) => Action::ViewerSearchPush(c),
            _ => Action::None,
        };
    }

    // Confirm dialog
    if matches!(input_mode, InputMode::Confirm) {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Enter => Action::ConfirmDialog(true),
            KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmDialog(false),
            _ => Action::None,
        };
    }

    // Namespace picker
    if matches!(input_mode, InputMode::NamespacePicker) {
        return match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::NamespacePickerNext,
            KeyCode::Char('k') | KeyCode::Up => Action::NamespacePickerPrev,
            KeyCode::Enter => Action::NamespacePickerSelect,
            KeyCode::Esc => Action::NamespacePickerCancel,
            _ => Action::None,
        };
    }

    // Normal mode
    match view {
        ViewState::List(_) => handle_list_key(key),
        ViewState::TerraformDetail { .. } | ViewState::KustomizationDetail { .. } => {
            handle_detail_key(key)
        }
        ViewState::PlanViewer { .. } | ViewState::JsonViewer { .. }
        | ViewState::EventsViewer { .. } | ViewState::OutputsViewer { .. }
        | ViewState::LogViewer { .. } => {
            handle_viewer_key(key)
        }
    }
}

fn handle_list_key(key: KeyEvent) -> Action {
    // Ctrl-d / Ctrl-u for page scroll in lists
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('d') => Action::PageDown,
            KeyCode::Char('u') => Action::PageUp,
            _ => Action::None,
        };
    }

    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('j') | KeyCode::Down => Action::SelectNext,
        KeyCode::Char('k') | KeyCode::Up => Action::SelectPrev,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Enter | KeyCode::Char('l') => Action::Enter,
        KeyCode::Tab => Action::NextTab,
        KeyCode::BackTab => Action::PrevTab,
        KeyCode::Char('1') => Action::GoToTab(0),
        KeyCode::Char('2') => Action::GoToTab(1),
        KeyCode::Char('3') => Action::GoToTab(2),
        KeyCode::Char('4') => Action::GoToTab(3),
        KeyCode::Char('5') => Action::GoToTab(4),
        KeyCode::Char('/') => Action::SearchStart,
        KeyCode::Char('f') => Action::ToggleFailuresOnly,
        KeyCode::Char('w') => Action::ToggleWaitingOnly,
        KeyCode::Char('o') => Action::CycleSort,
        KeyCode::Char('i') => Action::InvertSort,
        KeyCode::Char('n') => Action::OpenNamespacePicker,
        KeyCode::Char('g') => Action::ScrollTop,
        KeyCode::Char('G') => Action::ScrollBottom,
        KeyCode::Char('!') => Action::JumpToFirstFailure,
        KeyCode::Char(' ') => Action::ToggleSelect,
        KeyCode::Char('m') => Action::ToggleMouse,
        KeyCode::Char('M') => Action::ToggleMetrics,
        // Context-dependent actions resolved in app.rs
        KeyCode::Char('a') | KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Char('s')
        | KeyCode::Char('u') | KeyCode::Char('d') | KeyCode::Char('p') | KeyCode::Char('F')
        | KeyCode::Char('y') | KeyCode::Char('e')
        | KeyCode::Char('O') | KeyCode::Char('x') | KeyCode::Char('L') => Action::None,
        KeyCode::Esc => Action::Back,
        _ => Action::None,
    }
}

fn handle_detail_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Back,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('m') => Action::ToggleMouse,
        // Context-dependent actions resolved in app.rs
        KeyCode::Char('a') | KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Char('s')
        | KeyCode::Char('u') | KeyCode::Char('p') | KeyCode::Char('F')
        | KeyCode::Char('y') | KeyCode::Char('e')
        | KeyCode::Char('O') | KeyCode::Char('x') => Action::None,
        _ => Action::None,
    }
}

fn handle_viewer_key(key: KeyEvent) -> Action {
    // Ctrl-d / Ctrl-u for half-page scroll
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('d') => Action::PageDown,
            KeyCode::Char('u') => Action::PageUp,
            _ => Action::None,
        };
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Back,
        KeyCode::Char('j') | KeyCode::Down => Action::SelectNext,
        KeyCode::Char('k') | KeyCode::Up => Action::SelectPrev,
        KeyCode::Char('h') | KeyCode::Left => Action::ScrollLeft,
        KeyCode::Char('l') | KeyCode::Right => Action::ScrollRight,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Char('g') => Action::ScrollTop,
        KeyCode::Char('G') => Action::ScrollBottom,
        KeyCode::Char('w') => Action::ToggleWrap,
        KeyCode::Char('/') => Action::ViewerSearchStart,
        KeyCode::Char('n') => Action::ViewerSearchNext,
        KeyCode::Char('N') => Action::ViewerSearchPrev,
        KeyCode::Char('S') => Action::SaveViewerContent,
        KeyCode::Tab => Action::NextContainer,
        KeyCode::BackTab => Action::PrevContainer,
        _ => Action::None,
    }
}
