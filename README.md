<div align="center">

# cursor-byok

A local backend for Cursor: bring your own model APIs, keep Agent tools, Skills, and MCP.

[中文说明](./README-CN.md) · [Download](https://github.com/zkldwxz/cursor-byok/releases/latest) · [Telegram](https://t.me/cursor_byok)

[![Release](https://img.shields.io/github/v/release/zkldwxz/cursor-byok?style=flat-square)](https://github.com/zkldwxz/cursor-byok/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/zkldwxz/cursor-byok/total?style=flat-square)](https://github.com/zkldwxz/cursor-byok/releases)
[![License](https://img.shields.io/github/license/zkldwxz/cursor-byok?style=flat-square)](./LICENSE)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square)](https://github.com/zkldwxz/cursor-byok/releases/latest)

</div>

![Connect cursor-byok to a wide range of model APIs](./images/en-brand-1.png)

![cursor-byok dashboard](./images/en-home-1.png)

## What this project is

**cursor-byok** runs on your machine as Cursor's local backend. Cursor talks to this service instead of (or alongside) the official cloud; the service forwards model calls to the OpenAI- or Anthropic-compatible APIs you configure.

API keys and settings stay on disk. Model traffic still goes to the provider you chose. This project is independent and is not affiliated with Cursor.

This repository, [zkldwxz/cursor-byok](https://github.com/zkldwxz/cursor-byok), is where the app is maintained and released. See [Origin](#origin) for the archived upstream it continues.

## Features

- **Your model channels:** endpoint, credentials, model ID, headers, and extra parameters per model.
- **Protocols:** OpenAI Responses, OpenAI Chat Completions, Anthropic Messages.
- **Built-in provider plugins:** including ZCode (Z.AI / Zhipu Coding Plan) with the official client identity.
- **Agent loop:** tool calling, Skills, MCP, subagents, and multi-turn history.
- **Local store:** SQLite checkpoints, startup pruning of dead lineages, and a one-file device migration export.
- **Operations UI:** model editor, connection tests, token/cache metrics, plugins, and system settings.
- **Desktop app:** macOS, Windows, and Linux, with in-app updates from this repository's GitHub Releases.

## Quick start

1. Download the latest build for your OS from [GitHub Releases](https://github.com/zkldwxz/cursor-byok/releases/latest).
2. Open cursor-byok → **Cursor**, and initialize the local CA when prompted.
3. Open **Model Settings**, add a model (server URL, API key, model ID), save, and run **Test**.
4. Keep cursor-byok running.
5. After the first install, or after upgrading Cursor, **quit Cursor completely**, start it again, open a **new** chat, and pick the model you configured — not **Auto**.

Existing chats opened before the first successful connection will not pick up the local backend.

## Model settings

Each saved model is one upstream channel.

![cursor-byok model settings](./images/en-model-1.png)

| Family | Type | Protocol |
| --- | --- | --- |
| Claude | **Anthropic** | Messages API |
| GPT / OpenAI | **OpenAI** | **Responses API** (preferred; Chat Completions often drops prompt cache) |
| Everything else | **OpenAI** | **Chat Completions API** |

Useful fields: type, protocol, server URL (base or full), API key, upstream model ID, display name shown in Cursor, context / max-output tokens, reasoning effort, JSON headers, and extra JSON body fields. Only send fields the provider documents.

ZCode models can be imported as a plugin credential, or created as a manual OpenAI-compatible model with the “spoof ZCode environment” switch once the plugin is installed.

## Tab completion

Cursor Tab is a separate service; it does not go through your chat model channel. In **System Settings → TAB**:

- **Public service (default):** the shared Tab endpoint, no extra setup.
- **Direct:** the official Tab service for the Cursor account currently signed in.
- **Custom:** a self-hosted Tab server. The last published tree is on the archived upstream: [`cursor-tab-server`](https://github.com/leookun/cursor-byok/tree/archive/v0.0.49/cursor-tab-server).

Restart Cursor and open a new chat after changing Tab mode.

## Official Cursor account

cursor-byok can sit next to a real Cursor login:

- Sign in with your own Cursor account. If an older fake account is still logged in, sign it out first.
- Official models and local models can be mixed when the account has official quota.
- **Auto uses official models only.** With no official quota, pick a local model yourself.
- Indexing, plugins, and other Cursor IDE features keep working.

## Data flow

```text
Cursor desktop
    │  Agent requests, tool results, KV blobs
    ▼
cursor-byok (this app, on your machine)
    │  OpenAI / Anthropic compatible HTTP
    ▼
The model API you configured
```

Keys never leave the local SQLite store except as request headers to that API.

## Project layout

```text
cursor-byok/
├── apps/desktop/          Desktop UI (React) and Tauri shell
├── server/                Local Cursor backend, store, plugins, control API
├── crates/semble-core/    Local code index / search
├── protocols/cursor/      Captured Cursor protobuf (single source)
├── support/               Protocol extractor, capture tools, benchmarks
├── .agents/skills/        Area rules for contributors
└── .github/workflows/     CI and desktop GitHub Releases
```

## Development

Need Rust, Node.js / npm, and Tauri 2 system libraries.

```bash
npm --prefix apps/desktop install
make dev-web          # server + Vite
make dev-desktop      # Tauri
make check            # fmt, clippy, tests, frontend check — same as CI
```

See [AGENTS.md](./AGENTS.md) and [`.agents/skills/`](.agents/skills/) before changing an area.

## Origin

cursor-byok was created by [leookun](https://github.com/leookun) in [leookun/cursor-byok](https://github.com/leookun/cursor-byok). That repository was **archived in October 2026**.

**This repository is the continuation:** bug fixes, ZCode, store/pruning work, updater signing, and GitHub Releases are published from [zkldwxz/cursor-byok](https://github.com/zkldwxz/cursor-byok). Installers and `latest.json` / `update.json` point here.

The original MIT-licensed code remains the foundation. Credit belongs with the original author and everyone who contributed upstream; this repo adds maintenance on top of that tree.

## Community

- [Telegram](https://t.me/cursor_byok)
- QQ: `1095916242`, `1094411438`, `1095918002`, `1094419321`

When reporting a problem, include OS, cursor-byok version, model type, protocol, a redacted endpoint, the error, and reproduction steps. Do not paste API keys.

## License

[MIT License](./LICENSE). Copyright (c) 2026 leookun, zkldwxz.

Original contributors:

<a href="https://github.com/leookun/cursor-byok/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=leookun/cursor-byok" alt="contributors to leookun/cursor-byok" />
</a>
