mod components;
mod input;
mod render;
mod state;
mod text_editor;
mod theme;

use std::{io, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{control::Control, model::SamplingSettings};
use state::{MainFocus, Overlay, SessionCursor, UiState};
use text_editor::TextEditor;

pub fn run(control: &mut Control) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error);
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error);
        }
    };

    let result = run_loop(&mut terminal, control);
    // Attempt every restoration step even if a preceding one fails.
    let cursor_result = terminal.show_cursor();
    let screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let raw_mode_result = disable_raw_mode();

    result?;
    cursor_result?;
    screen_result?;
    raw_mode_result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    control: &mut Control,
) -> io::Result<()> {
    let mut model_cursor = control
        .startup_model_selection()
        .map(|selection| selection.selected_index);
    let mut ui_state = UiState::new(&control.snapshot());
    let startup_warnings = control.take_startup_warnings();
    if let Some(error) = control.startup_error() {
        let warnings = startup_warnings.join("\n");
        ui_state.show_startup_error(if warnings.is_empty() {
            error
        } else {
            format!("{error}\n\n{warnings}")
        });
    } else if !startup_warnings.is_empty() {
        ui_state.show_error(" SESSION WARNING ", startup_warnings.join("\n"));
    }

    loop {
        if let Err(error) = control.poll() {
            ui_state.show_error(" MODEL ERROR ", error.to_string());
        }
        let runtime = control.runtime_view();
        let snapshot = control.snapshot();
        ui_state.sync_sessions(&snapshot);
        let model_selection = control.startup_model_selection();
        terminal.draw(|frame| {
            render::draw(
                frame,
                &runtime,
                &snapshot,
                &mut ui_state,
                &control.runtime_sampling(),
                control.turn_active(),
                model_selection
                    .as_ref()
                    .map(|selection| selection.names.as_slice()),
                model_cursor,
            )
        })?;
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            let action = input::action(key);
            if model_cursor.is_some() {
                match action {
                    Some(input::UiAction::Back) => return Ok(()),
                    Some(input::UiAction::MoveUp) => {
                        if let Some(cursor) = &mut model_cursor {
                            *cursor = cursor.saturating_sub(1);
                        }
                    }
                    Some(input::UiAction::MoveDown) => {
                        if let (Some(cursor), Some(selection)) =
                            (&mut model_cursor, control.startup_model_selection())
                        {
                            *cursor = (*cursor + 1).min(selection.names.len().saturating_sub(1));
                        }
                    }
                    Some(input::UiAction::Activate) => {
                        if let Some(cursor) = model_cursor {
                            match control.select_startup_model(cursor) {
                                Ok(true) => model_cursor = None,
                                Ok(false) => {}
                                Err(error) => {
                                    model_cursor = None;
                                    ui_state.show_startup_error(error.to_string());
                                }
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if ui_state.overlay.is_some() {
                match handle_overlay(control, &mut ui_state, key, action) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => ui_state.show_error(" ERROR ", error.to_string()),
                }
                continue;
            }

            if matches!(action, Some(input::UiAction::Interrupt)) {
                if control.interrupt() {
                    ui_state.show_error(" INTERRUPTED ", "Interrupted by user.");
                }
                continue;
            }

            match action {
                Some(input::UiAction::OpenControl) => {
                    ui_state.overlay = Some(Overlay::Control { cursor: 0 })
                }
                Some(input::UiAction::Back) => return Ok(()),
                Some(input::UiAction::FocusNext) => ui_state.focus_next(),
                Some(input::UiAction::FocusPrevious) => ui_state.focus_previous(),
                Some(input::UiAction::MoveUp) if ui_state.focus == MainFocus::Input => {
                    ui_state.input.input(key);
                }
                Some(input::UiAction::MoveDown) if ui_state.focus == MainFocus::Input => {
                    ui_state.input.input(key);
                }
                Some(input::UiAction::MoveUp) if ui_state.focus == MainFocus::Sessions => {
                    ui_state.move_session_cursor(&snapshot, false);
                    if let SessionCursor::Session(id) = &ui_state.session_cursor {
                        control.select_session(id);
                        ui_state.follow_transcript_bottom();
                    }
                }
                Some(input::UiAction::MoveDown) if ui_state.focus == MainFocus::Sessions => {
                    ui_state.move_session_cursor(&snapshot, true);
                    if let SessionCursor::Session(id) = &ui_state.session_cursor {
                        control.select_session(id);
                        ui_state.follow_transcript_bottom();
                    }
                }
                Some(input::UiAction::MoveUp) if ui_state.focus == MainFocus::Session => {
                    ui_state.scroll_transcript(false);
                }
                Some(input::UiAction::MoveDown) if ui_state.focus == MainFocus::Session => {
                    ui_state.scroll_transcript(true);
                }
                None if ui_state.focus == MainFocus::Session && key.code == KeyCode::End => {
                    ui_state.follow_transcript_bottom();
                }
                Some(input::UiAction::Activate) if ui_state.focus == MainFocus::Sessions => {
                    match ui_state.session_cursor.clone() {
                        SessionCursor::NewSession => match control.create_session() {
                            Ok(session) => {
                                ui_state.session_cursor = SessionCursor::Session(session.id);
                                ui_state.focus = MainFocus::Input;
                                ui_state.follow_transcript_bottom();
                            }
                            Err(error) => ui_state.show_error(" SESSION ERROR ", error.to_string()),
                        },
                        SessionCursor::Session(id) => {
                            control.select_session(&id);
                            ui_state.follow_transcript_bottom();
                        }
                    }
                }
                Some(input::UiAction::Delete) if ui_state.focus == MainFocus::Sessions => {
                    if let SessionCursor::Session(id) = &ui_state.session_cursor {
                        ui_state.overlay = Some(Overlay::ConfirmDelete(id.clone()));
                    }
                }
                Some(input::UiAction::Activate) if ui_state.focus == MainFocus::Input => {
                    if !control.turn_active()
                        && !ui_state.input.is_empty()
                        && let Err(error) =
                            submit_input(&mut ui_state.input, |content| control.submit(content))
                    {
                        ui_state.show_error(" MODEL ERROR ", error.to_string());
                    }
                }
                None if ui_state.focus == MainFocus::Input => {
                    ui_state.input.input(key);
                }
                _ => {}
            }
        }
    }
}

const CONTROL_ROWS: usize = 4;

#[derive(Clone, Copy)]
enum SamplingField {
    Temperature,
    MaxTokens,
    TopP,
    TopK,
    MinP,
    RepeatPenalty,
    Advanced,
    Seed,
    RepeatLastN,
    PresencePenalty,
    FrequencyPenalty,
    StopSequences,
    Grammar,
}

fn sampling_fields(advanced: bool) -> Vec<SamplingField> {
    let mut fields = vec![
        SamplingField::Temperature,
        SamplingField::MaxTokens,
        SamplingField::TopP,
        SamplingField::TopK,
        SamplingField::MinP,
        SamplingField::RepeatPenalty,
        SamplingField::Advanced,
    ];
    if advanced {
        fields.extend([
            SamplingField::Seed,
            SamplingField::RepeatLastN,
            SamplingField::PresencePenalty,
            SamplingField::FrequencyPenalty,
            SamplingField::StopSequences,
            SamplingField::Grammar,
        ]);
    }
    fields
}

fn sampling_value(settings: &SamplingSettings, field: SamplingField) -> String {
    match field {
        SamplingField::Temperature => format!("{:.2}", settings.temperature),
        SamplingField::MaxTokens => settings.max_tokens.to_string(),
        SamplingField::TopP => format!("{:.2}", settings.top_p),
        SamplingField::TopK => settings.top_k.to_string(),
        SamplingField::MinP => format!("{:.2}", settings.min_p),
        SamplingField::RepeatPenalty => format!("{:.2}", settings.repeat_penalty),
        SamplingField::Advanced => String::new(),
        SamplingField::Seed => settings
            .seed
            .map_or_else(|| "Default".to_owned(), |value| value.to_string()),
        SamplingField::RepeatLastN => settings
            .repeat_last_n
            .map_or_else(|| "Default".to_owned(), |value| value.to_string()),
        SamplingField::PresencePenalty => settings
            .presence_penalty
            .map_or_else(|| "Default".to_owned(), |value| format!("{value:.2}")),
        SamplingField::FrequencyPenalty => settings
            .frequency_penalty
            .map_or_else(|| "Default".to_owned(), |value| format!("{value:.2}")),
        SamplingField::StopSequences => {
            if settings.stop.is_empty() {
                "Default".to_owned()
            } else {
                settings.stop.join(", ")
            }
        }
        SamplingField::Grammar => settings
            .grammar
            .clone()
            .unwrap_or_else(|| "Default".to_owned()),
    }
}

fn update_sampling_value(
    settings: &mut SamplingSettings,
    field: SamplingField,
    value: &str,
) -> Result<(), String> {
    let value = value.trim();
    let default = value.is_empty() || value.eq_ignore_ascii_case("default");
    match field {
        SamplingField::Temperature => {
            settings.temperature = value.parse().map_err(|_| "Temperature must be a number")?
        }
        SamplingField::MaxTokens => {
            settings.max_tokens = value
                .parse()
                .map_err(|_| "Max tokens must be a whole number")?
        }
        SamplingField::TopP => {
            settings.top_p = value.parse().map_err(|_| "Top P must be a number")?
        }
        SamplingField::TopK => {
            settings.top_k = value.parse().map_err(|_| "Top K must be a whole number")?
        }
        SamplingField::MinP => {
            settings.min_p = value.parse().map_err(|_| "Min P must be a number")?
        }
        SamplingField::RepeatPenalty => {
            settings.repeat_penalty = value
                .parse()
                .map_err(|_| "Repeat penalty must be a number")?
        }
        SamplingField::Seed => {
            settings.seed = (!default)
                .then(|| value.parse())
                .transpose()
                .map_err(|_| "Seed must be a whole number")?
        }
        SamplingField::RepeatLastN => {
            settings.repeat_last_n = (!default)
                .then(|| value.parse())
                .transpose()
                .map_err(|_| "Repeat last N must be a whole number")?
        }
        SamplingField::PresencePenalty => {
            settings.presence_penalty = (!default)
                .then(|| value.parse())
                .transpose()
                .map_err(|_| "Presence penalty must be a number")?
        }
        SamplingField::FrequencyPenalty => {
            settings.frequency_penalty = (!default)
                .then(|| value.parse())
                .transpose()
                .map_err(|_| "Frequency penalty must be a number")?
        }
        SamplingField::StopSequences => {
            settings.stop = if default {
                Vec::new()
            } else {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
        }
        SamplingField::Grammar => settings.grammar = (!default).then(|| value.to_owned()),
        SamplingField::Advanced => {}
    }
    Ok(())
}

fn handle_overlay(
    control: &mut Control,
    ui: &mut UiState,
    key: KeyEvent,
    action: Option<input::UiAction>,
) -> io::Result<bool> {
    let overlay = ui.overlay.take().expect("overlay checked before routing");
    if overlay_consumes_interrupt(action) {
        ui.overlay = Some(overlay);
        return Ok(false);
    }
    let mut next = Some(overlay);
    let mut exit = false;

    match next.take().expect("overlay remains present") {
        Overlay::Error {
            exit_on_close,
            title,
            message,
        } => match action {
            Some(input::UiAction::Activate | input::UiAction::Back) => exit = exit_on_close,
            _ => {
                next = Some(Overlay::Error {
                    title,
                    message,
                    exit_on_close,
                })
            }
        },
        Overlay::ConfirmDelete(id) => match action {
            Some(input::UiAction::Activate) => {
                control.delete_session(&id).map_err(app_error)?;
            }
            Some(input::UiAction::Back) => {}
            _ => next = Some(Overlay::ConfirmDelete(id)),
        },
        Overlay::Control { mut cursor } => {
            let selected = match action {
                Some(input::UiAction::MoveUp) => {
                    cursor = cursor.saturating_sub(1);
                    None
                }
                Some(input::UiAction::MoveDown) => {
                    cursor = (cursor + 1).min(CONTROL_ROWS - 1);
                    None
                }
                _ if control_accelerator(key).is_some() => {
                    match control_accelerator(key).expect("accelerator guard guarantees a value") {
                        'M' => Some(0),
                        'I' => Some(1),
                        'F' => Some(2),
                        'Q' => Some(3),
                        _ => None,
                    }
                }
                Some(input::UiAction::Activate) => Some(cursor),
                Some(input::UiAction::Back) => {
                    next = None;
                    None
                }
                _ => None,
            };
            if let Some(selected) = selected {
                next = match selected {
                    0 => Some(Overlay::LlmParameters {
                        cursor: 0,
                        advanced: false,
                        draft: None,
                    }),
                    1 => Some(Overlay::AdditionalInstructions {
                        editor: TextEditor::editor(
                            &control.get_additional_instructions().map_err(app_error)?,
                        ),
                    }),
                    2 => Some(Overlay::Profiles {
                        cursor: 0,
                        names: control.available_profile_names().map_err(app_error)?,
                    }),
                    3 => {
                        exit = true;
                        None
                    }
                    _ => unreachable!("control cursor is bounded"),
                };
            } else if !matches!(action, Some(input::UiAction::Back)) {
                next = Some(Overlay::Control { cursor });
            }
        }
        Overlay::AdditionalInstructions { mut editor } => {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                control
                    .save_additional_instructions(&editor.content())
                    .map_err(app_error)?;
                next = Some(Overlay::Control { cursor: 1 });
            } else if matches!(action, Some(input::UiAction::Back)) {
                next = Some(Overlay::Control { cursor: 1 });
            } else {
                editor.input(key);
                next = Some(Overlay::AdditionalInstructions { editor });
            }
        }
        Overlay::Profiles { mut cursor, names } => match action {
            Some(input::UiAction::MoveUp) => {
                cursor = cursor.saturating_sub(1);
                next = Some(Overlay::Profiles { cursor, names });
            }
            Some(input::UiAction::MoveDown) => {
                cursor = (cursor + 1).min(names.len().saturating_sub(1));
                next = Some(Overlay::Profiles { cursor, names });
            }
            Some(input::UiAction::Activate) if !names.is_empty() => {
                control
                    .toggle_active_session_profile(&names[cursor])
                    .map_err(app_error)?;
                next = Some(Overlay::Profiles { cursor, names });
            }
            Some(input::UiAction::Back) => next = Some(Overlay::Control { cursor: 2 }),
            _ => next = Some(Overlay::Profiles { cursor, names }),
        },
        Overlay::LlmParameters {
            mut cursor,
            mut advanced,
            mut draft,
        } => {
            let fields = sampling_fields(advanced);
            if let Some(mut editor) = draft.take() {
                if matches!(action, Some(input::UiAction::Back)) {
                    next = Some(Overlay::LlmParameters {
                        cursor,
                        advanced,
                        draft: None,
                    });
                } else if matches!(action, Some(input::UiAction::Activate)) {
                    let field = fields[cursor];
                    let mut settings = control.runtime_sampling();
                    update_sampling_value(&mut settings, field, &editor.content())
                        .map_err(io::Error::other)?;
                    control
                        .update_runtime_sampling(settings)
                        .map_err(app_error)?;
                    next = Some(Overlay::LlmParameters {
                        cursor,
                        advanced,
                        draft: None,
                    });
                } else {
                    editor.input(key);
                    next = Some(Overlay::LlmParameters {
                        cursor,
                        advanced,
                        draft: Some(editor),
                    });
                }
            } else {
                match action {
                    Some(input::UiAction::MoveUp) => cursor = cursor.saturating_sub(1),
                    Some(input::UiAction::MoveDown) => cursor = (cursor + 1).min(fields.len() - 1),
                    Some(input::UiAction::Back) => {
                        next = Some(Overlay::Control { cursor: 0 });
                    }
                    Some(input::UiAction::Activate)
                        if matches!(fields[cursor], SamplingField::Advanced) =>
                    {
                        advanced = !advanced;
                        cursor = cursor.min(sampling_fields(advanced).len() - 1);
                    }
                    Some(input::UiAction::Activate) => {
                        let settings = control.runtime_sampling();
                        draft = Some(TextEditor::editor(&sampling_value(
                            &settings,
                            fields[cursor],
                        )));
                    }
                    _ => {}
                }
                if !matches!(action, Some(input::UiAction::Back)) {
                    next = Some(Overlay::LlmParameters {
                        cursor,
                        advanced,
                        draft,
                    });
                }
            }
        }
    }
    ui.overlay = next;
    Ok(exit)
}

fn app_error(error: Box<dyn std::error::Error>) -> io::Error {
    io::Error::other(error.to_string())
}

fn submit_input<E>(
    input: &mut TextEditor,
    submit: impl FnOnce(String) -> Result<(), E>,
) -> Result<(), E> {
    submit(input.content())?;
    input.take_content();
    Ok(())
}

fn overlay_consumes_interrupt(action: Option<input::UiAction>) -> bool {
    matches!(action, Some(input::UiAction::Interrupt))
}

fn control_accelerator(key: KeyEvent) -> Option<char> {
    match key {
        KeyEvent {
            code: KeyCode::Char(character),
            modifiers: KeyModifiers::NONE,
            ..
        } if character.is_ascii_alphabetic() => Some(character.to_ascii_uppercase()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with(content: &str) -> TextEditor {
        TextEditor::editor(content)
    }

    #[test]
    fn successful_submit_clears_input() {
        let mut input = input_with("message");

        assert_eq!(submit_input(&mut input, |_| Ok::<_, ()>(())), Ok(()));
        assert!(input.is_empty());
    }

    #[test]
    fn failed_submit_preserves_input() {
        let mut input = input_with("message");

        assert_eq!(submit_input(&mut input, |_| Err("failed")), Err("failed"));
        assert_eq!(input.content(), "message");
    }

    #[test]
    fn overlay_consumes_interrupt_instead_of_cancelling_background_work() {
        assert!(overlay_consumes_interrupt(Some(input::UiAction::Interrupt)));
        assert!(!overlay_consumes_interrupt(Some(input::UiAction::Activate)));
    }
}
