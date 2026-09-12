use crate::control::AppSnapshot;

use super::text_editor::TextEditor;

/// The only keyboard-focusable regions on the main work surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainFocus {
    Sessions,
    Session,
    Input,
}

/// UI cursor state for the session rail. It never owns a session object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCursor {
    Session(String),
    NewSession,
}

/// The topmost modal currently owning keyboard input.
pub enum Overlay {
    Control {
        cursor: usize,
    },
    ConfirmDelete(String),
    LlmParameters {
        cursor: usize,
        advanced: bool,
        draft: Option<TextEditor>,
    },
    AdditionalInstructions {
        editor: TextEditor,
    },
    Profiles {
        cursor: usize,
        names: Vec<String>,
    },
    Error {
        title: String,
        message: String,
        exit_on_close: bool,
    },
}

/// Presentation state that may change without mutating the application.
pub struct UiState {
    pub focus: MainFocus,
    pub session_cursor: SessionCursor,
    pub overlay: Option<Overlay>,
    pub input: TextEditor,
    transcript_offset: usize,
    transcript_follow_bottom: bool,
    transcript_content_lines: usize,
    transcript_viewport_lines: usize,
    session_list_offset: usize,
}

impl UiState {
    pub fn new(snapshot: &AppSnapshot) -> Self {
        Self {
            focus: MainFocus::Sessions,
            overlay: None,
            input: TextEditor::chat(),
            transcript_offset: 0,
            transcript_follow_bottom: true,
            transcript_content_lines: 0,
            transcript_viewport_lines: 0,
            session_list_offset: 0,
            session_cursor: snapshot
                .active_session
                .as_ref()
                .map(|session| session.id.clone())
                .or_else(|| snapshot.sessions.first().map(|session| session.id.clone()))
                .map(SessionCursor::Session)
                .unwrap_or(SessionCursor::NewSession),
        }
    }

    /// Keeps a cursor on its stable ID across snapshot updates.
    pub fn sync_sessions(&mut self, snapshot: &AppSnapshot) {
        let cursor_is_live = match &self.session_cursor {
            SessionCursor::Session(id) => snapshot.sessions.iter().any(|session| session.id == *id),
            SessionCursor::NewSession => true,
        };
        if cursor_is_live {
            return;
        }

        self.session_cursor = snapshot
            .active_session
            .as_ref()
            .map(|session| session.id.clone())
            .or_else(|| snapshot.sessions.first().map(|session| session.id.clone()))
            .map(SessionCursor::Session)
            .unwrap_or(SessionCursor::NewSession);
    }

    pub fn focus_next(&mut self) {
        self.focus = match self.focus {
            MainFocus::Sessions => MainFocus::Session,
            MainFocus::Session => MainFocus::Input,
            MainFocus::Input => MainFocus::Sessions,
        };
    }

    pub fn focus_previous(&mut self) {
        self.focus = match self.focus {
            MainFocus::Sessions => MainFocus::Input,
            MainFocus::Session => MainFocus::Sessions,
            MainFocus::Input => MainFocus::Session,
        };
    }

    pub fn move_session_cursor(&mut self, snapshot: &AppSnapshot, down: bool) {
        let session_ids = snapshot
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>();
        let current_index = match &self.session_cursor {
            SessionCursor::Session(id) => session_ids
                .iter()
                .position(|session_id| *session_id == id)
                .map(|index| index + 1)
                .unwrap_or(0),
            SessionCursor::NewSession => 0,
        };
        let next_index = if down {
            (current_index + 1).min(session_ids.len())
        } else {
            current_index.saturating_sub(1)
        };

        self.session_cursor = if next_index == 0 {
            SessionCursor::NewSession
        } else {
            SessionCursor::Session(session_ids[next_index - 1].to_owned())
        };
    }

    /// Keeps the selected stored session visible without rendering a scrollbar.
    pub fn sync_session_list_scroll(&mut self, selected: Option<usize>, viewport_rows: usize) {
        let Some(selected) = selected else {
            return;
        };
        if viewport_rows == 0 {
            return;
        }
        if selected < self.session_list_offset {
            self.session_list_offset = selected;
        } else if selected >= self.session_list_offset + viewport_rows {
            self.session_list_offset = selected + 1 - viewport_rows;
        }
    }

    pub fn session_list_offset(&self) -> usize {
        self.session_list_offset
    }

    /// Synchronizes the visual transcript bounds after every render, following streamed output
    /// only while the user has not deliberately moved away from the bottom.
    pub fn sync_transcript(&mut self, content_lines: usize, viewport_lines: usize) {
        self.transcript_content_lines = content_lines;
        self.transcript_viewport_lines = viewport_lines;
        let last_offset = content_lines.saturating_sub(viewport_lines);
        if self.transcript_follow_bottom {
            self.transcript_offset = last_offset;
        } else {
            self.transcript_offset = self.transcript_offset.min(last_offset);
        }
    }

    pub fn scroll_transcript(&mut self, down: bool) {
        let last_offset = self
            .transcript_content_lines
            .saturating_sub(self.transcript_viewport_lines);
        if down {
            self.transcript_offset = (self.transcript_offset + 1).min(last_offset);
            // Treat the final line before the end as bottom so a small downward correction
            // resumes streaming follow behavior.
            self.transcript_follow_bottom = last_offset.saturating_sub(self.transcript_offset) <= 1;
            if self.transcript_follow_bottom {
                self.transcript_offset = last_offset;
            }
        } else {
            self.transcript_offset = self.transcript_offset.saturating_sub(1);
            self.transcript_follow_bottom = last_offset == 0;
        }
    }

    pub fn transcript_offset(&self) -> u16 {
        self.transcript_offset.min(u16::MAX as usize) as u16
    }

    pub fn transcript_scrolls(&self) -> bool {
        self.transcript_content_lines > self.transcript_viewport_lines
    }

    pub fn transcript_scrollbar_state(&self) -> (usize, usize, usize) {
        (
            self.transcript_content_lines,
            self.transcript_offset,
            self.transcript_viewport_lines,
        )
    }

    pub fn follow_transcript_bottom(&mut self) {
        self.transcript_follow_bottom = true;
    }

    /// Replaces the active UI context with a small, user-dismissible error dialog.
    pub fn show_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.overlay = Some(Overlay::Error {
            title: title.into(),
            message: message.into(),
            exit_on_close: false,
        });
    }

    pub fn show_startup_error(&mut self, message: impl Into<String>) {
        self.overlay = Some(Overlay::Error {
            title: " STARTUP ERROR ".to_owned(),
            message: message.into(),
            exit_on_close: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::SessionSummary;

    #[test]
    fn focus_cycle_follows_the_locked_main_screen_order() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);

        state.focus_next();
        assert_eq!(state.focus, MainFocus::Session);
        state.focus_next();
        assert_eq!(state.focus, MainFocus::Input);
        state.focus_next();
        assert_eq!(state.focus, MainFocus::Sessions);
    }

    #[test]
    fn session_cursor_moves_from_the_new_action_to_the_first_session() {
        let snapshot = AppSnapshot {
            sessions: vec![
                SessionSummary {
                    id: "first".to_owned(),
                    title: "First".to_owned(),
                },
                SessionSummary {
                    id: "second".to_owned(),
                    title: "Second".to_owned(),
                },
            ],
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);
        state.session_cursor = SessionCursor::NewSession;

        state.move_session_cursor(&snapshot, true);
        assert_eq!(
            state.session_cursor,
            SessionCursor::Session("first".to_owned())
        );
    }

    #[test]
    fn session_cursor_falls_back_to_the_active_session_after_its_session_is_deleted() {
        let before = AppSnapshot {
            sessions: vec![
                SessionSummary {
                    id: "first".to_owned(),
                    title: "First".to_owned(),
                },
                SessionSummary {
                    id: "second".to_owned(),
                    title: "Second".to_owned(),
                },
            ],
            active_session: None,
        };
        let mut state = UiState::new(&before);
        state.session_cursor = SessionCursor::Session("first".to_owned());
        let after = AppSnapshot {
            sessions: vec![SessionSummary {
                id: "second".to_owned(),
                title: "Second".to_owned(),
            }],
            active_session: None,
        };

        state.sync_sessions(&after);
        assert_eq!(
            state.session_cursor,
            SessionCursor::Session("second".to_owned())
        );
    }

    #[test]
    fn transcript_keeps_manual_position_when_content_grows() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);
        state.sync_transcript(20, 5);
        state.scroll_transcript(false);
        state.sync_transcript(24, 5);

        assert_eq!(state.transcript_offset(), 14);
    }

    #[test]
    fn transcript_resumes_following_at_the_bottom() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);
        state.sync_transcript(20, 5);
        state.scroll_transcript(false);
        state.scroll_transcript(true);
        state.sync_transcript(24, 5);

        assert_eq!(state.transcript_offset(), 19);
    }

    #[test]
    fn end_of_transcript_resumes_following_output() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);
        state.sync_transcript(20, 5);
        state.scroll_transcript(false);
        state.follow_transcript_bottom();
        state.sync_transcript(24, 5);

        assert_eq!(state.transcript_offset(), 19);
    }

    #[test]
    fn stored_session_list_scrolls_without_a_scrollbar() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);

        state.sync_session_list_scroll(Some(5), 3);
        assert_eq!(state.session_list_offset(), 3);
        state.sync_session_list_scroll(Some(2), 3);
        assert_eq!(state.session_list_offset(), 2);
    }

    #[test]
    fn error_overlay_replaces_the_active_ui_context() {
        let snapshot = AppSnapshot {
            sessions: Vec::new(),
            active_session: None,
        };
        let mut state = UiState::new(&snapshot);
        state.overlay = Some(Overlay::Control { cursor: 2 });

        state.show_error(" MODEL ERROR ", "request failed");

        assert!(matches!(
            state.overlay,
            Some(Overlay::Error {
                ref title,
                ref message,
                exit_on_close: false,
            }) if title == " MODEL ERROR " && message == "request failed"
        ));
    }
}
