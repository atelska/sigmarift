use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect, style::Style};
use tui_textarea::{TextArea, WrapMode};

use super::theme;

/// Shared textarea adapter for SigmaRift's chat input and future long-form editors.
pub struct TextEditor {
    textarea: TextArea<'static>,
}

impl TextEditor {
    pub fn chat() -> Self {
        let mut textarea = Self::textarea();
        textarea.set_placeholder_text("Type a message. Enter sends, LeftAlt+Enter adds a line.");
        textarea.set_placeholder_style(Style::default().fg(theme::MUTED_TEXT));
        Self { textarea }
    }

    pub fn editor(content: &str) -> Self {
        let mut textarea = Self::textarea();
        textarea.set_lines(content.split('\n').map(str::to_owned).collect(), (0, 0));
        Self { textarea }
    }

    fn textarea() -> TextArea<'static> {
        let mut textarea = TextArea::default();
        textarea.set_style(
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND),
        );
        textarea.set_cursor_style(
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN),
        );
        textarea.set_cursor_line_style(Style::default());
        textarea.set_wrap_mode(WrapMode::WordOrGlyph);
        textarea
    }

    pub fn input(&mut self, key: KeyEvent) -> bool {
        self.textarea.input(key)
    }

    pub fn is_empty(&self) -> bool {
        self.textarea
            .lines()
            .iter()
            .all(|line| line.trim().is_empty())
    }

    pub fn take_content(&mut self) -> String {
        let content = self.textarea.lines().join("\n");
        self.textarea.set_lines(vec![String::new()], (0, 0));
        content
    }

    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(&self.textarea, area);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn editor_inserts_at_the_cursor() {
        let mut editor = TextEditor::chat();
        editor.input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        editor.input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        editor.input(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        editor.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(editor.take_content(), "axb");
    }

    #[test]
    fn alt_enter_creates_a_new_line() {
        let mut editor = TextEditor::chat();
        editor.input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        editor.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        editor.input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));

        assert_eq!(editor.take_content(), "a\nb");
    }
}
