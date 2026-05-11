# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ipmsg-rs is a Rust reimplementation of IP Messenger, a LAN messaging and file transfer application. It implements the IPMSG protocol for UDP-based peer discovery/messaging and TCP-based file transfer. Currently uses GTK4 + libadwaita for the GUI.

## Build & Run

```bash
cargo build
cargo run
```

There are no tests, no build script, and no CI/CD pipeline.

## Architecture

### Event-Driven Design with Global Channel

The app uses a **global crossbeam MPMC channel** (`src/core/mod.rs`) as the central event bus:

- `GLOBLE_SENDER` / `GLOBLE_RECEIVER` — all components send `ModelEvent` through this channel
- `model_event_loop()` in `src/events/model.rs` receives and dispatches events
- UI updates flow back through a separate `async_channel` carrying `UiEvent` values

### Threading Model (no async runtime)

Multiple OS threads are spawned directly — no tokio/async-std:
- UDP listener thread (peer discovery, message receive)
- Model event loop thread (central dispatcher)
- TCP file server thread (file sending)
- Per-download threads

### Key Source Layout

- `src/constants/protocol.rs` — IPMSG protocol constants, command parsing helpers (`get_mode()`, `get_opt()`), lazy statics for `HOST_NAME`, `LOCAL_IP`, `ADDR`
- `src/models/event.rs` — `ModelEvent` (17 variants) and `UiEvent` (9 variants) enums
- `src/models/model.rs` — `Packet`, `User`, `ShareInfo`, `FileInfo` data structures; `PacketBuilder` (partially implemented, `finish()` commented out)
- `src/events/model.rs` — Central coordination: `model_run()`, `start_daemon()`, `model_event_loop()`, `model_packet_dispatcher()`
- `src/core/fileserver.rs` — TCP file server for outgoing files
- `src/core/download.rs` — TCP download manager (`ManagerPool`)
- `src/util.rs` — `utf8_to_gb18030()` encoding, `packet_parser()` (combine parser combinator)
- `src/ui/main_win.rs` — Main window with user list (`TreeView`)
- `src/ui/chat_window.rs` + `chat_window.ui` — Per-peer chat window (GTK4 XML + Rust)

### Network Protocol

- **UDP** on port 2425 (`IPMSG_DEFAULT_PORT`): broadcast entry/exit, direct messaging, peer discovery
- **TCP** on port 2425: file transfer (regular files and recursive directory transfers)
- All network communication uses **GB18030 encoding** for compatibility with classic IP Messenger (hardcoded, TODO: configurable)

### UI Language

All user-facing strings are in Chinese (window titles, buttons, labels, log messages).
main Colors:#518463