use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    execute::{self, ExecResult, ExecutionSettings},
    files,
    model::{
        self, ModelEvent, ModelMessage, ModelParameters, ModelRuntime, ModelRuntimeState,
        SamplingSettings, StartupModelSelection, TokenUsage,
    },
};

const RUNTIME_SETTINGS_PATH: &str = "sessions/runtime.json";
const LEGACY_DEFAULT_SETTINGS_PATH: &str = "sessions/default.json";
const SESSIONS_DIRECTORY: &str = "sessions";
const BASE_SYSTEM_PROMPT_PATH: &str = "prompts/system.md";
const ADDITIONAL_INSTRUCTIONS_PATH: &str = "prompts/instructions.md";
const PROFILES_DIRECTORY: &str = "prompts/profiles";
const PLAYBOOKS_DIRECTORY: &str = "playbooks";
const WORKSPACE_DIRECTORY: &str = "workspace";
const BUNDLED_SYSTEM_PROMPT: &str = include_str!("../prompts/system.md");
const STREAM_PERSIST_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SessionDefaults {
    model: ModelParameters,
    #[serde(default)]
    sampling: SamplingSettings,
    #[serde(default)]
    execution: ExecutionSettings,
}

/// Stable identity for one persisted conversation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct SessionId(String);

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredSession {
    id: SessionId,
    title: String,
    #[serde(default)]
    messages: Vec<StoredMessage>,
    #[serde(default)]
    profiles: Vec<String>,
    #[serde(default)]
    last_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MessageRole {
    User,
    #[serde(alias = "assistant")]
    Model,
    Exec,
    ExecResult,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredMessage {
    role: MessageRole,
    content: String,
    #[serde(default)]
    reasoning_content: String,
    #[serde(default)]
    tool_call_id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    duration_ms: u128,
    #[serde(default)]
    timed_out: bool,
}

/// Read-only session information used by the terminal UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
}

/// Read-only application data for rendering the main work surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSnapshot {
    pub sessions: Vec<SessionSummary>,
    pub active_session: Option<SessionView>,
}

/// Read-only active session content for transcript rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: String,
    pub title: String,
    pub messages: Vec<MessageView>,
    pub profiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageView {
    pub role: String,
    pub content: String,
    pub reasoning_content: String,
    pub status: String,
}

/// Read-only runtime information for the informational TUI rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeView {
    pub model_name: Option<String>,
    pub status: String,
    pub llama_version: &'static str,
    pub context_size: u32,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub context_full: bool,
}

/// Authoritative application boundary between the terminal UI and runtime work.
///
/// Persistent sessions and in-flight turns live here rather than in the UI.
pub struct Control {
    defaults: SessionDefaults,
    startup_model: StartupModelSelection,
    sessions: Vec<StoredSession>,
    active_session: Option<SessionId>,
    runtime: Option<ModelRuntime>,
    pending_completion: Option<PendingTurn>,
    pending_session_write: Option<PendingSessionWrite>,
    pending_model_boundary: Option<PendingModelBoundary>,
    startup_warnings: Vec<String>,
    tool_calls_started: usize,
}

#[derive(Clone)]
struct PendingSessionWrite {
    session_id: SessionId,
    due_at: Instant,
}

enum PendingModelBoundary {
    ToolCalls(Vec<model::ToolCall>),
    Complete,
    Failed(String),
    Cancelled,
    Disconnected,
}

struct PendingTurn {
    session_id: SessionId,
    work: PendingWork,
}

enum PendingWork {
    Model {
        stream: model::ModelStream,
        cancelling: bool,
    },
    Exec {
        call_id: String,
        receiver: Receiver<ExecResult>,
        remaining: Vec<model::ToolCall>,
        cancel: Arc<AtomicBool>,
        cancelled: bool,
    },
}

enum PendingEvent {
    Model {
        event: ModelEvent,
        cancelling: bool,
    },
    ModelCancelled,
    Exec {
        call_id: String,
        result: ExecResult,
        remaining: Vec<model::ToolCall>,
        cancelled: bool,
    },
}

/// Read-only startup selector data for the terminal UI.
///
/// Model paths and selection policy remain inside `Control` and `model`.
pub struct StartupModelSelectionView {
    pub names: Vec<String>,
    pub selected_index: usize,
}

impl Control {
    /// Loads the persistent defaults, creating them on the first startup.
    pub fn initialize() -> Result<Self, Box<dyn std::error::Error>> {
        let path = Path::new(RUNTIME_SETTINGS_PATH);
        let legacy_path = Path::new(LEGACY_DEFAULT_SETTINGS_PATH);
        if !path.exists() && legacy_path.exists() {
            std::fs::rename(legacy_path, path)?;
        }
        let defaults = if path.exists() {
            files::read_json(path)?
        } else {
            let defaults = SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            };
            files::write_json(path, &defaults)?;
            defaults
        };
        defaults.model.validate().map_err(std::io::Error::other)?;
        defaults
            .sampling
            .validate()
            .map_err(std::io::Error::other)?;
        std::fs::create_dir_all(PLAYBOOKS_DIRECTORY)?;
        std::fs::create_dir_all(WORKSPACE_DIRECTORY)?;

        let models = model::discover_models(Path::new(model::MODELS_DIRECTORY))?;
        let startup_model = model::select_startup_model(models, &defaults.model.default_model);
        let (sessions, startup_warnings) = load_sessions()?;
        let active_session = sessions.first().map(|session| session.id.clone());

        let mut control = Self {
            defaults,
            startup_model,
            sessions,
            active_session,
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings,
            tool_calls_started: 0,
        };
        control.start_selected_model()?;
        Ok(control)
    }

    /// Returns structured runtime information for the main work surface.
    pub fn runtime_view(&mut self) -> RuntimeView {
        let (model_name, mut status) = match &self.startup_model {
            StartupModelSelection::NoUsableModels => {
                (None, "No usable models in models/".to_owned())
            }
            StartupModelSelection::Selected(model) => (
                Some(model.name().to_owned()),
                "RUNTIME NOT STARTED".to_owned(),
            ),
            StartupModelSelection::SelectionRequired { .. } => {
                (None, "SELECT A STARTUP MODEL".to_owned())
            }
        };

        if let Some(runtime) = &mut self.runtime {
            status = match runtime.state() {
                Ok(ModelRuntimeState::Starting) => "STARTING".to_owned(),
                Ok(ModelRuntimeState::Ready) => "READY".to_owned(),
                Ok(ModelRuntimeState::Failed(error)) => error,
                Err(error) => format!("runtime poll failed: {error}"),
            };
        }

        let session = self
            .active_session
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| session.id == *id));
        let usage = session.and_then(|session| session.last_usage);
        RuntimeView {
            model_name,
            status,
            llama_version: model::llama_server_version(),
            context_size: self.defaults.model.context_size,
            prompt_tokens: usage.map(|usage| usage.prompt_tokens),
            completion_tokens: usage.map(|usage| usage.completion_tokens),
            context_full: usage.is_some_and(|usage| {
                usage.prompt_tokens.saturating_add(usage.completion_tokens)
                    >= self.defaults.model.context_size
            }),
        }
    }

    /// Returns the sampling settings persisted for every runtime request.
    pub fn runtime_sampling(&self) -> SamplingSettings {
        self.defaults.sampling.clone()
    }

    /// Persists sampling settings used by subsequent model requests.
    pub fn update_runtime_sampling(
        &mut self,
        sampling: SamplingSettings,
    ) -> Result<(), Box<dyn std::error::Error>> {
        sampling.validate().map_err(std::io::Error::other)?;
        let mut defaults = self.defaults.clone();
        defaults.sampling = sampling;
        files::write_json(Path::new(RUNTIME_SETTINGS_PATH), &defaults)?;
        self.defaults = defaults;
        Ok(())
    }

    /// Returns the editable global instructions, or an empty value when none were saved.
    pub fn get_additional_instructions(&self) -> Result<String, Box<dyn std::error::Error>> {
        read_system_prompt(Path::new(ADDITIONAL_INSTRUCTIONS_PATH))
    }

    /// Saves the user's global instructions appended after the bundled system prompt.
    pub fn save_additional_instructions(
        &self,
        instructions: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new(ADDITIONAL_INSTRUCTIONS_PATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, instructions)?;
        Ok(())
    }

    fn base_system_prompt(&self) -> Result<String, Box<dyn std::error::Error>> {
        let path = Path::new(BASE_SYSTEM_PROMPT_PATH);
        if path.exists() {
            read_system_prompt(path)
        } else {
            Ok(BUNDLED_SYSTEM_PROMPT.to_owned())
        }
    }

    /// Lists profile names backed by regular Markdown files in a stable order.
    pub fn available_profile_names(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        profile_names(Path::new(PROFILES_DIRECTORY)).map_err(Into::into)
    }

    /// Adds or removes one profile name from the active session and persists it.
    pub fn toggle_active_session_profile(
        &mut self,
        profile: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(id) = &self.active_session else {
            return Ok(false);
        };
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == *id)
            .expect("active session must exist");
        let mut session = self.sessions[index].clone();
        if let Some(index) = session.profiles.iter().position(|name| name == profile) {
            session.profiles.remove(index);
        } else {
            session.profiles.push(profile.to_owned());
        }
        self.commit_session(index, session)?;
        Ok(true)
    }

    /// Returns the models and initial cursor position for the startup overlay.
    pub fn startup_model_selection(&self) -> Option<StartupModelSelectionView> {
        match &self.startup_model {
            StartupModelSelection::SelectionRequired {
                models,
                selected_index,
            } => Some(StartupModelSelectionView {
                names: models.iter().map(|model| model.name().to_owned()).collect(),
                selected_index: *selected_index,
            }),
            _ => None,
        }
    }

    /// Returns the blocking startup problem when no bundled model is usable.
    pub fn startup_error(&self) -> Option<String> {
        match &self.startup_model {
            StartupModelSelection::NoUsableModels => {
                Some("No usable .gguf model was found in models/.".to_owned())
            }
            _ => None,
        }
    }

    /// Returns and clears recoverable startup warnings collected while loading sessions.
    pub fn take_startup_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.startup_warnings)
    }

    /// Confirms a model selected by the startup overlay.
    ///
    /// The index comes from UI-local cursor state and is checked here before
    /// the application transitions out of model selection.
    pub fn select_startup_model(
        &mut self,
        index: usize,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let model = match &self.startup_model {
            StartupModelSelection::SelectionRequired { models, .. } => models.get(index).cloned(),
            _ => None,
        };

        if let Some(model) = model {
            self.startup_model = StartupModelSelection::Selected(model);
            self.start_selected_model()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns renderable session data without exposing mutable session state.
    pub fn snapshot(&self) -> AppSnapshot {
        let sessions = self.sessions.iter().map(session_summary).collect();
        let active_session = self.active_session.as_ref().and_then(|active_id| {
            self.sessions
                .iter()
                .find(|session| session.id == *active_id)
                .map(session_view)
        });

        AppSnapshot {
            sessions,
            active_session,
        }
    }

    /// Reports whether the single application-wide user turn is still active.
    pub fn turn_active(&self) -> bool {
        self.pending_completion.is_some()
    }

    /// Stops the active model request or local command and returns whether work was active.
    pub fn interrupt(&mut self) -> bool {
        let Some(turn) = &mut self.pending_completion else {
            return false;
        };
        match &mut turn.work {
            PendingWork::Model { stream, cancelling } if !*cancelling => {
                stream.cancel();
                *cancelling = true;
            }
            PendingWork::Model { .. } => return false,
            PendingWork::Exec {
                cancel, cancelled, ..
            } if !*cancelled => {
                cancel.store(true, Ordering::Relaxed);
                *cancelled = true;
            }
            PendingWork::Exec { .. } => return false,
        }
        true
    }

    /// Creates and persists a new session, then makes it active.
    pub fn create_session(&mut self) -> Result<SessionSummary, Box<dyn std::error::Error>> {
        let session = StoredSession {
            id: self.next_session_id()?,
            title: "New session".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        files::write_json(&session_path(&session.id), &session)?;
        self.active_session = Some(session.id.clone());
        let summary = session_summary(&session);
        self.sessions.push(session);
        Ok(summary)
    }

    /// Makes an existing session active by its stable ID.
    pub fn select_session(&mut self, id: &str) -> bool {
        let Some(session) = self.sessions.iter().find(|session| session.id.0 == id) else {
            return false;
        };
        self.active_session = Some(session.id.clone());
        true
    }

    /// Deletes a persisted session by stable ID and selects a neighboring session if needed.
    /// A session with an in-flight completion is retained so worker output always has a live target.
    pub fn delete_session(&mut self, id: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(index) = self.sessions.iter().position(|session| session.id.0 == id) else {
            return Ok(false);
        };
        if self
            .pending_completion
            .as_ref()
            .is_some_and(|turn| turn.session_id.0 == id)
        {
            return Ok(false);
        }

        let deleted_id = self.sessions[index].id.clone();
        std::fs::remove_file(session_path(&deleted_id))?;
        self.sessions.remove(index);
        if self.active_session.as_ref() == Some(&deleted_id) {
            self.active_session = self
                .sessions
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|index| self.sessions.get(index))
                })
                .map(|session| session.id.clone());
        }
        Ok(true)
    }

    /// Appends a user message and assigns a new session's title on its first input.
    pub fn submit(&mut self, content: String) -> Result<(), Box<dyn std::error::Error>> {
        if self.pending_completion.is_some() {
            return Ok(());
        }
        let Some(active_id) = self.active_session.clone() else {
            return Ok(());
        };
        self.ensure_runtime_ready()?;
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == active_id)
            .expect("active session must exist");
        let mut session = self.sessions[index].clone();
        if session.messages.is_empty() {
            session.title = title_from_first_message(&content);
        }
        session.messages.push(StoredMessage {
            role: MessageRole::User,
            content,
            reasoning_content: String::new(),
            tool_call_id: String::new(),
            status: String::new(),
            exit_code: None,
            duration_ms: 0,
            timed_out: false,
        });
        let messages = self.model_messages_for_session(&session)?;
        self.commit_session(index, session)?;
        self.tool_calls_started = 0;
        self.begin_model_request(active_id, messages);
        Ok(())
    }

    /// Incorporates all currently available worker events without waiting.
    pub fn poll(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            self.flush_pending_session_if_due()?;
            if self.pending_model_boundary.is_some() {
                self.finish_model_boundary()?;
                continue;
            }
            let Some((session_id, event)) = self.pending_completion.as_ref().and_then(|turn| {
                let event = match &turn.work {
                    PendingWork::Model { stream, cancelling } => match stream.receiver.try_recv() {
                        Ok(event) => Ok(PendingEvent::Model {
                            event,
                            cancelling: *cancelling,
                        }),
                        Err(TryRecvError::Empty) => Err(TryRecvError::Empty),
                        Err(TryRecvError::Disconnected) if *cancelling => {
                            Ok(PendingEvent::ModelCancelled)
                        }
                        Err(TryRecvError::Disconnected) => Err(TryRecvError::Disconnected),
                    },
                    PendingWork::Exec {
                        call_id,
                        receiver,
                        remaining,
                        cancelled,
                        ..
                    } => receiver.try_recv().map(|result| PendingEvent::Exec {
                        call_id: call_id.clone(),
                        result,
                        remaining: remaining.clone(),
                        cancelled: *cancelled,
                    }),
                };
                match event {
                    Ok(event) => Some((turn.session_id.clone(), Ok(event))),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some((turn.session_id.clone(), Err(()))),
                }
            }) else {
                return Ok(());
            };

            let event = match event {
                Ok(event) => event,
                Err(()) => {
                    self.pending_model_boundary = Some(PendingModelBoundary::Disconnected);
                    continue;
                }
            };
            match event {
                PendingEvent::ModelCancelled => {
                    self.pending_model_boundary = Some(PendingModelBoundary::Cancelled);
                }
                PendingEvent::Model {
                    event:
                        ModelEvent::Delta {
                            content,
                            reasoning_content,
                        },
                    cancelling: false,
                } => self.append_model_delta(&session_id, &content, &reasoning_content)?,
                PendingEvent::Model {
                    event: ModelEvent::ToolCalls(calls),
                    cancelling: false,
                } => {
                    self.pending_model_boundary = Some(PendingModelBoundary::ToolCalls(calls));
                }
                PendingEvent::Model {
                    event: ModelEvent::Usage(usage),
                    cancelling: false,
                } => self.record_usage(&session_id, usage)?,
                PendingEvent::Model {
                    event: ModelEvent::Complete,
                    cancelling: false,
                } => {
                    self.pending_model_boundary = Some(PendingModelBoundary::Complete);
                }
                PendingEvent::Model {
                    event: ModelEvent::Failed(error),
                    cancelling: false,
                } => {
                    self.pending_model_boundary = Some(PendingModelBoundary::Failed(error));
                }
                PendingEvent::Model {
                    cancelling: true, ..
                } => {}
                PendingEvent::Exec {
                    call_id,
                    result,
                    remaining,
                    cancelled,
                } => {
                    self.append_exec_results(
                        &session_id,
                        result,
                        call_id,
                        if cancelled { remaining.as_slice() } else { &[] },
                    )?;
                    self.pending_completion = None;
                    if cancelled {
                        return Ok(());
                    }
                    if let Some((call, remaining)) = remaining.split_first() {
                        self.start_pending_exec(session_id, call.clone(), remaining.to_vec())?;
                    } else {
                        self.start_model_request(session_id)?;
                    }
                }
            }
        }
    }

    /// Releases the local model runtime before returning control to the terminal.
    pub fn shutdown(mut self) -> Result<(), Box<dyn std::error::Error>> {
        let pending_result = if self.pending_completion.is_some() {
            self.interrupt();
            let mut result = Ok(());
            while self.pending_completion.is_some() {
                if let Err(error) = self.poll() {
                    result = Err(error);
                    break;
                }
                if self.pending_completion.is_some() {
                    thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            result
        } else {
            Ok(())
        };
        let runtime_result = self.runtime.as_mut().map(ModelRuntime::stop).transpose();
        let persistence_result = self.flush_pending_session();

        if let Err(error) = pending_result {
            persistence_result?;
            runtime_result?;
            return Err(error);
        }
        persistence_result?;
        runtime_result?;
        Ok(())
    }

    fn next_session_id(&self) -> Result<SessionId, Box<dyn std::error::Error>> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        for sequence in 0_u32.. {
            let suffix = if sequence == 0 {
                String::new()
            } else {
                format!("-{sequence}")
            };
            let id = SessionId(format!("session-{timestamp}{suffix}"));
            if !self.sessions.iter().any(|session| session.id == id) && !session_path(&id).exists()
            {
                return Ok(id);
            }
        }
        unreachable!("the session ID counter cannot be exhausted")
    }

    fn start_selected_model(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let StartupModelSelection::Selected(model) = &self.startup_model else {
            return Ok(());
        };
        self.runtime = Some(ModelRuntime::start(model, &self.defaults.model).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "could not start {path}: {error}. Ensure the bundle was extracted with tar into an executable directory and that {path} has mode 755.",
                    path = model::LLAMA_SERVER_PATH,
                ),
            )
        })?);
        Ok(())
    }

    fn mark_session_dirty(&mut self, session_id: &SessionId) {
        match &self.pending_session_write {
            Some(pending) if pending.session_id == *session_id => {}
            Some(_) => unreachable!("only one session can stream at a time"),
            None => {
                self.pending_session_write = Some(PendingSessionWrite {
                    session_id: session_id.clone(),
                    due_at: Instant::now() + STREAM_PERSIST_INTERVAL,
                });
            }
        }
    }

    fn flush_pending_session_if_due(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self
            .pending_session_write
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.due_at)
        {
            self.flush_pending_session()?;
        }
        Ok(())
    }

    fn flush_pending_session(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(pending) = self.pending_session_write.clone() else {
            return Ok(());
        };
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == pending.session_id)
            .expect("dirty session must exist");
        files::write_json(&session_path(&session.id), session)?;
        self.pending_session_write = None;
        Ok(())
    }

    fn commit_session(
        &mut self,
        index: usize,
        session: StoredSession,
    ) -> Result<(), Box<dyn std::error::Error>> {
        files::write_json(&session_path(&session.id), &session)?;
        if self
            .pending_session_write
            .as_ref()
            .is_some_and(|pending| pending.session_id == session.id)
        {
            self.pending_session_write = None;
        }
        self.sessions[index] = session;
        Ok(())
    }

    fn finish_model_boundary(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let session_id = self
            .pending_completion
            .as_ref()
            .expect("model boundary must have pending work")
            .session_id
            .clone();
        if let Some(PendingModelBoundary::ToolCalls(calls)) = &self.pending_model_boundary {
            let calls = calls.clone();
            let result = self.start_tool_calls(session_id, calls);
            if result.is_ok() || self.pending_completion.is_none() {
                self.pending_model_boundary = None;
            }
            return result;
        }

        self.flush_pending_session()?;
        let boundary = self
            .pending_model_boundary
            .take()
            .expect("checked model boundary");
        self.pending_completion = None;

        match boundary {
            PendingModelBoundary::ToolCalls(_) => unreachable!("tool calls handled above"),
            PendingModelBoundary::Complete | PendingModelBoundary::Cancelled => Ok(()),
            PendingModelBoundary::Failed(error) => {
                Err(std::io::Error::other(format!("model request failed: {error}")).into())
            }
            PendingModelBoundary::Disconnected => {
                Err(std::io::Error::other("turn worker disconnected").into())
            }
        }
    }

    fn append_model_delta(
        &mut self,
        session_id: &SessionId,
        content: &str,
        reasoning_content: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == *session_id)
            .expect("pending session must exist");
        let session = &mut self.sessions[index];
        if !matches!(
            session.messages.last(),
            Some(StoredMessage {
                role: MessageRole::Model,
                ..
            })
        ) {
            session.messages.push(StoredMessage {
                role: MessageRole::Model,
                content: String::new(),
                reasoning_content: String::new(),
                tool_call_id: String::new(),
                status: String::new(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
            });
        }
        let message = session
            .messages
            .last_mut()
            .expect("model message must exist");
        message.content.push_str(content);
        message.reasoning_content.push_str(reasoning_content);
        self.mark_session_dirty(session_id);
        Ok(())
    }

    fn record_usage(
        &mut self,
        session_id: &SessionId,
        usage: TokenUsage,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == *session_id)
            .expect("pending session must exist");
        self.sessions[index].last_usage = Some(usage);
        self.mark_session_dirty(session_id);
        self.flush_pending_session()
    }

    fn start_model_request(
        &mut self,
        session_id: SessionId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_runtime_ready()?;
        let messages = self.model_messages(&session_id)?;
        self.begin_model_request(session_id, messages);
        Ok(())
    }

    fn begin_model_request(&mut self, session_id: SessionId, messages: Vec<ModelMessage>) {
        if let Some(runtime) = &self.runtime {
            self.pending_completion = Some(PendingTurn {
                session_id,
                work: PendingWork::Model {
                    stream: runtime.stream(messages, self.defaults.sampling.clone()),
                    cancelling: false,
                },
            });
        }
    }

    fn ensure_runtime_ready(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let Some(runtime) = &mut self.runtime else {
            return Ok(());
        };
        if let Some(error) = runtime_not_ready_error(runtime.state()?) {
            return Err(std::io::Error::other(error).into());
        }
        Ok(())
    }

    fn start_tool_calls(
        &mut self,
        session_id: SessionId,
        calls: Vec<model::ToolCall>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
            .expect("pending session must exist");
        let mut session = self.sessions[index].clone();
        let mut runnable = Vec::new();
        let mut tool_calls_started = self.tool_calls_started;
        for call in calls {
            session.messages.push(StoredMessage {
                role: MessageRole::Exec,
                content: call.command.clone(),
                reasoning_content: String::new(),
                tool_call_id: call.id.clone(),
                status: String::new(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
            });
            if tool_calls_started < self.defaults.execution.max_tool_calls_per_turn {
                tool_calls_started += 1;
                runnable.push(call);
            } else {
                push_exec_result(
                    &mut session,
                    ExecResult {
                        stdout: String::new(),
                        stderr: format!(
                            "Tool call was not run: the limit of {} calls per user turn was reached.",
                            self.defaults.execution.max_tool_calls_per_turn
                        ),
                        status: "tool call limit reached".to_owned(),
                        exit_code: None,
                        duration_ms: 0,
                        timed_out: false,
                    },
                    call.id,
                );
            }
        }

        self.commit_session(index, session)?;
        self.pending_completion = None;
        self.tool_calls_started = tool_calls_started;

        if let Some((call, remaining)) = runnable.split_first() {
            self.start_pending_exec(session_id, call.clone(), remaining.to_vec())?;
        } else {
            self.start_model_request(session_id)?;
        }
        Ok(())
    }

    fn start_pending_exec(
        &mut self,
        session_id: SessionId,
        call: model::ToolCall,
        remaining: Vec<model::ToolCall>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_completion = Some(PendingTurn {
            session_id,
            work: PendingWork::Exec {
                call_id: call.id,
                receiver: execute_in_background_cancellable(
                    call.command,
                    self.defaults.execution.clone(),
                    Arc::clone(&cancel),
                ),
                remaining,
                cancel,
                cancelled: false,
            },
        });
        Ok(())
    }

    fn append_exec_results(
        &mut self,
        session_id: &SessionId,
        result: ExecResult,
        tool_call_id: String,
        skipped: &[model::ToolCall],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let index = self
            .sessions
            .iter()
            .position(|session| session.id == *session_id)
            .expect("pending session must exist");
        let mut session = self.sessions[index].clone();
        push_exec_result(&mut session, result, tool_call_id);
        for call in skipped {
            push_exec_result(
                &mut session,
                ExecResult {
                    stdout: String::new(),
                    stderr: "Tool call was not run because the user interrupted the turn."
                        .to_owned(),
                    status: "not run: interrupted".to_owned(),
                    exit_code: None,
                    duration_ms: 0,
                    timed_out: false,
                },
                call.id.clone(),
            );
        }
        self.commit_session(index, session)
    }

    fn model_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<ModelMessage>, Box<dyn std::error::Error>> {
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == *session_id)
            .expect("pending session must exist");
        self.model_messages_for_session(session)
    }

    fn model_messages_for_session(
        &self,
        session: &StoredSession,
    ) -> Result<Vec<ModelMessage>, Box<dyn std::error::Error>> {
        let mut messages = vec![ModelMessage::System(self.combined_system_prompt()?)];
        for profile in &session.profiles {
            messages.push(ModelMessage::System(read_profile(profile)?));
        }
        for message in &session.messages {
            match message.role {
                MessageRole::User => messages.push(ModelMessage::User(message.content.clone())),
                // Reasoning is presentation-only model output. A cancelled stream may leave
                // only reasoning behind, which is not a valid assistant history message.
                MessageRole::Model if !message.content.is_empty() => {
                    messages.push(ModelMessage::Model {
                        content: message.content.clone(),
                        tool_calls: Vec::new(),
                    })
                }
                MessageRole::Model => {}
                MessageRole::Exec => {
                    let call = model::ToolCall {
                        id: message.tool_call_id.clone(),
                        command: message.content.clone(),
                    };
                    match messages.last_mut() {
                        Some(ModelMessage::Model { tool_calls, .. }) => tool_calls.push(call),
                        _ => messages.push(ModelMessage::Model {
                            content: String::new(),
                            tool_calls: vec![call],
                        }),
                    }
                }
                MessageRole::ExecResult => messages.push(ModelMessage::ToolResult {
                    tool_call_id: message.tool_call_id.clone(),
                    content: serde_json::json!({
                        "status": message.status,
                        "exit_code": message.exit_code,
                        "stdout": message.content,
                        "stderr": message.reasoning_content,
                        "duration_ms": message.duration_ms,
                        "timed_out": message.timed_out,
                    })
                    .to_string(),
                }),
            }
        }
        Ok(messages)
    }

    fn combined_system_prompt(&self) -> Result<String, Box<dyn std::error::Error>> {
        let base = self.base_system_prompt()?;
        let instructions = self.get_additional_instructions()?;
        Ok(format!(
            "{}\n\n{}",
            combine_system_instructions(base, instructions),
            execution_privilege_context()
        ))
    }
}

fn push_exec_result(session: &mut StoredSession, result: ExecResult, tool_call_id: String) {
    session.messages.push(StoredMessage {
        role: MessageRole::ExecResult,
        content: result.stdout,
        reasoning_content: result.stderr,
        tool_call_id,
        status: result.status,
        exit_code: result.exit_code,
        duration_ms: result.duration_ms,
        timed_out: result.timed_out,
    });
}

fn execution_privilege_context() -> &'static str {
    if execute::running_as_administrator() {
        "SigmaRift is running with administrator privileges. Commands already inherit them; do not use sudo."
    } else {
        "SigmaRift is not running with administrator privileges. Do not use sudo: interactive password prompts are unsupported. Tell the user when administrator privileges are required."
    }
}

fn runtime_not_ready_error(state: ModelRuntimeState) -> Option<String> {
    match state {
        ModelRuntimeState::Ready => None,
        ModelRuntimeState::Starting => Some("model runtime is still starting".to_owned()),
        ModelRuntimeState::Failed(error) => Some(format!("model runtime failed: {error}")),
    }
}

fn combine_system_instructions(base: String, instructions: String) -> String {
    if instructions.is_empty() {
        base
    } else {
        format!("{base}\n\n{instructions}")
    }
}

#[cfg(test)]
fn execute_in_background(command: String, settings: ExecutionSettings) -> Receiver<ExecResult> {
    execute_in_background_cancellable(command, settings, Arc::new(AtomicBool::new(false)))
}

fn execute_in_background_cancellable(
    command: String,
    settings: ExecutionSettings,
    cancelled: Arc<AtomicBool>,
) -> Receiver<ExecResult> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = execute::execute_with_settings_and_cancel(&command, &settings, &cancelled)
            .unwrap_or_else(|error| ExecResult {
                stdout: String::new(),
                stderr: error.to_string(),
                status: "failed to start".to_owned(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
            });
        let _ = sender.send(result);
    });
    receiver
}

fn profile_names(directory: &Path) -> Result<Vec<String>, std::io::Error> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Some(name) = path.file_stem().and_then(|name| name.to_str())
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn read_system_prompt(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    match std::fs::read_to_string(path) {
        Ok(prompt) => Ok(prompt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

fn read_profile(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    if name.is_empty() || Path::new(name).components().count() != 1 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid profile name").into(),
        );
    }
    Ok(std::fs::read_to_string(
        Path::new(PROFILES_DIRECTORY).join(format!("{name}.md")),
    )?)
}

fn load_sessions() -> Result<(Vec<StoredSession>, Vec<String>), Box<dyn std::error::Error>> {
    load_sessions_from(Path::new(SESSIONS_DIRECTORY))
}

fn load_sessions_from(
    directory: &Path,
) -> Result<(Vec<StoredSession>, Vec<String>), Box<dyn std::error::Error>> {
    let mut sessions = Vec::new();
    let mut warnings = Vec::new();
    for path in files::json_files(directory)? {
        if path.file_name().is_some_and(|name| {
            name == Path::new(RUNTIME_SETTINGS_PATH).file_name().unwrap()
                || name == Path::new(LEGACY_DEFAULT_SETTINGS_PATH).file_name().unwrap()
        }) {
            continue;
        }
        let session: StoredSession = match files::read_json(&path) {
            Ok(session) => session,
            Err(error) => {
                warnings.push(format!(
                    "Skipped invalid session {}: {error}",
                    path.display()
                ));
                continue;
            }
        };
        if !session_id_is_valid(&session.id) {
            warnings.push(format!(
                "Skipped session with an invalid ID: {}",
                path.display()
            ));
            continue;
        }
        if path.file_stem().and_then(|name| name.to_str()) != Some(&session.id.0) {
            warnings.push(format!(
                "Skipped session whose ID does not match its file name: {}",
                path.display()
            ));
            continue;
        }
        if sessions
            .iter()
            .any(|stored: &StoredSession| stored.id == session.id)
        {
            warnings.push(format!("Skipped duplicate session: {}", path.display()));
            continue;
        }
        sessions.push(session);
    }
    Ok((sessions, warnings))
}

fn session_path(id: &SessionId) -> PathBuf {
    Path::new(SESSIONS_DIRECTORY).join(format!("{}.json", id.0))
}

fn session_id_is_valid(id: &SessionId) -> bool {
    !id.0.is_empty()
        && id
            .0
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn session_summary(session: &StoredSession) -> SessionSummary {
    SessionSummary {
        id: session.id.0.clone(),
        title: session.title.clone(),
    }
}

fn session_view(session: &StoredSession) -> SessionView {
    SessionView {
        id: session.id.0.clone(),
        title: session.title.clone(),
        messages: session
            .messages
            .iter()
            .map(|message| MessageView {
                role: match message.role {
                    MessageRole::User => "user".to_owned(),
                    MessageRole::Model => "model".to_owned(),
                    MessageRole::Exec => "exec".to_owned(),
                    MessageRole::ExecResult => "exec_result".to_owned(),
                },
                content: message.content.clone(),
                reasoning_content: message.reasoning_content.clone(),
                status: message.status.clone(),
            })
            .collect(),
        profiles: session.profiles.clone(),
    }
}

fn title_from_first_message(content: &str) -> String {
    let line = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("New session");
    let characters = line.trim().chars().collect::<Vec<_>>();
    if characters.len() <= 48 {
        return characters.into_iter().collect();
    }
    format!("{}...", characters.into_iter().take(45).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread, time::Duration};

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sigmarift-{name}-{}", std::process::id()))
    }

    fn control_with_session(id: SessionId) -> Control {
        Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Test session".to_owned(),
                messages: Vec::new(),
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        }
    }

    #[test]
    fn sessions_without_profiles_deserialize_with_an_empty_selection() {
        let session: StoredSession =
            serde_json::from_str(r#"{"id":"old","title":"Older session","messages":[]}"#).unwrap();

        assert!(session.profiles.is_empty());
    }

    #[test]
    fn session_ids_allow_only_safe_file_name_characters() {
        assert!(session_id_is_valid(&SessionId("session-123_A".to_owned())));
        assert!(!session_id_is_valid(&SessionId("../outside".to_owned())));
        assert!(!session_id_is_valid(&SessionId("session/name".to_owned())));
        assert!(!session_id_is_valid(&SessionId(String::new())));
    }

    #[test]
    fn invalid_session_files_are_skipped_with_a_warning() {
        let directory = temporary_path("invalid-sessions");
        let valid = StoredSession {
            id: SessionId("valid".to_owned()),
            title: "Valid".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        files::write_json(&directory.join("valid.json"), &valid).unwrap();
        fs::write(directory.join("broken.json"), "{not json").unwrap();
        fs::write(
            directory.join("runtime.json"),
            "{runtime is loaded separately",
        )
        .unwrap();

        let (sessions, warnings) = load_sessions_from(&directory).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, valid.id);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("broken.json"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn model_deltas_update_memory_and_are_throttled_on_disk() {
        let id = SessionId(format!("delta-throttle-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();

        control
            .append_model_delta(&id, "Hello", "thinking")
            .unwrap();
        let first_deadline = control.pending_session_write.as_ref().unwrap().due_at;
        control.append_model_delta(&id, " world", "").unwrap();

        let memory = &control.snapshot().active_session.unwrap().messages[0];
        assert_eq!(memory.content, "Hello world");
        assert_eq!(memory.reasoning_content, "thinking");
        assert_eq!(
            control.pending_session_write.as_ref().unwrap().due_at,
            first_deadline
        );
        let disk: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert!(disk.messages.is_empty());

        control.pending_session_write.as_mut().unwrap().due_at = Instant::now();
        control.poll().unwrap();
        let disk: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert_eq!(disk.messages[0].content, "Hello world");
        assert!(control.pending_session_write.is_none());
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn complete_forces_stream_persistence_before_finishing() {
        let id = SessionId(format!("delta-complete-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        control.pending_completion = Some(PendingTurn {
            session_id: id.clone(),
            work: PendingWork::Model {
                stream: model::ModelStream::for_test(receiver),
                cancelling: false,
            },
        });
        sender
            .send(ModelEvent::Delta {
                content: "final".to_owned(),
                reasoning_content: String::new(),
            })
            .unwrap();
        sender.send(ModelEvent::Complete).unwrap();

        control.poll().unwrap();

        assert!(!control.turn_active());
        let disk: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert_eq!(disk.messages[0].content, "final");
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn cancellation_flushes_model_output_already_received() {
        let id = SessionId(format!("delta-cancel-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        control.pending_completion = Some(PendingTurn {
            session_id: id.clone(),
            work: PendingWork::Model {
                stream: model::ModelStream::for_test(receiver),
                cancelling: false,
            },
        });
        sender
            .send(ModelEvent::Delta {
                content: "partial".to_owned(),
                reasoning_content: String::new(),
            })
            .unwrap();
        control.poll().unwrap();

        assert!(control.interrupt());
        drop(sender);
        control.poll().unwrap();

        assert!(!control.turn_active());
        let disk: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert_eq!(disk.messages[0].content, "partial");
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn tool_call_boundary_persists_output_and_calls_together() {
        let id = SessionId(format!("delta-tools-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        control.defaults.execution.max_tool_calls_per_turn = 0;
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        control.pending_completion = Some(PendingTurn {
            session_id: id.clone(),
            work: PendingWork::Model {
                stream: model::ModelStream::for_test(receiver),
                cancelling: false,
            },
        });
        sender
            .send(ModelEvent::Delta {
                content: "Checking".to_owned(),
                reasoning_content: String::new(),
            })
            .unwrap();
        sender
            .send(ModelEvent::ToolCalls(vec![model::ToolCall {
                id: "call".to_owned(),
                command: "printf ignored".to_owned(),
            }]))
            .unwrap();

        control.poll().unwrap();

        assert!(!control.turn_active());
        assert!(control.pending_session_write.is_none());
        let disk: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert_eq!(disk.messages.len(), 3);
        assert_eq!(disk.messages[0].content, "Checking");
        assert_eq!(disk.messages[1].role, MessageRole::Exec);
        assert_eq!(disk.messages[2].role, MessageRole::ExecResult);
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn tool_follow_up_error_clears_the_consumed_model_boundary() {
        let id = SessionId(format!("tool-follow-up-error-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        control.defaults.execution.max_tool_calls_per_turn = 0;
        control.sessions[0]
            .profiles
            .push(format!("missing-profile-{}", std::process::id()));
        let (_sender, receiver) = std::sync::mpsc::channel();
        control.pending_completion = Some(PendingTurn {
            session_id: id.clone(),
            work: PendingWork::Model {
                stream: model::ModelStream::for_test(receiver),
                cancelling: false,
            },
        });
        control.pending_model_boundary =
            Some(PendingModelBoundary::ToolCalls(vec![model::ToolCall {
                id: "rejected".to_owned(),
                command: "printf ignored".to_owned(),
            }]));

        assert!(control.poll().is_err());
        assert!(!control.turn_active());
        assert!(control.pending_model_boundary.is_none());
        control.poll().unwrap();
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn interrupt_records_results_for_tool_calls_that_were_not_started() {
        let id = SessionId(format!("cancelled-tools-{}", std::process::id()));
        let mut control = control_with_session(id.clone());
        for (call_id, command) in [("first", "sleep 1"), ("second", "printf second")] {
            control.sessions[0].messages.push(StoredMessage {
                role: MessageRole::Exec,
                content: command.to_owned(),
                reasoning_content: String::new(),
                tool_call_id: call_id.to_owned(),
                status: String::new(),
                exit_code: None,
                duration_ms: 0,
                timed_out: false,
            });
        }
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(ExecResult {
                stdout: String::new(),
                stderr: String::new(),
                status: "interrupted by user".to_owned(),
                exit_code: None,
                duration_ms: 1,
                timed_out: false,
            })
            .unwrap();
        control.pending_completion = Some(PendingTurn {
            session_id: id.clone(),
            work: PendingWork::Exec {
                call_id: "first".to_owned(),
                receiver,
                remaining: vec![model::ToolCall {
                    id: "second".to_owned(),
                    command: "printf second".to_owned(),
                }],
                cancel: Arc::new(AtomicBool::new(true)),
                cancelled: true,
            },
        });

        control.poll().unwrap();

        let results = control.sessions[0]
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::ExecResult)
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id, "first");
        assert_eq!(results[1].tool_call_id, "second");
        assert_eq!(results[1].status, "not run: interrupted");
        fs::remove_file(session_path(&id)).unwrap();
    }

    #[test]
    fn older_runtime_files_receive_default_execution_limits() {
        let defaults: SessionDefaults =
            serde_json::from_str(r#"{"model":{"context_size":8192}}"#).unwrap();

        assert_eq!(defaults.execution, ExecutionSettings::default());
    }

    #[test]
    fn missing_global_prompt_is_empty_and_profiles_are_sorted_markdown_names() {
        let directory = temporary_path("profiles");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("zeta.md"), "Zeta").unwrap();
        fs::write(directory.join("alpha.MD"), "Alpha").unwrap();
        fs::write(directory.join("ignored.txt"), "Ignored").unwrap();

        assert_eq!(
            read_system_prompt(&directory.join("system.md")).unwrap(),
            ""
        );
        assert_eq!(profile_names(&directory).unwrap(), vec!["alpha", "zeta"]);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn snapshot_exposes_the_active_session_by_stable_id() {
        let first = StoredSession {
            id: SessionId("first".to_owned()),
            title: "First".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        let second = StoredSession {
            id: SessionId("second".to_owned()),
            title: "Second".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        let control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![first, second],
            active_session: Some(SessionId("second".to_owned())),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        let snapshot = control.snapshot();
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(snapshot.active_session.unwrap().id, "second");
    }

    #[test]
    fn interrupted_model_turn_remains_active_until_its_stream_closes() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: Vec::new(),
            active_session: None,
            runtime: None,
            pending_completion: Some(PendingTurn {
                session_id: SessionId("cancelled".to_owned()),
                work: PendingWork::Model {
                    stream: model::ModelStream::for_test(receiver),
                    cancelling: false,
                },
            }),
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        assert!(control.interrupt());
        assert!(control.turn_active());
        drop(sender);
        control.poll().unwrap();
        assert!(!control.turn_active());
    }

    #[test]
    fn runtime_must_be_ready_before_a_request_starts() {
        assert_eq!(runtime_not_ready_error(ModelRuntimeState::Ready), None);
        assert_eq!(
            runtime_not_ready_error(ModelRuntimeState::Starting).as_deref(),
            Some("model runtime is still starting")
        );
        assert_eq!(
            runtime_not_ready_error(ModelRuntimeState::Failed("exited".to_owned())).as_deref(),
            Some("model runtime failed: exited")
        );
    }

    #[test]
    fn selected_model_without_a_running_runtime_is_not_reported_as_ready() {
        let directory = temporary_path("runtime-not-started");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("missing.gguf"), "").unwrap();
        let startup_model = model::select_startup_model(
            model::discover_models(&directory).unwrap(),
            "missing.gguf",
        );
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model,
            sessions: Vec::new(),
            active_session: None,
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        assert_eq!(control.runtime_view().status, "RUNTIME NOT STARTED");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shutdown_interrupts_an_active_exec_command() {
        let id = SessionId(format!("shutdown-exec-{}", std::process::id()));
        let marker = temporary_path("shutdown-marker");
        let _ = fs::remove_file(&marker);
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Shutdown exec".to_owned(),
                messages: Vec::new(),
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id.clone()),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };
        files::write_json(&session_path(&id), &control.sessions[0]).unwrap();
        control
            .start_tool_calls(
                id.clone(),
                vec![model::ToolCall {
                    id: "call-shutdown".to_owned(),
                    command: format!("sleep 5; touch {}", marker.display()),
                }],
            )
            .unwrap();

        control.shutdown().unwrap();

        assert!(!marker.exists());
        let _ = fs::remove_file(session_path(&id));
    }

    #[test]
    fn full_context_usage_is_persisted_and_marks_the_runtime_display() {
        let id = SessionId(format!("usage-limit-{}", std::process::id()));
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters {
                    context_size: 100,
                    ..ModelParameters::default()
                },
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Usage limit".to_owned(),
                messages: Vec::new(),
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id.clone()),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        control
            .record_usage(
                &id,
                TokenUsage {
                    prompt_tokens: 80,
                    completion_tokens: 20,
                },
            )
            .unwrap();

        let runtime = control.runtime_view();
        assert_eq!(runtime.prompt_tokens, Some(80));
        assert_eq!(runtime.completion_tokens, Some(20));
        assert!(runtime.context_full);
        control.submit("Still allowed".to_owned()).unwrap();
        let persisted: StoredSession = files::read_json(&session_path(&id)).unwrap();
        assert_eq!(persisted.last_usage.unwrap().completion_tokens, 20);
        assert_eq!(persisted.messages.last().unwrap().content, "Still allowed");
        let _ = fs::remove_file(session_path(&id));
    }

    #[test]
    fn no_usable_model_has_a_blocking_startup_error() {
        let control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: Vec::new(),
            active_session: None,
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        assert_eq!(
            control.startup_error().as_deref(),
            Some("No usable .gguf model was found in models/.")
        );
    }

    #[test]
    fn delete_session_removes_the_active_session_and_selects_its_neighbor() {
        let deleted_id = format!("delete-test-{}", std::process::id());
        let first = StoredSession {
            id: SessionId(deleted_id.clone()),
            title: "First".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        let second = StoredSession {
            id: SessionId("second".to_owned()),
            title: "Second".to_owned(),
            messages: Vec::new(),
            profiles: Vec::new(),
            last_usage: None,
        };
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![first, second],
            active_session: Some(SessionId(deleted_id.clone())),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        let path = session_path(&SessionId(deleted_id.clone()));
        files::write_json(&path, &control.sessions[0]).unwrap();
        assert!(control.delete_session(&deleted_id).unwrap());
        assert_eq!(control.snapshot().active_session.unwrap().id, "second");
        assert!(!path.exists());
    }

    #[test]
    fn model_history_includes_the_system_prompt_and_tool_exchange() {
        let id = SessionId("tool-history".to_owned());
        let control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Tool history".to_owned(),
                messages: vec![
                    StoredMessage {
                        role: MessageRole::User,
                        content: "Inspect the host".to_owned(),
                        reasoning_content: String::new(),
                        tool_call_id: String::new(),
                        status: String::new(),
                        exit_code: None,
                        duration_ms: 0,
                        timed_out: false,
                    },
                    StoredMessage {
                        role: MessageRole::Exec,
                        content: "uname -s".to_owned(),
                        reasoning_content: String::new(),
                        tool_call_id: "call-1".to_owned(),
                        status: String::new(),
                        exit_code: None,
                        duration_ms: 0,
                        timed_out: false,
                    },
                    StoredMessage {
                        role: MessageRole::ExecResult,
                        content: "Linux\n".to_owned(),
                        reasoning_content: String::new(),
                        tool_call_id: "call-1".to_owned(),
                        status: "exit status: 0".to_owned(),
                        exit_code: Some(0),
                        duration_ms: 12,
                        timed_out: false,
                    },
                    StoredMessage {
                        role: MessageRole::Model,
                        content: String::new(),
                        reasoning_content: "Interrupted while planning the next step.".to_owned(),
                        tool_call_id: String::new(),
                        status: String::new(),
                        exit_code: None,
                        duration_ms: 0,
                        timed_out: false,
                    },
                ],
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id.clone()),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        let messages = control.model_messages(&id).unwrap();
        assert_eq!(messages.len(), 4);
        assert!(matches!(messages[0], ModelMessage::System(_)));
        assert!(matches!(
            &messages[1],
            ModelMessage::User(content) if content == "Inspect the host"
        ));
        assert!(matches!(
            &messages[2],
            ModelMessage::Model { tool_calls, .. }
                if tool_calls == &vec![model::ToolCall {
                    id: "call-1".to_owned(),
                    command: "uname -s".to_owned(),
                }]
        ));
        let ModelMessage::ToolResult {
            tool_call_id,
            content,
        } = &messages[3]
        else {
            panic!("expected tool result");
        };
        assert_eq!(tool_call_id, "call-1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(content).unwrap(),
            serde_json::json!({
                "status": "exit status: 0",
                "exit_code": 0,
                "stdout": "Linux\n",
                "stderr": "",
                "duration_ms": 12,
                "timed_out": false,
            })
        );
    }

    #[test]
    fn bundled_system_prompt_defines_workspace_and_execution_contract() {
        let prompt = BUNDLED_SYSTEM_PROMPT;

        assert!(prompt.contains("When output is truncated"));
        assert!(prompt.contains("ask the user to specify"));
        assert!(prompt.contains("Do not guess or repeat a broad command"));
        assert!(prompt.contains("limited number of tool calls per user turn"));
        assert!(prompt.contains("`workspace/` is your working area"));
        assert!(prompt.contains("`playbooks/` contains user-managed Markdown playbooks"));
    }

    #[test]
    fn additional_instructions_append_without_replacing_the_base_contract() {
        assert_eq!(
            combine_system_instructions("Base instructions".to_owned(), "Use Estonian".to_owned()),
            "Base instructions\n\nUse Estonian"
        );
        assert_eq!(
            combine_system_instructions("Base instructions".to_owned(), String::new()),
            "Base instructions"
        );
    }

    #[test]
    fn execution_context_tells_the_model_about_runtime_privileges() {
        let context = execution_privilege_context();

        assert!(context.contains("administrator privileges"));
        assert!(context.contains("do not use sudo"));
    }

    #[test]
    fn exec_worker_does_not_block_the_control_thread() {
        let receiver = execute_in_background(
            "sleep 0.05; printf done".to_owned(),
            ExecutionSettings::default(),
        );

        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
        let result = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result.stdout, "done");
    }

    #[test]
    fn exec_is_visible_before_its_background_result_arrives() {
        let id = SessionId(format!("async-exec-{}", std::process::id()));
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings::default(),
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Async execution".to_owned(),
                messages: Vec::new(),
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id.clone()),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        control
            .start_tool_calls(
                id.clone(),
                vec![model::ToolCall {
                    id: "call-async".to_owned(),
                    command: "sleep 0.05; printf done".to_owned(),
                }],
            )
            .unwrap();

        assert!(control.turn_active());
        assert!(matches!(
            control.snapshot().active_session.unwrap().messages.last(),
            Some(message) if message.role == "exec"
        ));

        for _ in 0..20 {
            control.poll().unwrap();
            if matches!(
                control.snapshot().active_session.unwrap().messages.last(),
                Some(message) if message.role == "exec_result"
            ) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(!control.turn_active());
        assert!(matches!(
            control.snapshot().active_session.unwrap().messages.last(),
            Some(message) if message.role == "exec_result" && message.content == "done"
        ));
        let _ = fs::remove_file(session_path(&id));
    }

    #[test]
    fn tool_call_limit_returns_a_result_without_running_excess_calls() {
        let id = SessionId(format!("tool-limit-{}", std::process::id()));
        let mut control = Control {
            defaults: SessionDefaults {
                model: ModelParameters::default(),
                sampling: SamplingSettings::default(),
                execution: ExecutionSettings {
                    max_tool_calls_per_turn: 1,
                    ..ExecutionSettings::default()
                },
            },
            startup_model: StartupModelSelection::NoUsableModels,
            sessions: vec![StoredSession {
                id: id.clone(),
                title: "Tool limit".to_owned(),
                messages: Vec::new(),
                profiles: Vec::new(),
                last_usage: None,
            }],
            active_session: Some(id.clone()),
            runtime: None,
            pending_completion: None,
            pending_session_write: None,
            pending_model_boundary: None,
            startup_warnings: Vec::new(),
            tool_calls_started: 0,
        };

        control
            .start_tool_calls(
                id.clone(),
                vec![
                    model::ToolCall {
                        id: "call-first".to_owned(),
                        command: "printf first".to_owned(),
                    },
                    model::ToolCall {
                        id: "call-rejected".to_owned(),
                        command: "printf should-not-run".to_owned(),
                    },
                ],
            )
            .unwrap();

        let messages = &control.snapshot().active_session.unwrap().messages;
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "exec");
        assert_eq!(messages[1].role, "exec");
        assert_eq!(messages[2].role, "exec_result");
        assert_eq!(messages[2].status, "tool call limit reached");

        for _ in 0..20 {
            control.poll().unwrap();
            if !control.turn_active() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        let messages = &control.snapshot().active_session.unwrap().messages;
        assert!(
            messages
                .iter()
                .any(|message| { message.role == "exec_result" && message.content == "first" })
        );
        assert!(messages.iter().all(|message| {
            message.role != "exec_result" || !message.content.contains("should-not-run")
        }));
        let _ = fs::remove_file(session_path(&id));
    }
}
