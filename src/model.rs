use std::{
    fs,
    io::{self, BufRead, BufReader, ErrorKind, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};

pub const MODELS_DIRECTORY: &str = "models";
pub const LLAMA_SERVER_PATH: &str = "runtime/llama-server";
pub const DEFAULT_MODEL_NAME: &str = "gemma-4-E2B-it-Q4_K_M.gguf";
const MODEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MODEL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Returns the version of the checked-in llama.cpp runtime.
pub fn llama_server_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();

    VERSION
        .get_or_init(|| detect_llama_server_version().unwrap_or_else(|_| "unknown".to_owned()))
        .as_str()
}

fn detect_llama_server_version() -> io::Result<String> {
    let output = Command::new(LLAMA_SERVER_PATH).arg("--version").output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "llama-server --version exited with {}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_llama_server_version(&stdout)
        .or_else(|| parse_llama_server_version(&stderr))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "missing llama-server version"))
}

fn parse_llama_server_version(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("version: "))?
        .split_whitespace()
        .next()
}

/// A usable GGUF file bundled with SigmaRift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableModel {
    name: String,
    path: PathBuf,
}

impl AvailableModel {
    fn new(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_owned();
        Some(Self {
            name,
            path: path.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The local llama-server process selected for this application run.
pub struct ModelRuntime {
    child: Child,
    address: SocketAddr,
    model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRuntimeState {
    Starting,
    Ready,
    Failed(String),
}

impl ModelRuntime {
    /// Starts the bundled server on an unused loopback port.
    pub fn start(model: &AvailableModel, parameters: &ModelParameters) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);

        let child = Command::new(LLAMA_SERVER_PATH)
            .args(server_args(model, parameters, address.port()))
            // Server logs must not corrupt the alternate-screen TUI or block on a full pipe.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(Self {
            child,
            address,
            model_name: model.name.clone(),
        })
    }

    /// Polls without waiting for model loading to finish.
    pub fn state(&mut self) -> io::Result<ModelRuntimeState> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(ModelRuntimeState::Failed(format!(
                "llama-server exited with {status}"
            )));
        }

        Ok(if server_is_healthy(self.address)? {
            ModelRuntimeState::Ready
        } else {
            ModelRuntimeState::Starting
        })
    }

    /// Stops the server before SigmaRift returns control to the terminal.
    pub fn stop(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
            self.child.wait()?;
        }
        Ok(())
    }

    /// Streams one OpenAI-compatible chat request on a worker thread.
    pub fn stream(&self, messages: Vec<ModelMessage>, sampling: SamplingSettings) -> ModelStream {
        let address = self.address;
        let model_name = self.model_name.clone();
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let socket = Arc::new(Mutex::new(None));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_socket = Arc::clone(&socket);
        thread::spawn(move || {
            if let Err(error) = stream_chat_completion(
                address,
                &model_name,
                messages,
                &sampling,
                &sender,
                &worker_cancelled,
                &worker_socket,
            ) && !worker_cancelled.load(Ordering::Relaxed)
            {
                let _ = sender.send(ModelEvent::Failed(error));
            }
        });
        ModelStream {
            receiver,
            cancelled,
            socket,
        }
    }
}

/// A conversation message passed from persistent control state to the model boundary.
#[derive(Debug, Clone)]
pub enum ModelMessage {
    System(String),
    User(String),
    Model {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

/// One completed call to SigmaRift's single local execution tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub command: String,
}

/// An event emitted by the streaming model worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    Delta {
        content: String,
        reasoning_content: String,
    },
    ToolCalls(Vec<ToolCall>),
    Usage(TokenUsage),
    Complete,
    Failed(String),
}

/// A running model request that can be interrupted by closing its HTTP socket.
pub struct ModelStream {
    pub receiver: Receiver<ModelEvent>,
    cancelled: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
}

impl ModelStream {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Ok(socket) = self.socket.lock()
            && let Some(socket) = socket.as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(receiver: Receiver<ModelEvent>) -> Self {
        Self {
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
            socket: Arc::new(Mutex::new(None)),
        }
    }
}

/// Token accounting reported by llama-server for one completed model request.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

fn server_args(model: &AvailableModel, parameters: &ModelParameters, port: u16) -> Vec<String> {
    vec![
        "--model".to_owned(),
        model.path.display().to_string(),
        "--ctx-size".to_owned(),
        parameters.context_size.to_string(),
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        port.to_string(),
        "--no-ui".to_owned(),
    ]
}

fn server_is_healthy(address: SocketAddr) -> io::Result<bool> {
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(10)) {
        Ok(stream) => stream,
        Err(_) => return Ok(false),
    };
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;

    let mut response = String::new();
    match stream.read_to_string(&mut response) {
        Ok(_) => Ok(is_successful_health_response(&response)),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::ConnectionReset
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn is_successful_health_response(response: &str) -> bool {
    response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200")
}

#[derive(Serialize)]
struct HttpMessage<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<HttpToolCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct HttpToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: HttpFunction,
}

#[derive(Serialize)]
struct HttpFunction {
    name: &'static str,
    arguments: String,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

#[derive(Deserialize)]
struct StreamUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Default, Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: StreamFunction,
}

#[derive(Default, Deserialize)]
struct StreamFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn stream_chat_completion(
    address: SocketAddr,
    model: &str,
    messages: Vec<ModelMessage>,
    sampling: &SamplingSettings,
    sender: &mpsc::Sender<ModelEvent>,
    cancelled: &AtomicBool,
    cancellation_socket: &Mutex<Option<TcpStream>>,
) -> Result<(), String> {
    let messages = messages
        .iter()
        .map(|message| match message {
            ModelMessage::System(content) => HttpMessage {
                role: "system",
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
            },
            ModelMessage::User(content) => HttpMessage {
                role: "user",
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
            },
            // OpenAI-compatible HTTP calls use `assistant`; SigmaRift state calls this MODEL.
            ModelMessage::Model {
                content,
                tool_calls,
            } => HttpMessage {
                role: "assistant",
                content: (!content.is_empty()).then_some(content),
                tool_calls: (!tool_calls.is_empty()).then(|| {
                    tool_calls
                        .iter()
                        .map(|call| HttpToolCall {
                            id: &call.id,
                            kind: "function",
                            function: HttpFunction {
                                name: "execute",
                                arguments: serde_json::json!({ "command": call.command })
                                    .to_string(),
                            },
                        })
                        .collect()
                }),
                tool_call_id: None,
            },
            ModelMessage::ToolResult {
                tool_call_id,
                content,
            } => HttpMessage {
                role: "tool",
                content: Some(content),
                tool_calls: None,
                tool_call_id: Some(tool_call_id),
            },
        })
        .collect::<Vec<_>>();
    let body = chat_request_body(model, messages, sampling).to_string();
    let mut stream = TcpStream::connect_timeout(&address, MODEL_CONNECT_TIMEOUT)
        .map_err(|error| error.to_string())?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let cancellation_stream = stream.try_clone().map_err(|error| error.to_string())?;
    *cancellation_socket
        .lock()
        .map_err(|_| "model cancellation lock was poisoned")? = Some(cancellation_stream);
    if cancelled.load(Ordering::Relaxed) {
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }
    stream
        .set_write_timeout(Some(MODEL_CONNECT_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(MODEL_IDLE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(
            format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            )
            .as_bytes(),
        )
        .map_err(|error| error.to_string())?;
    read_sse_events(BufReader::new(stream), sender)
}

fn chat_request_body(
    model: &str,
    messages: Vec<HttpMessage<'_>>,
    sampling: &SamplingSettings,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": sampling.temperature,
        "max_tokens": sampling.max_tokens,
        "top_p": sampling.top_p,
        "top_k": sampling.top_k,
        "min_p": sampling.min_p,
        "repeat_penalty": sampling.repeat_penalty,
        "stream": true,
        "stream_options": { "include_usage": true },
        "tools": [{
            "type": "function",
            "function": {
                "name": "execute",
                "description": "Run one shell command on the local host. Use it when host interaction is needed.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string", "description": "The complete command line to run." } },
                    "required": ["command"],
                    "additionalProperties": false
                }
            }
        }],
    });
    let object = body.as_object_mut().expect("JSON object literal");
    if let Some(seed) = sampling.seed {
        object.insert("seed".to_owned(), seed.into());
    }
    if let Some(repeat_last_n) = sampling.repeat_last_n {
        object.insert("repeat_last_n".to_owned(), repeat_last_n.into());
    }
    if let Some(presence_penalty) = sampling.presence_penalty {
        object.insert("presence_penalty".to_owned(), presence_penalty.into());
    }
    if let Some(frequency_penalty) = sampling.frequency_penalty {
        object.insert("frequency_penalty".to_owned(), frequency_penalty.into());
    }
    if !sampling.stop.is_empty() {
        object.insert("stop".to_owned(), serde_json::json!(sampling.stop));
    }
    if let Some(grammar) = &sampling.grammar {
        object.insert("grammar".to_owned(), grammar.clone().into());
    }
    body
}

fn read_sse_events<R: BufRead>(
    mut reader: R,
    sender: &mpsc::Sender<ModelEvent>,
) -> Result<(), String> {
    let mut status = String::new();
    let status_bytes = reader
        .read_line(&mut status)
        .map_err(|error| error.to_string())?;
    if status_bytes == 0 {
        return Err("llama-server closed the connection before its HTTP status line".to_owned());
    }
    if !status.starts_with("HTTP/1.1 2") && !status.starts_with("HTTP/1.0 2") {
        return Err(format!("llama-server returned {}", status.trim_end()));
    }

    let mut chunked = false;
    loop {
        let mut header = String::new();
        let header_bytes = reader
            .read_line(&mut header)
            .map_err(|error| error.to_string())?;
        if header_bytes == 0 {
            return Err(
                "llama-server closed the connection before completing HTTP headers".to_owned(),
            );
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if header.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        }) {
            chunked = true;
        }
    }

    let body = HttpBody::new(reader, chunked);
    let mut body = BufReader::new(body);
    let mut data = String::new();
    let mut tool_calls = Vec::new();
    loop {
        let mut line = String::new();
        let read = body
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("llama-server closed the stream before [DONE]".to_owned());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if process_sse_data(&mut data, sender, &mut tool_calls)? {
                return Ok(());
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
}

fn process_sse_data(
    data: &mut String,
    sender: &mpsc::Sender<ModelEvent>,
    tool_calls: &mut Vec<PendingToolCall>,
) -> Result<bool, String> {
    if data.is_empty() {
        return Ok(false);
    }
    let payload = std::mem::take(data);
    if payload == "[DONE]" {
        send_completion(sender, std::mem::take(tool_calls))?;
        return Ok(true);
    }
    let chunk: StreamChunk = serde_json::from_str(&payload).map_err(|error| error.to_string())?;
    if let Some(usage) = chunk.usage {
        sender
            .send(ModelEvent::Usage(TokenUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            }))
            .map_err(|error| error.to_string())?;
    }
    let Some(choice) = chunk.choices.first() else {
        return Ok(false);
    };
    let content = choice.delta.content.clone().unwrap_or_default();
    let reasoning_content = choice.delta.reasoning_content.clone().unwrap_or_default();
    for call in &choice.delta.tool_calls {
        while tool_calls.len() <= call.index {
            tool_calls.push(PendingToolCall::default());
        }
        let pending = &mut tool_calls[call.index];
        pending.id.push_str(&call.id);
        pending.name.push_str(&call.function.name);
        pending.arguments.push_str(&call.function.arguments);
    }
    if !content.is_empty() || !reasoning_content.is_empty() {
        sender
            .send(ModelEvent::Delta {
                content,
                reasoning_content,
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(false)
}

fn send_completion(
    sender: &mpsc::Sender<ModelEvent>,
    pending: Vec<PendingToolCall>,
) -> Result<(), String> {
    if pending.is_empty() {
        return sender
            .send(ModelEvent::Complete)
            .map_err(|error| error.to_string());
    }

    let calls = pending
        .into_iter()
        .map(|call| {
            if call.name != "execute" {
                return Err(format!("unsupported tool: {}", call.name));
            }
            #[derive(Deserialize)]
            struct ExecuteArguments {
                command: String,
            }
            let arguments: ExecuteArguments =
                serde_json::from_str(&call.arguments).map_err(|error| error.to_string())?;
            Ok(ToolCall {
                id: call.id,
                command: arguments.command,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    sender
        .send(ModelEvent::ToolCalls(calls))
        .map_err(|error| error.to_string())
}

struct HttpBody<R> {
    reader: R,
    chunked: bool,
    remaining: usize,
    finished: bool,
}

impl<R: BufRead> HttpBody<R> {
    fn new(reader: R, chunked: bool) -> Self {
        Self {
            reader,
            chunked,
            remaining: 0,
            finished: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut size = String::new();
        self.reader.read_line(&mut size)?;
        let size = size.trim_end().split(';').next().unwrap_or_default();
        self.remaining = usize::from_str_radix(size, 16)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid HTTP chunk size"))?;
        if self.remaining == 0 {
            self.finished = true;
        }
        Ok(())
    }
}

impl<R: BufRead> Read for HttpBody<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.chunked {
            return self.reader.read(buffer);
        }
        if self.finished || buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            self.next_chunk()?;
            if self.finished {
                return Ok(0);
            }
        }
        let limit = buffer.len().min(self.remaining);
        let count = self.reader.read(&mut buffer[..limit])?;
        self.remaining -= count;
        if self.remaining == 0 {
            let mut ending = [0; 2];
            self.reader.read_exact(&mut ending)?;
            if ending != *b"\r\n" {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "invalid HTTP chunk ending",
                ));
            }
        }
        Ok(count)
    }
}

/// Startup state derived from the bundled models and the configured default.
///
/// A configured default only positions the selector cursor. It never bypasses
/// `MODEL SELECT` when multiple usable models are bundled.
#[derive(Debug, PartialEq, Eq)]
pub enum StartupModelSelection {
    NoUsableModels,
    Selected(AvailableModel),
    SelectionRequired {
        models: Vec<AvailableModel>,
        selected_index: usize,
    },
}

/// Finds regular `.gguf` files in the portable bundle's model directory.
pub fn discover_models(directory: &Path) -> io::Result<Vec<AvailableModel>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut models = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
            && let Some(model) = AvailableModel::new(&path)
        {
            models.push(model);
        }
    }

    models.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(models)
}

/// Applies the fixed startup selection policy.
pub fn select_startup_model(
    models: Vec<AvailableModel>,
    default_model: &str,
) -> StartupModelSelection {
    match models.len() {
        0 => StartupModelSelection::NoUsableModels,
        1 => StartupModelSelection::Selected(models.into_iter().next().expect("checked length")),
        _ => {
            let selected_index = models
                .iter()
                .position(|model| model.name == default_model)
                .unwrap_or(0);
            StartupModelSelection::SelectionRequired {
                models,
                selected_index,
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelParameters {
    #[serde(default = "default_model_name")]
    pub default_model: String,
    pub context_size: u32,
}

impl ModelParameters {
    pub fn validate(&self) -> Result<(), String> {
        if self.context_size == 0 {
            return Err("Context size must be greater than zero".to_owned());
        }
        Ok(())
    }
}

/// Sampling settings sent with every OpenAI-compatible chat request.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SamplingSettings {
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_min_p")]
    pub min_p: f32,
    #[serde(default = "default_repeat_penalty")]
    pub repeat_penalty: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grammar: Option<String>,
}

impl Default for SamplingSettings {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            top_p: default_top_p(),
            top_k: default_top_k(),
            min_p: default_min_p(),
            repeat_penalty: default_repeat_penalty(),
            seed: None,
            repeat_last_n: None,
            presence_penalty: None,
            frequency_penalty: None,
            stop: Vec::new(),
            grammar: None,
        }
    }
}

impl SamplingSettings {
    pub fn validate(&self) -> Result<(), String> {
        validate_finite_at_least("Temperature", self.temperature, 0.0)?;
        if self.max_tokens == 0 {
            return Err("Max tokens must be greater than zero".to_owned());
        }
        validate_finite_range("Top P", self.top_p, 0.0, 1.0)?;
        validate_finite_range("Min P", self.min_p, 0.0, 1.0)?;
        validate_finite_greater_than("Repeat penalty", self.repeat_penalty, 0.0)?;
        if let Some(value) = self.presence_penalty {
            validate_finite("Presence penalty", value)?;
        }
        if let Some(value) = self.frequency_penalty {
            validate_finite("Frequency penalty", value)?;
        }
        Ok(())
    }
}

fn validate_finite(name: &str, value: f32) -> Result<(), String> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(format!("{name} must be a finite number"))
    }
}

fn validate_finite_at_least(name: &str, value: f32, minimum: f32) -> Result<(), String> {
    validate_finite(name, value)?;
    if value >= minimum {
        Ok(())
    } else {
        Err(format!("{name} must be at least {minimum}"))
    }
}

fn validate_finite_greater_than(name: &str, value: f32, minimum: f32) -> Result<(), String> {
    validate_finite(name, value)?;
    if value > minimum {
        Ok(())
    } else {
        Err(format!("{name} must be greater than {minimum}"))
    }
}

fn validate_finite_range(name: &str, value: f32, minimum: f32, maximum: f32) -> Result<(), String> {
    validate_finite(name, value)?;
    if (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(format!("{name} must be between {minimum} and {maximum}"))
    }
}

impl Default for ModelParameters {
    fn default() -> Self {
        Self {
            default_model: default_model_name(),
            context_size: 8_192,
        }
    }
}

fn default_model_name() -> String {
    DEFAULT_MODEL_NAME.to_owned()
}

fn default_temperature() -> f32 {
    0.70
}
fn default_max_tokens() -> u32 {
    4_096
}
fn default_top_p() -> f32 {
    0.90
}
fn default_top_k() -> u32 {
    40
}
fn default_min_p() -> f32 {
    0.05
}
fn default_repeat_penalty() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn model(name: &str) -> AvailableModel {
        AvailableModel {
            name: name.to_owned(),
            path: PathBuf::from(name),
        }
    }

    #[test]
    fn bundled_llama_server_version_is_available() {
        assert_ne!(llama_server_version(), "unknown");
    }

    #[test]
    fn llama_server_version_is_parsed_from_runtime_output() {
        assert_eq!(
            parse_llama_server_version("version: 1.2.3-dev (build 4, commit abc123)\n"),
            Some("1.2.3-dev")
        );
    }

    #[test]
    fn no_models_blocks_startup() {
        assert_eq!(
            select_startup_model(Vec::new(), DEFAULT_MODEL_NAME),
            StartupModelSelection::NoUsableModels
        );
    }

    #[test]
    fn one_model_is_selected_without_a_selector() {
        let only_model = model("only.gguf");
        assert_eq!(
            select_startup_model(vec![only_model.clone()], DEFAULT_MODEL_NAME),
            StartupModelSelection::Selected(only_model)
        );
    }

    #[test]
    fn multiple_models_keep_the_default_at_the_cursor() {
        let result = select_startup_model(
            vec![model("E2B.gguf"), model(DEFAULT_MODEL_NAME)],
            DEFAULT_MODEL_NAME,
        );

        assert_eq!(
            result,
            StartupModelSelection::SelectionRequired {
                models: vec![model("E2B.gguf"), model(DEFAULT_MODEL_NAME)],
                selected_index: 1,
            }
        );
    }

    #[test]
    fn server_arguments_keep_the_runtime_local_and_disable_the_web_ui() {
        let args = server_args(&model("model.gguf"), &ModelParameters::default(), 8123);

        assert_eq!(args[0..2], ["--model", "model.gguf"]);
        assert!(args.windows(2).any(|pair| pair == ["--host", "127.0.0.1"]));
        assert!(args.windows(2).any(|pair| pair == ["--port", "8123"]));
        assert!(args.contains(&"--no-ui".to_owned()));
    }

    #[test]
    fn sampling_defaults_are_explicit_and_sent_with_chat_requests() {
        let settings = SamplingSettings::default();
        assert_eq!(settings.temperature, 0.70);
        assert_eq!(settings.max_tokens, 4_096);
        assert_eq!(settings.top_p, 0.90);
        assert_eq!(settings.top_k, 40);
        assert_eq!(settings.min_p, 0.05);
        assert_eq!(settings.repeat_penalty, 1.0);

        let body = chat_request_body(
            "model.gguf",
            vec![HttpMessage {
                role: "user",
                content: Some("Hello"),
                tool_calls: None,
                tool_call_id: None,
            }],
            &settings,
        );
        assert!((body["temperature"].as_f64().unwrap() - 0.70).abs() < 0.000_001);
        assert_eq!(body["max_tokens"], 4_096);
        assert!((body["top_p"].as_f64().unwrap() - 0.90).abs() < 0.000_001);
        assert_eq!(body["top_k"], 40);
        assert!((body["min_p"].as_f64().unwrap() - 0.05).abs() < 0.000_001);
        assert_eq!(body["repeat_penalty"], 1.0);
        assert!(body.get("seed").is_none());
        assert!(body.get("stop").is_none());
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn optional_sampling_values_are_omitted_unless_configured() {
        let settings = SamplingSettings {
            seed: Some(7),
            stop: vec!["END".to_owned()],
            ..SamplingSettings::default()
        };
        let body = chat_request_body("model.gguf", Vec::new(), &settings);

        assert_eq!(body["seed"], 7);
        assert_eq!(body["stop"], serde_json::json!(["END"]));
        assert!(body.get("repeat_last_n").is_none());
        assert!(body.get("presence_penalty").is_none());
    }

    #[test]
    fn sampling_validation_rejects_values_the_server_cannot_use() {
        let invalid_top_p = SamplingSettings {
            top_p: 1.1,
            ..SamplingSettings::default()
        };
        let invalid_temperature = SamplingSettings {
            temperature: f32::NAN,
            ..SamplingSettings::default()
        };
        let invalid_tokens = SamplingSettings {
            max_tokens: 0,
            ..SamplingSettings::default()
        };

        assert!(invalid_top_p.validate().is_err());
        assert!(invalid_temperature.validate().is_err());
        assert!(invalid_tokens.validate().is_err());
        assert!(SamplingSettings::default().validate().is_ok());
    }

    #[test]
    fn context_size_must_be_positive() {
        assert!(
            ModelParameters {
                context_size: 0,
                ..ModelParameters::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn health_response_requires_a_success_status() {
        assert!(is_successful_health_response("HTTP/1.1 200 OK\r\n\r\n{}"));
        assert!(!is_successful_health_response(
            "HTTP/1.1 503 Service Unavailable\r\n\r\n"
        ));
    }

    #[test]
    fn sse_chunk_preserves_content_and_reasoning_separately() {
        let (sender, receiver) = mpsc::channel();
        let mut data =
            r#"{"choices":[{"delta":{"content":"answer","reasoning_content":"work"}}]}"#.to_owned();

        assert!(!process_sse_data(&mut data, &sender, &mut Vec::new()).unwrap());
        assert_eq!(
            receiver.recv().unwrap(),
            ModelEvent::Delta {
                content: "answer".to_owned(),
                reasoning_content: "work".to_owned(),
            }
        );
    }

    #[test]
    fn sse_usage_is_reported_without_a_content_delta() {
        let (sender, receiver) = mpsc::channel();
        let mut data =
            r#"{"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45}}"#.to_owned();

        assert!(!process_sse_data(&mut data, &sender, &mut Vec::new()).unwrap());
        assert_eq!(
            receiver.recv().unwrap(),
            ModelEvent::Usage(TokenUsage {
                prompt_tokens: 123,
                completion_tokens: 45,
            })
        );
    }

    #[test]
    fn sse_tool_call_fragments_become_one_execute_call() {
        let (sender, receiver) = mpsc::channel();
        let mut pending = Vec::new();
        let mut first = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"execute","arguments":"{\"command\":\"printf "}}]}}]}"#.to_owned();
        let mut second = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"hello\"}"}}]}}]}"#.to_owned();
        let mut done = "[DONE]".to_owned();

        assert!(!process_sse_data(&mut first, &sender, &mut pending).unwrap());
        assert!(!process_sse_data(&mut second, &sender, &mut pending).unwrap());
        assert!(process_sse_data(&mut done, &sender, &mut pending).unwrap());
        assert_eq!(
            receiver.recv().unwrap(),
            ModelEvent::ToolCalls(vec![ToolCall {
                id: "call-1".to_owned(),
                command: "printf hello".to_owned(),
            }])
        );
    }

    #[test]
    fn chunked_http_body_removes_chunk_framing() {
        let input = Cursor::new(b"4\r\ndata\r\n4\r\n: ok\r\n0\r\n\r\n".to_vec());
        let mut body = HttpBody::new(BufReader::new(input), true);
        let mut output = String::new();

        body.read_to_string(&mut output).unwrap();

        assert_eq!(output, "data: ok");
    }

    #[test]
    fn incomplete_http_headers_report_an_error() {
        let (sender, _receiver) = mpsc::channel();
        let response =
            Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n".to_vec());

        let error = read_sse_events(BufReader::new(response), &sender).unwrap_err();

        assert!(error.contains("before completing HTTP headers"));
    }

    #[test]
    fn stream_eof_before_done_is_a_model_error_after_preserving_deltas() {
        let (sender, receiver) = mpsc::channel();
        let response = Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"
                .to_vec(),
        );

        let error = read_sse_events(BufReader::new(response), &sender).unwrap_err();

        assert!(error.contains("before [DONE]"));
        assert_eq!(
            receiver.recv().unwrap(),
            ModelEvent::Delta {
                content: "partial".to_owned(),
                reasoning_content: String::new(),
            }
        );
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn done_event_completes_a_stream() {
        let (sender, receiver) = mpsc::channel();
        let response = Cursor::new(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: [DONE]\n\n".to_vec(),
        );

        read_sse_events(BufReader::new(response), &sender).unwrap();

        assert_eq!(receiver.recv().unwrap(), ModelEvent::Complete);
    }
}
