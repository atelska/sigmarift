# SigmaRift Agent Guide

## Purpose

SigmaRift is a small Rust TUI for local LLM-driven system operations. It runs
GGUF models through the bundled `runtime/llama-server`, gives the model an
`execute` shell tool, and stores its state in ordinary local files.

Keep the project direct and inspectable. Do not add layers, dependencies,
scripts, frameworks, or abstractions without a concrete need in implemented
behavior.

## Repository Layout

- `src/` contains all SigmaRift source code.
- `runtime/llama-server` is a checked-in third-party executable. Do not add
  llama.cpp source, build logic, or runtime metadata files.
- `prompts/system.md` is the base system prompt.
- `prompts/profiles/` contains reusable prompt profiles.
- `models/` contains local GGUF files; only its `README.md` is committed.
- `screenshots/` contains current UI screenshots.
- `STYLES.md` is the UI and interaction contract.

SigmaRift creates `sessions/`, `workspace/`, `playbooks/`, and
`prompts/instructions.md` at runtime. They are user state and are not committed.
`target/` and `dist/` are generated output.

## Architecture

- `main.rs` wires application control to the terminal UI.
- `control.rs` owns sessions, runtime state, persistence, and the active turn.
- `model.rs` owns model discovery, llama-server lifecycle, and chat streaming.
- `execute.rs` runs model-requested shell commands.
- `files.rs` contains filesystem persistence helpers.
- `tui/` owns presentation state, input routing, and Ratatui rendering.

Keep domain state in `Control`; the TUI must not become a second state owner.
Keep the model loop and shell execution explicit rather than introducing an
agent framework or command bus. Follow `STYLES.md` for all UI work instead of
duplicating its rules here.

All application paths are relative to the repository or extracted bundle root.
Run development commands from that directory.

## Commands

A GGUF model must be present directly under `models/` to perform inference.

```sh
cargo run --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

## Release Distribution

- Build with `cargo build --release --locked`.
- Stage the complete distribution under `target/sigmarift/`.
- Include the SigmaRift binary, `runtime/`, `prompts/`, `models/README.md`,
  `README.md`, `LICENSE`, and `THIRD_PARTY_NOTICES.md`.
- Create empty `sessions/`, `workspace/`, and `playbooks/` directories.
- Archive the complete `sigmarift/` directory as
  `dist/sigmarift-linux-x86_64.tar.gz`.
- `target/` and `dist/` are generated and never committed.

## Change Rules

- Make the smallest change that completely solves the requested problem.
- Do not add dependencies or architectural layers without a demonstrated need.
- Do not add helper scripts, documentation trees, placeholder modules, or
  compatibility code speculatively.
- Do not add llama.cpp build or maintenance responsibilities to this repo.
- Preserve plain-file state and the current root-relative portable layout.
- Update `README.md` when public setup or behavior changes.
- Update `STYLES.md` and current screenshots when UI behavior changes.
- Run the relevant checks before considering a change complete.
