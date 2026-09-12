use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    FocusNext,
    FocusPrevious,
    MoveUp,
    MoveDown,
    Activate,
    Delete,
    Back,
    OpenControl,
    Interrupt,
}

pub fn action(key: KeyEvent) -> Option<UiAction> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::OpenControl)
        }
        KeyCode::Char('\0') => Some(UiAction::OpenControl),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::Interrupt)
        }
        KeyCode::Tab => Some(UiAction::FocusNext),
        KeyCode::BackTab => Some(UiAction::FocusPrevious),
        KeyCode::Up => Some(UiAction::MoveUp),
        KeyCode::Down => Some(UiAction::MoveDown),
        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => Some(UiAction::Activate),
        KeyCode::Delete => Some(UiAction::Delete),
        KeyCode::Esc => Some(UiAction::Back),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_space_opens_control() {
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert_eq!(action(key), Some(UiAction::OpenControl));
    }

    #[test]
    fn arrows_are_selection_actions() {
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(action(key), Some(UiAction::MoveDown));
    }

    #[test]
    fn horizontal_arrows_remain_available_to_the_focused_editor() {
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);

        assert_eq!(action(left), None);
        assert_eq!(action(right), None);
    }

    #[test]
    fn control_c_interrupts_active_work() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(action(key), Some(UiAction::Interrupt));
    }
}
