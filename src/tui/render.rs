use std::borrow::Cow;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use tui_markdown::{Options, StyleSheet, from_str_with_options};
use unicode_width::UnicodeWidthChar;

use crate::{
    control::{AppSnapshot, RuntimeView},
    model::SamplingSettings,
};

use super::{
    components,
    state::{MainFocus, Overlay, SessionCursor, UiState},
    text_editor::TextEditor,
    theme,
};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    runtime: &RuntimeView,
    snapshot: &AppSnapshot,
    ui_state: &mut UiState,
    sampling: &SamplingSettings,
    turn_active: bool,
    models: Option<&[String]>,
    model_cursor: Option<usize>,
) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::MAIN_BACKGROUND)),
        area,
    );

    if let Some(models) = models {
        draw_model_select(frame, models, model_cursor.unwrap_or(0));
        return;
    }

    draw_main(frame, runtime, snapshot, ui_state, turn_active);
    match &ui_state.overlay {
        Some(Overlay::Control { cursor }) => draw_control(frame, *cursor),
        Some(Overlay::ConfirmDelete(id)) => draw_delete_confirmation(frame, snapshot, id),
        Some(Overlay::LlmParameters {
            cursor,
            advanced,
            draft,
        }) => draw_llm_parameters(frame, sampling, *cursor, *advanced, draft.as_ref()),
        Some(Overlay::AdditionalInstructions { editor }) => {
            draw_additional_instructions(frame, editor)
        }
        Some(Overlay::Profiles { cursor, names }) => draw_profiles(frame, snapshot, *cursor, names),
        Some(Overlay::Error {
            title,
            message,
            exit_on_close,
        }) => draw_error(frame, title, message, *exit_on_close),
        None => {}
    }
}

fn draw_main(
    frame: &mut Frame,
    runtime: &RuntimeView,
    snapshot: &AppSnapshot,
    ui_state: &mut UiState,
    turn_active: bool,
) {
    let [rail, _, work_area] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(frame.area());
    let [logo, runtime_area, _, sessions, _, shortcuts] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(12),
    ])
    .areas(rail);
    let [session_area, _, input_area] = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(8),
    ])
    .areas(work_area);

    draw_logo(frame, logo);
    draw_runtime(frame, runtime_area, runtime);
    draw_sessions(frame, sessions, snapshot, ui_state);
    draw_shortcuts(frame, shortcuts, ui_state.focus);
    draw_transcript(frame, session_area, snapshot, ui_state, turn_active);
    draw_input(
        frame,
        input_area,
        ui_state.focus == MainFocus::Input,
        &ui_state.input,
    );
}

fn draw_logo(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::PANEL_BACKGROUND)),
        area,
    );
    let [_, logo_area, tagline_area, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let logo = Paragraph::new(Line::from(vec![
        Span::styled(
            "S I G M A  ",
            Style::default().fg(theme::PRIMARY_TEXT).bold(),
        ),
        Span::styled(
            "Σ",
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::LOGO_BLUE)
                .bold(),
        ),
        Span::styled("  R I F T", Style::default().fg(theme::PRIMARY_TEXT).bold()),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(logo, logo_area);
    frame.render_widget(
        Paragraph::new("LLM-DRIVEN SYSTEM OPERATIONS")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme::MUTED_TEXT)),
        tagline_area,
    );
}

fn draw_runtime(frame: &mut Frame, area: Rect, runtime: &RuntimeView) {
    let model_name = runtime
        .model_name
        .as_deref()
        .map(|name| truncate_label(name, 22))
        .unwrap_or_else(|| "-".to_owned());
    let usage_style = if runtime.context_full {
        Style::default().fg(theme::ERROR_RED)
    } else {
        Style::default().fg(theme::SYSTEM_BLUE)
    };
    let used_tokens = runtime
        .prompt_tokens
        .zip(runtime.completion_tokens)
        .map(|(prompt, completion)| prompt.saturating_add(completion));
    let runtime = Paragraph::new(vec![
        status_line("LLAMA", runtime.llama_version),
        status_line("MODEL", &model_name),
        status_line_with_style(
            "STATUS",
            &runtime.status,
            runtime_status_style(&runtime.status),
        ),
        status_line_with_style(
            "CTX",
            &used_tokens.map_or_else(
                || format!("- / {}", runtime.context_size),
                |used| format!("{used} / {}", runtime.context_size),
            ),
            usage_style,
        ),
        status_line_with_style(
            "LEFT",
            &used_tokens.map_or_else(
                || "-".to_owned(),
                |used| runtime.context_size.saturating_sub(used).to_string(),
            ),
            usage_style,
        ),
    ])
    .style(Style::default().bg(theme::PANEL_BACKGROUND))
    .block(information_panel(" RUNTIME ").padding(Padding::horizontal(1)));
    frame.render_widget(runtime, area);
}

fn draw_sessions(frame: &mut Frame, area: Rect, snapshot: &AppSnapshot, ui_state: &mut UiState) {
    let focused = ui_state.focus == MainFocus::Sessions;
    let panel = main_panel(" SESSIONS ", focused);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let new_session_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: 1,
    };
    let stored_sessions_area = Rect {
        x: inner.x,
        y: new_session_area.y.saturating_add(2),
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };
    let new_session_style = if focused && ui_state.session_cursor == SessionCursor::NewSession {
        Style::default()
            .fg(theme::MAIN_BACKGROUND)
            .bg(theme::ACTIVE_GREEN)
            .bold()
    } else {
        Style::default().bg(theme::RAISED_BACKGROUND)
    };
    frame.render_widget(
        Paragraph::new(padded_label("+ NEW SESSION", new_session_area.width))
            .style(new_session_style),
        new_session_area,
    );

    let mut items = Vec::new();
    for session in &snapshot.sessions {
        let active = snapshot
            .active_session
            .as_ref()
            .is_some_and(|active| active.id == session.id);
        let marker = "❏  ";
        let style = if active {
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold()
        } else {
            Style::default()
        };
        items.push(
            ListItem::new(Line::from(padded_label(
                &format!("{marker}{}", session.title),
                stored_sessions_area.width,
            )))
            .style(style),
        );
    }
    let selected_index = match &ui_state.session_cursor {
        SessionCursor::Session(id) => snapshot
            .sessions
            .iter()
            .position(|session| session.id == *id)
            .unwrap_or(0),
        SessionCursor::NewSession => usize::MAX,
    };
    ui_state.sync_session_list_scroll(
        (selected_index != usize::MAX).then_some(selected_index),
        usize::from(stored_sessions_area.height),
    );
    let mut state = ListState::default()
        .with_offset(ui_state.session_list_offset())
        .with_selected((selected_index != usize::MAX).then_some(selected_index));
    let highlight = if focused {
        Style::default()
            .fg(theme::MAIN_BACKGROUND)
            .bg(theme::ACTIVE_GREEN)
            .bold()
    } else {
        Style::default()
    };
    let list = List::new(items)
        .style(
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::PANEL_BACKGROUND),
        )
        .highlight_style(highlight);
    frame.render_stateful_widget(list, stored_sessions_area, &mut state);
}

fn draw_shortcuts(frame: &mut Frame, area: Rect, focus: MainFocus) {
    let mut lines = vec![shortcut_line("[ Tab ]", "Switch panels")];
    if focus == MainFocus::Sessions {
        lines.push(shortcut_line("[ ↑ ] [ ↓ ]", "Navigate"));
        lines.push(shortcut_line("[ Enter ]", "Select"));
        lines.push(shortcut_line("[ Delete ]", "Delete session"));
    }
    if focus == MainFocus::Session {
        lines.push(shortcut_line("[ ↑ ] [ ↓ ]", "Scroll transcript"));
        lines.push(shortcut_line("[ End ]", "Latest output"));
    }
    lines.push(shortcut_line("[ Ctrl+Space ]", "Control menu"));
    lines.push(shortcut_line("[ Ctrl+C ]", "Interrupt active work"));
    lines.push(shortcut_line("[ Esc ]", "Exit"));

    let shortcuts = Paragraph::new(lines)
        .style(Style::default().bg(theme::PANEL_BACKGROUND))
        .block(information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, area);
}

fn draw_transcript(
    frame: &mut Frame,
    area: Rect,
    snapshot: &AppSnapshot,
    ui_state: &mut UiState,
    turn_active: bool,
) {
    let mut content = match &snapshot.active_session {
        Some(session) if session.messages.is_empty() => {
            vec![Line::from(format!(
                " No messages in {} yet.",
                session.title
            ))]
        }
        Some(session) => {
            let mut lines = Vec::new();
            let mut model_turn_open = false;
            for message in &session.messages {
                lines.push(Line::from(""));
                if message.role == "user" {
                    model_turn_open = false;
                    lines.push(Line::from(Span::styled(
                        " YOU ",
                        Style::default()
                            .fg(theme::MAIN_BACKGROUND)
                            .bg(theme::SYSTEM_BLUE)
                            .bold(),
                    )));
                    lines.extend(markdown_lines(
                        &message.content,
                        Style::default().fg(theme::SYSTEM_BLUE),
                        false,
                    ));
                } else if message.role == "model" {
                    if !model_turn_open {
                        lines.push(model_badge());
                        model_turn_open = true;
                    }
                    if !message.reasoning_content.is_empty() {
                        lines.push(Line::from(""));
                        lines.push(indented_badge(" REASONING ", theme::REASONING_PURPLE));
                        let mut reasoning = markdown_lines(
                            &reasoning_without_label(&message.reasoning_content),
                            Style::default().fg(theme::MUTED_TEXT).italic(),
                            false,
                        );
                        indent_lines(&mut reasoning, "   ");
                        lines.extend(reasoning);
                        lines.push(Line::from(""));
                    }
                    lines.extend(markdown_lines(
                        &message.content,
                        Style::default().fg(theme::PRIMARY_TEXT),
                        true,
                    ));
                } else if message.role == "exec" {
                    lines.push(indented_badge(" EXEC ", theme::TOOL_YELLOW));
                    lines.push(indented_line(
                        "   ",
                        Style::default().fg(theme::TOOL_YELLOW),
                        vec![Span::styled(
                            format!(
                                "execute {{\"command\":{}}}",
                                serde_json::json!(message.content)
                            ),
                            Style::default().fg(theme::TOOL_YELLOW),
                        )],
                    ));
                } else if message.role == "exec_result" {
                    lines.push(indented_badge(" EXEC RESULT ", theme::TOOL_DIM_YELLOW));
                    if !message.content.is_empty() {
                        let mut result = markdown_lines(
                            &message.content,
                            Style::default().fg(theme::PRIMARY_TEXT),
                            false,
                        );
                        indent_lines(&mut result, "   ");
                        lines.extend(result);
                    }
                    if !message.reasoning_content.is_empty() {
                        let mut stderr = markdown_lines(
                            &message.reasoning_content,
                            Style::default().fg(theme::ERROR_RED),
                            false,
                        );
                        indent_lines(&mut stderr, "   ");
                        lines.extend(stderr);
                    }
                    if !message.status.is_empty() {
                        lines.push(indented_line(
                            "   ",
                            Style::default().fg(theme::TOOL_DIM_YELLOW),
                            vec![Span::styled(
                                message.status.clone(),
                                Style::default().fg(theme::TOOL_DIM_YELLOW),
                            )],
                        ));
                    }
                }
            }
            lines
        }
        None => vec![Line::from(" Create a session from the SESSIONS panel.")],
    };
    if processing_model_input(snapshot, turn_active) {
        content.push(Line::from(""));
        if pending_model_turn_has_no_output(snapshot) {
            content.push(model_badge());
            content.push(Line::from(""));
        }
        content.push(indented_badge(" PROCESSING... ", theme::PROCESSING_TEAL));
    }

    let panel =
        session_panel(ui_state.focus == MainFocus::Session).padding(Padding::new(1, 1, 1, 1));
    let transcript_area = panel.inner(area);
    frame.render_widget(panel, area);

    // Always reserve the rightmost inner column so wrapped text never occupies the scrollbar.
    let content_width = transcript_area.width.saturating_sub(1);
    let content_area = Rect {
        width: content_width,
        ..transcript_area
    };
    content = constrain_reasoning_lines(content, content_area.width);
    let content_line_count = wrapped_line_count(&content, content_area.width);
    let transcript = Paragraph::new(content)
        .style(
            Style::default()
                .fg(theme::MUTED_TEXT)
                .bg(theme::PANEL_BACKGROUND),
        )
        .wrap(Wrap { trim: false });
    ui_state.sync_transcript(content_line_count, usize::from(content_area.height));
    frame.render_widget(
        transcript.scroll((ui_state.transcript_offset(), 0)),
        content_area,
    );

    if ui_state.transcript_scrolls() {
        let (content_length, position, viewport_content_length) =
            ui_state.transcript_scrollbar_state();
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(theme::ACTIVE_GREEN))
            .track_style(Style::default().fg(theme::UNFOCUSED_BORDER));
        let mut scrollbar_state = ScrollbarState::new(content_length)
            .position(position)
            .viewport_content_length(viewport_content_length);
        let scrollbar_area = Rect {
            x: content_area.x.saturating_add(content_area.width),
            width: 1,
            ..transcript_area
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn draw_input(frame: &mut Frame, area: Rect, focused: bool, input: &TextEditor) {
    let panel = main_panel(" INPUT ", focused);
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let editor_area = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y.saturating_add(1),
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };
    input.render(frame, editor_area);
}

fn processing_model_input(snapshot: &AppSnapshot, turn_active: bool) -> bool {
    turn_active
        && snapshot.active_session.as_ref().is_some_and(|session| {
            matches!(
                session.messages.last(),
                Some(message) if matches!(message.role.as_str(), "user" | "exec_result")
            )
        })
}

fn pending_model_turn_has_no_output(snapshot: &AppSnapshot) -> bool {
    snapshot.active_session.as_ref().is_some_and(|session| {
        !session
            .messages
            .iter()
            .rev()
            .take_while(|message| message.role != "user")
            .any(|message| message.role == "model")
    })
}

fn model_badge() -> Line<'static> {
    Line::from(Span::styled(
        " MODEL ",
        Style::default()
            .fg(theme::MAIN_BACKGROUND)
            .bg(theme::ACTIVE_GREEN)
            .bold(),
    ))
}

fn draw_control(frame: &mut Frame, cursor: usize) {
    let modal_area = components::overlay(frame, 44, 16);
    let [menu_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Length(7),
    ])
    .areas(modal_area);
    let menu = overlay_active_panel(" CONTROL ");
    let inner = menu.inner(menu_area);
    frame.render_widget(menu, menu_area);
    let rows = [
        ("LLM Parameters", "M"),
        ("Additional Instructions", "I"),
        ("Profiles", "F"),
        ("Quit", "Q"),
    ];
    for (index, (label, accelerator)) in rows.into_iter().enumerate() {
        let area = Rect {
            x: inner.x,
            y: inner.y.saturating_add(1 + index as u16),
            width: inner.width,
            height: 1,
        };
        let style = if index == cursor {
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold()
        } else {
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND)
        };
        let text = format!(
            " {}{}",
            label,
            " ".repeat(usize::from(area.width).saturating_sub(label.chars().count() + 4))
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(text, style),
                Span::styled(accelerator, style),
            ])),
            area,
        );
    }
    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ ↑ ] [ ↓ ]", "Navigate"),
        shortcut_line("[ Enter ]", "Open"),
        shortcut_line("[ M I F Q ]", "Open / activate"),
        shortcut_line("[ Esc ]", "Close"),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn draw_llm_parameters(
    frame: &mut Frame,
    sampling: &SamplingSettings,
    cursor: usize,
    advanced: bool,
    draft: Option<&TextEditor>,
) {
    let rows = llm_rows(sampling, advanced);
    let modal_area =
        components::overlay(frame, 54, (rows.len() as u16 + 10).min(frame.area().height));
    let [parameters_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length((rows.len() as u16 + 3).min(modal_area.height.saturating_sub(7))),
        Constraint::Length(1),
        Constraint::Length(6),
    ])
    .areas(modal_area);
    let panel = overlay_active_panel(" LLM PARAMETERS ");
    let inner = panel.inner(parameters_area);
    frame.render_widget(panel, parameters_area);
    for (index, (label, value)) in rows.iter().enumerate() {
        let area = Rect {
            x: inner.x,
            y: inner.y.saturating_add(1 + index as u16),
            width: inner.width,
            height: 1,
        };
        let selected = index == cursor;
        let style = if selected {
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold()
        } else {
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND)
        };
        let value = if selected && draft.is_some() {
            String::new()
        } else {
            value.clone()
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {:<18} {}", label, value))).style(style),
            area,
        );
        if selected && let Some(editor) = draft {
            let editor_area = Rect {
                x: inner.x.saturating_add(20),
                y: area.y,
                width: inner.width.saturating_sub(20),
                height: 1,
            };
            editor.render(frame, editor_area);
        }
    }
    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ ↑ ] [ ↓ ]", "Navigate"),
        shortcut_line(
            "[ Enter ]",
            if draft.is_some() {
                "Save value"
            } else {
                "Edit / expand"
            },
        ),
        shortcut_line(
            "[ Esc ]",
            if draft.is_some() {
                "Discard value"
            } else {
                "Back"
            },
        ),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn llm_rows(sampling: &SamplingSettings, advanced: bool) -> Vec<(String, String)> {
    let mut rows = vec![
        (
            "Temperature".to_owned(),
            format!("{:.2}", sampling.temperature),
        ),
        ("Max tokens".to_owned(), sampling.max_tokens.to_string()),
        ("Top P".to_owned(), format!("{:.2}", sampling.top_p)),
        ("Top K".to_owned(), sampling.top_k.to_string()),
        ("Min P".to_owned(), format!("{:.2}", sampling.min_p)),
        (
            "Repeat penalty".to_owned(),
            format!("{:.2}", sampling.repeat_penalty),
        ),
        (
            if advanced {
                "Advanced ▲"
            } else {
                "Advanced..."
            }
            .to_owned(),
            String::new(),
        ),
    ];
    if advanced {
        rows.extend([
            (
                "Seed".to_owned(),
                sampling
                    .seed
                    .map_or_else(|| "Default".to_owned(), |value| value.to_string()),
            ),
            (
                "Repeat last N".to_owned(),
                sampling
                    .repeat_last_n
                    .map_or_else(|| "Default".to_owned(), |value| value.to_string()),
            ),
            (
                "Presence penalty".to_owned(),
                sampling
                    .presence_penalty
                    .map_or_else(|| "Default".to_owned(), |value| format!("{value:.2}")),
            ),
            (
                "Frequency penalty".to_owned(),
                sampling
                    .frequency_penalty
                    .map_or_else(|| "Default".to_owned(), |value| format!("{value:.2}")),
            ),
            (
                "Stop sequences".to_owned(),
                if sampling.stop.is_empty() {
                    "Default".to_owned()
                } else {
                    sampling.stop.join(", ")
                },
            ),
            (
                "Grammar / JSON".to_owned(),
                sampling
                    .grammar
                    .clone()
                    .unwrap_or_else(|| "Default".to_owned()),
            ),
        ]);
    }
    rows
}

fn draw_additional_instructions(frame: &mut Frame, editor: &TextEditor) {
    let modal_area = components::overlay(
        frame,
        frame.area().width.saturating_sub(8).min(100),
        frame.area().height.saturating_sub(4).min(30),
    );
    let [editor_panel_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(1),
        Constraint::Length(6),
    ])
    .areas(modal_area);
    let panel = overlay_active_panel(" ADDITIONAL INSTRUCTIONS ").padding(Padding::new(1, 1, 1, 1));
    let editor_area = panel.inner(editor_panel_area);
    frame.render_widget(panel, editor_panel_area);
    editor.render(frame, editor_area);
    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ Ctrl+S ]", "Save"),
        shortcut_line("[ Esc ]", "Discard"),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn draw_profiles(frame: &mut Frame, snapshot: &AppSnapshot, cursor: usize, names: &[String]) {
    let modal_area = components::overlay(
        frame,
        44,
        (names.len() as u16 + 11).min(frame.area().height),
    );
    let [profiles_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length((names.len() as u16 + 4).min(modal_area.height.saturating_sub(7))),
        Constraint::Length(1),
        Constraint::Length(6),
    ])
    .areas(modal_area);
    let panel = overlay_active_panel(" PROFILES ");
    let inner = panel.inner(profiles_area);
    frame.render_widget(panel, profiles_area);
    let active = snapshot
        .active_session
        .as_ref()
        .map(|session| &session.profiles);
    if names.is_empty() {
        frame.render_widget(Paragraph::new(" No profiles in prompts/profiles/."), inner);
    }
    for (index, name) in names.iter().enumerate() {
        let checked = active.is_some_and(|profiles| profiles.contains(name));
        let style = if index == cursor {
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold()
        } else {
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND)
        };
        let area = Rect {
            x: inner.x,
            y: inner.y.saturating_add(1 + index as u16),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(format!(" [{}] {}", if checked { "x" } else { " " }, name)).style(style),
            area,
        );
    }
    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ ↑ ] [ ↓ ]", "Navigate"),
        shortcut_line("[ Enter ]", "Toggle"),
        shortcut_line("[ Esc ]", "Back"),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn draw_delete_confirmation(frame: &mut Frame, snapshot: &AppSnapshot, id: &str) {
    let modal_area = components::overlay(frame, 50, 12);
    let [dialog_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(6),
    ])
    .areas(modal_area);
    let title = snapshot
        .sessions
        .iter()
        .find(|session| session.id == id)
        .map(|session| session.title.as_str())
        .unwrap_or("this session");
    let dialog = Paragraph::new(vec![
        Line::from(" Delete the selected stored session?"),
        Line::from(Span::styled(
            format!(" {title}"),
            Style::default().fg(theme::PRIMARY_TEXT).bold(),
        )),
    ])
    .style(
        Style::default()
            .fg(theme::MUTED_TEXT)
            .bg(theme::RAISED_BACKGROUND),
    )
    .block(overlay_active_panel(" CONFIRM DELETE ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(dialog, dialog_area);

    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ Enter ]", "Delete"),
        shortcut_line("[ Esc ]", "Cancel"),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn draw_error(frame: &mut Frame, title: &str, message: &str, exit_on_close: bool) {
    let modal_area = components::overlay(frame, 60, 12);
    let [message_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(6),
    ])
    .areas(modal_area);
    let message = Paragraph::new(message)
        .style(
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND),
        )
        .wrap(Wrap { trim: false })
        .block(error_panel(title).padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(message, message_area);

    let action = if exit_on_close { "Exit" } else { "Close" };
    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ Enter ]", action),
        shortcut_line("[ Esc ]", action),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

#[derive(Clone)]
struct TranscriptMarkdownStyle {
    body: Style,
}

impl StyleSheet for TranscriptMarkdownStyle {
    fn heading(&self, _level: u8) -> Style {
        self.body.bold()
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code(&self) -> Style {
        Style::default().fg(theme::TOOL_DIM_YELLOW)
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn link(&self) -> Style {
        Style::default().fg(theme::SYSTEM_BLUE).underlined()
    }

    fn blockquote(&self) -> Style {
        Style::default().fg(theme::MUTED_TEXT).italic()
    }

    fn table_header(&self) -> Style {
        self.body.bold()
    }

    fn table_border(&self) -> Style {
        Style::default().fg(theme::MUTED_TEXT)
    }
}

/// Converts CommonMark/GFM content directly into Ratatui lines while preserving SigmaRift's
/// semantic palette for each transcript section.
fn markdown_lines(content: &str, body: Style, unwrap_document_wrapper: bool) -> Vec<Line<'static>> {
    let content = if unwrap_document_wrapper {
        unwrap_markdown_document(content)
    } else {
        Cow::Borrowed(content)
    };
    let options = Options::new(TranscriptMarkdownStyle { body });
    from_str_with_options(&content, &options)
        .lines
        .into_iter()
        .map(|line| {
            Line::from(
                line.spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), body.patch(span.style)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Models sometimes wrap a complete Markdown response in a `markdown` fenced block. That wrapper
/// would intentionally render the document as literal code, so remove it while retaining nested
/// code blocks that are part of the document itself. User input is always rendered verbatim.
fn unwrap_markdown_document(content: &str) -> Cow<'_, str> {
    let trimmed_end = content.trim_end_matches(['\r', '\n']);
    let Some(last_newline) = trimmed_end.rfind('\n') else {
        return Cow::Borrowed(content);
    };
    if &trimmed_end[last_newline + 1..] != "```" {
        return Cow::Borrowed(content);
    }

    let Some((wrapper_start, wrapper_line_end)) = trimmed_end
        .match_indices("```")
        .filter(|(index, _)| *index == 0 || trimmed_end.as_bytes()[index - 1] == b'\n')
        .find_map(|(index, _)| {
            let line_end = trimmed_end[index..]
                .find('\n')
                .map(|offset| index + offset)?;
            let line = trimmed_end[index..line_end].trim_end_matches('\r');
            (line.eq_ignore_ascii_case("```markdown") || line.eq_ignore_ascii_case("```md"))
                .then_some((index, line_end))
        })
    else {
        return Cow::Borrowed(content);
    };

    if wrapper_start > 0 {
        let body = &trimmed_end[wrapper_line_end + 1..last_newline];
        let first_inner_fence = body
            .lines()
            .map(str::trim_end)
            .find(|line| line.starts_with("```"));
        if first_inner_fence.is_none_or(|line| line == "```") {
            return Cow::Borrowed(content);
        }
    }

    let mut unwrapped = String::with_capacity(trimmed_end.len());
    unwrapped.push_str(&trimmed_end[..wrapper_start]);
    unwrapped.push_str(&trimmed_end[wrapper_line_end + 1..last_newline]);
    Cow::Owned(unwrapped)
}

fn indent_lines(lines: &mut [Line<'static>], indent: &str) {
    for line in lines {
        line.spans.insert(
            0,
            Span::styled(
                indent.to_owned(),
                Style::default().fg(theme::MUTED_TEXT).italic(),
            ),
        );
    }
}

fn indented_badge(label: &str, background: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![
        Span::raw("   "),
        Span::styled(
            label.to_owned(),
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(background)
                .bold(),
        ),
    ])
}

/// Ratatui wraps each line from the left edge. Rewrap nested reasoning before it reaches the
/// paragraph widget so every continuation line retains its three-cell transcript indent.
fn constrain_reasoning_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| {
            if line
                .spans
                .first()
                .is_some_and(|span| span.content.as_ref() == "   ")
            {
                wrap_indented_line(line, width)
            } else {
                vec![line]
            }
        })
        .collect()
}

fn wrap_indented_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let indent = line.spans.first().expect("reasoning line has an indent");
    let indent_content = indent.content.to_string();
    let indent_style = indent.style;
    let available = usize::from(width)
        .saturating_sub(indent_content.chars().count())
        .max(1);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0;

    for span in line.spans.into_iter().skip(1) {
        let mut fragment = String::new();
        for character in span.content.chars() {
            let character_width = character.width().unwrap_or(0);
            if current_width > 0 && current_width + character_width > available {
                if !fragment.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut fragment), span.style));
                }
                lines.push(indented_line(
                    &indent_content,
                    indent_style,
                    std::mem::take(&mut current),
                ));
                current_width = 0;
            }
            fragment.push(character);
            current_width += character_width;
        }
        if !fragment.is_empty() {
            current.push(Span::styled(fragment, span.style));
        }
    }

    lines.push(indented_line(&indent_content, indent_style, current));
    lines
}

fn indented_line(
    indent: &str,
    indent_style: Style,
    mut content: Vec<Span<'static>>,
) -> Line<'static> {
    content.insert(0, Span::styled(indent.to_owned(), indent_style));
    Line::from(content)
}

fn reasoning_without_label(content: &str) -> Cow<'_, str> {
    let label_start = content.len() - content.trim_start_matches([' ', '\t']).len();
    let unlabelled = &content[label_start..];
    let Some(remaining) = unlabelled
        .strip_prefix("Thinking Process:")
        .or_else(|| unlabelled.strip_prefix("Thinking:"))
    else {
        return Cow::Borrowed(content);
    };

    let remaining = remaining
        .strip_prefix("\r\n")
        .or_else(|| remaining.strip_prefix('\n'))
        .or_else(|| remaining.strip_prefix(' '))
        .unwrap_or(remaining);
    Cow::Owned(format!("{}{remaining}", &content[..label_start]))
}

fn wrapped_line_count(lines: &[Line<'_>], width: u16) -> usize {
    let width = usize::from(width);
    if width == 0 {
        return 0;
    }

    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
}

fn draw_model_select(frame: &mut Frame, models: &[String], cursor: usize) {
    let model_height = (models.len().min(6) as u16) + 4;
    let modal_height = model_height + 1 + 7;
    let modal_area = components::overlay(frame, 52, modal_height);
    let [model_area, _, shortcuts_area] = Layout::vertical([
        Constraint::Length(model_height),
        Constraint::Length(1),
        Constraint::Length(7),
    ])
    .areas(modal_area);

    let model_block = overlay_active_panel(" MODEL SELECT ");
    let model_inner = model_block.inner(model_area);
    frame.render_widget(model_block, model_area);

    let list_area = Rect {
        x: model_inner.x,
        y: model_inner.y.saturating_add(1),
        width: model_inner.width,
        height: model_inner.height.saturating_sub(2),
    };

    let items = models
        .iter()
        .map(|model| ListItem::new(Line::from(padded_label(model, list_area.width))))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .style(
            Style::default()
                .fg(theme::PRIMARY_TEXT)
                .bg(theme::RAISED_BACKGROUND),
        )
        .highlight_style(
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold(),
        );
    let mut list_state = ListState::default();
    list_state.select(Some(cursor.min(models.len().saturating_sub(1))));
    frame.render_stateful_widget(list, list_area, &mut list_state);

    let shortcuts = Paragraph::new(vec![
        shortcut_line("[ ↑ ] [ ↓ ]", "Select"),
        shortcut_line("[ Enter ]", "Load"),
        shortcut_line("[ Esc ]", "Quit"),
    ])
    .style(Style::default().bg(theme::RAISED_BACKGROUND))
    .block(overlay_information_panel(" SHORTCUTS ").padding(Padding::new(1, 1, 1, 1)));
    frame.render_widget(shortcuts, shortcuts_area);
}

fn active_panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACTIVE_GREEN))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ACTIVE_GREEN)
                .bold(),
        ))
        .style(Style::default().bg(theme::PANEL_BACKGROUND))
}

fn information_panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::SYSTEM_BLUE))
        .title(Span::styled(
            title,
            Style::default().fg(theme::SYSTEM_BLUE).bold(),
        ))
        .style(Style::default().bg(theme::PANEL_BACKGROUND))
}

fn main_panel(title: &str, focused: bool) -> Block<'_> {
    if focused {
        active_panel(title)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::UNFOCUSED_BORDER))
            .title(Span::styled(
                title,
                Style::default().fg(theme::UNFOCUSED_BORDER).bold(),
            ))
            .style(Style::default().bg(theme::PANEL_BACKGROUND))
    }
}

fn session_panel(focused: bool) -> Block<'static> {
    if focused {
        active_panel(" SESSION ")
    } else {
        information_panel(" SESSION ")
    }
}

fn overlay_active_panel(title: &str) -> Block<'_> {
    active_panel(title).style(Style::default().bg(theme::RAISED_BACKGROUND))
}

fn error_panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ERROR_ACCENT))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme::MAIN_BACKGROUND)
                .bg(theme::ERROR_ACCENT)
                .bold(),
        ))
        .style(Style::default().bg(theme::RAISED_BACKGROUND))
}

fn overlay_information_panel(title: &str) -> Block<'_> {
    information_panel(title).style(Style::default().bg(theme::RAISED_BACKGROUND))
}

fn truncate_label(label: &str, available_width: usize) -> String {
    if label.chars().count() <= available_width {
        return label.to_owned();
    }
    if available_width <= 3 {
        return ".".repeat(available_width);
    }

    format!(
        "{}...",
        label.chars().take(available_width - 3).collect::<String>()
    )
}

fn padded_label(label: &str, row_width: u16) -> String {
    let available_width = usize::from(row_width).saturating_sub(2);
    format!(" {} ", truncate_label(label, available_width))
}

fn status_line(label: &str, value: &str) -> Line<'static> {
    status_line_with_style(label, value, Style::default().fg(theme::SYSTEM_BLUE))
}

fn status_line_with_style(label: &str, value: &str, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<8}"),
            Style::default().fg(theme::MUTED_TEXT),
        ),
        Span::styled(value.to_owned(), value_style),
    ])
}

fn runtime_status_style(status: &str) -> Style {
    if status == "READY" {
        Style::default().fg(theme::ACTIVE_GREEN).bold()
    } else {
        Style::default().fg(theme::SYSTEM_BLUE)
    }
}

fn shortcut_line(key: &str, action: &str) -> Line<'static> {
    const KEY_COLUMN_WIDTH: usize = 16;
    let key_padding = " ".repeat(KEY_COLUMN_WIDTH.saturating_sub(key.chars().count()));

    Line::from(vec![
        Span::styled(
            key.to_owned(),
            Style::default().fg(theme::SYSTEM_BLUE).bold(),
        ),
        Span::raw(key_padding),
        Span::styled(action.to_owned(), Style::default().fg(theme::MUTED_TEXT)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{MessageView, SessionView};

    fn snapshot_with_last_message(role: &str) -> AppSnapshot {
        AppSnapshot {
            sessions: Vec::new(),
            active_session: Some(SessionView {
                id: "session".to_owned(),
                title: "Session".to_owned(),
                messages: vec![MessageView {
                    role: role.to_owned(),
                    content: String::new(),
                    reasoning_content: String::new(),
                    status: String::new(),
                }],
                profiles: Vec::new(),
            }),
        }
    }

    #[test]
    fn processing_badge_marks_any_active_model_input() {
        assert!(processing_model_input(
            &snapshot_with_last_message("exec_result"),
            true
        ));
        assert!(processing_model_input(
            &snapshot_with_last_message("user"),
            true
        ));
        assert!(!processing_model_input(
            &snapshot_with_last_message("model"),
            true
        ));
        assert!(!processing_model_input(
            &snapshot_with_last_message("exec_result"),
            false
        ));
    }

    #[test]
    fn pending_model_turn_gets_a_badge_only_before_its_first_output() {
        assert!(pending_model_turn_has_no_output(
            &snapshot_with_last_message("user")
        ));
        assert!(pending_model_turn_has_no_output(
            &snapshot_with_last_message("exec_result")
        ));
        assert!(!pending_model_turn_has_no_output(
            &snapshot_with_last_message("model")
        ));
    }

    #[test]
    fn model_labels_keep_side_padding_when_they_fit() {
        assert_eq!(padded_label("model.gguf", 16), " model.gguf ");
    }

    #[test]
    fn ready_runtime_status_is_green_and_bold() {
        let style = runtime_status_style("READY");

        assert_eq!(style.fg, Some(theme::ACTIVE_GREEN));
        assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn model_labels_truncate_without_overflowing_the_row() {
        assert_eq!(padded_label("very-long-model.gguf", 12), " very-lo... ");
    }

    #[test]
    fn markdown_renderer_handles_common_chat_markdown() {
        let lines = markdown_lines(
            "# Heading\n\n- **bold** and *italic* with `code`\n\n> quote\n\n[link](https://example.com)\n\n| key | value |\n| --- | --- |\n| model | local |\n\n```rust\nlet value = 1;\n```",
            Style::default().fg(theme::PRIMARY_TEXT),
            false,
        );
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Heading"));
        assert!(rendered.contains("bold"));
        assert!(rendered.contains("italic"));
        assert!(rendered.contains("code"));
        assert!(rendered.contains("quote"));
        assert!(rendered.contains("link"));
        assert!(rendered.contains("model"));
        assert!(rendered.contains("let value = 1;"));
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content == "code" && span.style.fg == Some(theme::TOOL_DIM_YELLOW)
        }));
    }

    #[test]
    fn model_markdown_document_wrapper_is_removed_but_inner_code_fence_is_preserved() {
        let content = "```markdown\n# Heading\n\n```rust\nlet value = 1;\n```\n```";

        assert_eq!(
            unwrap_markdown_document(content),
            "# Heading\n\n```rust\nlet value = 1;\n```"
        );
        let lines = markdown_lines(content, Style::default().fg(theme::PRIMARY_TEXT), true);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Heading"));
        assert!(rendered.contains("let value = 1;"));
        assert!(!rendered.contains("```markdown"));
        assert!(!rendered.contains("```rust"));
        assert!(!rendered.contains("# Heading"));
    }

    #[test]
    fn model_markdown_document_wrapper_is_removed_after_an_introduction() {
        let content = "Here is the requested document:\n\n```markdown\n# Heading\n\n```bash\nprintf hello\n```\n\n*Done.*\n```";

        assert_eq!(
            unwrap_markdown_document(content),
            "Here is the requested document:\n\n# Heading\n\n```bash\nprintf hello\n```\n\n*Done.*"
        );
        let lines = markdown_lines(content, Style::default().fg(theme::PRIMARY_TEXT), true);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Here is the requested document:"));
        assert!(rendered.contains("Heading"));
        assert!(rendered.contains("printf hello"));
        assert!(rendered.contains("Done."));
        assert!(!rendered.contains("```markdown"));
        assert!(!rendered.contains("# Heading"));
    }

    #[test]
    fn ordinary_markdown_code_example_after_an_introduction_stays_literal() {
        let content = "Here is a Markdown example:\n\n```markdown\n# Literal heading\n```";

        assert_eq!(unwrap_markdown_document(content), content);
        let lines = markdown_lines(content, Style::default().fg(theme::PRIMARY_TEXT), true);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("# Literal heading"));
    }

    #[test]
    fn user_markdown_is_rendered_verbatim() {
        let content = "```markdown\n# Example\n```\n\nLiteral \\n remains escaped.";
        let lines = markdown_lines(content, Style::default().fg(theme::SYSTEM_BLUE), false);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("# Example"));
        assert!(rendered.contains("\\n"));
    }

    #[test]
    fn reasoning_markdown_keeps_its_indent_and_inline_styles() {
        let mut lines = markdown_lines(
            "*work* with `code`",
            Style::default().fg(theme::MUTED_TEXT).italic(),
            false,
        );
        indent_lines(&mut lines, "   ");

        assert!(lines.iter().all(|line| line.spans[0].content == "   "));
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content == "code" && span.style.fg == Some(theme::TOOL_DIM_YELLOW)
        }));

        let wrapped = constrain_reasoning_lines(lines, 8);
        assert!(wrapped.iter().all(|line| line.spans[0].content == "   "));
    }

    #[test]
    fn reasoning_wrap_uses_terminal_cell_width_for_wide_characters() {
        let lines = vec![indented_line(
            "   ",
            Style::default(),
            vec![Span::raw("界x")],
        )];

        let wrapped = constrain_reasoning_lines(lines, 5);

        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].to_string(), "   界");
        assert_eq!(wrapped[1].to_string(), "   x");
        assert!(wrapped.iter().all(|line| line.width() <= 5));
    }

    #[test]
    fn reasoning_labels_are_removed_without_changing_the_body() {
        assert_eq!(
            reasoning_without_label("Thinking: work it out"),
            "work it out"
        );
        assert_eq!(
            reasoning_without_label("  Thinking Process:\nwork it out"),
            "  work it out"
        );
        assert_eq!(
            reasoning_without_label("Thoughts: work it out"),
            "Thoughts: work it out"
        );
    }
}
