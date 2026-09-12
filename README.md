# SigmaRift

**Local LLM-driven system operations in the terminal.**

SigmaRift is a portable Rust application that gives a local GGUF model a
keyboard-driven workspace and direct shell execution. The model, runtime,
prompts, sessions, and work files remain local and inspectable.

![SigmaRift terminal UI](https://raw.githubusercontent.com/atelska/sigmarift/main/screenshots/sigmarift-main.png)

> [!CAUTION]
> SigmaRift lets the selected model execute shell commands automatically with
> the privileges of the user running it. It does not sandbox commands or ask
> for per-command confirmation. Run it only where you accept that risk.

## Download and Run

The ready-to-run release targets x86_64 Linux and includes SigmaRift plus its
CPU-only `llama-server` runtime. Model weights are not included.

```sh
curl -fLO https://github.com/atelska/sigmarift/releases/latest/download/sigmarift-linux-x86_64.tar.gz
tar -xzf sigmarift-linux-x86_64.tar.gz
cd sigmarift
```

Download the recommended model:

```sh
curl -fL --output models/gemma-4-E2B-it-Q4_K_M.gguf \
  'https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_K_M.gguf?download=true'
```

Then start SigmaRift from the extracted directory:

```sh
./sigmarift
```

If one GGUF model is present, it is selected automatically. If several are
present, SigmaRift shows a model selector at startup. See
[`models/README.md`](models/README.md) for other supported model options and
their terms.

## What It Does

- Runs local GGUF models through the bundled `llama-server`.
- Keeps multiple persistent work sessions without a database.
- Presents model responses, reasoning, commands, and results in one TUI.
- Lets the model execute shell commands and consume their output.
- Supports reusable prompt profiles from `prompts/profiles/`.
- Provides editable runtime parameters and additional instructions.
- Works directly in a local or SSH terminal without a browser or service.

`Ctrl+Space` opens the Control interface. `Ctrl+C` interrupts an active model
request or command. `Esc` closes the current overlay or exits from the main
interface.

## How It Works

SigmaRift starts `runtime/llama-server` on an unused loopback port and uses its
OpenAI-compatible chat completion API. During a turn, the model can request a
shell command, inspect the result, and continue until it returns a final
response:

```text
model -> command -> result -> model
```

Sessions preserve conversation history, reasoning, tool activity, selected
profiles, and token usage. SigmaRift creates `sessions/`, `workspace/`, and
`playbooks/` when needed.

## Security

- Model requests stay on the local loopback interface.
- Model-requested commands run automatically.
- Commands inherit SigmaRift's filesystem, process, and network access.
- There is no command sandbox or per-command approval step.
- Command stdin is unavailable, so interactive password prompts cannot work.
- Timeouts and output limits are operational safeguards, not security
  boundaries.

## Develop from Source

Requirements are a recent stable Rust toolchain and x86_64 Linux.

```sh
git clone https://github.com/atelska/sigmarift.git
cd sigmarift
```

Add a GGUF model to `models/`, then run:

```sh
cargo run --locked
```

Development checks:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Run SigmaRift from the repository or extracted bundle root. Its runtime,
models, prompts, sessions, and workspace use paths relative to that directory.

## Repository

```text
src/                  SigmaRift source code
runtime/llama-server  Bundled executable model runtime
prompts/              Base prompt and reusable profiles
models/                Local GGUF files and model guidance
screenshots/           Current terminal UI screenshots
STYLES.md              UI and interaction rules
AGENTS.md              Repository guidance for coding agents
```

Models, build output, distributions, sessions, playbooks, and workspace files
are intentionally not committed. The bundled runtime is a third-party
executable; SigmaRift does not build or maintain llama.cpp.

The project was developed with AI-assisted engineering through
[OpenCode](https://opencode.ai/) and OpenAI models.

## License

SigmaRift is released under the [MIT License](LICENSE). See
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for bundled third-party
software and [`models/README.md`](models/README.md) for model-specific terms.
