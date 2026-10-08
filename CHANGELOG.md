# Changelog

All notable changes to contextleleo are recorded here. Entries are generated from
the pull requests merged between releases, following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release is published to [crates.io](https://crates.io/crates/contextleleo),
[npm](https://www.npmjs.com/package/contextleleo), and
[GitHub Releases](https://github.com/skillsynchq/contextleleo/releases).

## Unreleased

### Added

- Add a read-only context-retrieval layer (`contextleleo::retrieval`): rank matching history across
  every stored session by keyword, file path, symbol, error, and tool signals, keep each chunk
  traceable to its `session#message`, and hand the assembled context to Jev so it fits a token
  budget. Deterministic and local; the `ContextRetriever` trait leaves room for other backends.
- Add the `context` command: retrieve the ranked history for a query, optionally optimized to a
  `--budget`. Read-only — no stored session is modified.
- Add `continue --retrieve <query>`: prepend the retrieved context to a handoff, then let `--jev`
  optimize the whole to the budget.
- Add the Freebuff harness (read and continue; `continue` opens the app).
- Add read-side session lineage (`Meta::lineage`): fork/continue/spawn relationships read from
  Codex's `session_meta` (subagent spawns, guardian review, `/fork`), pi/Campfire's
  `parentSession` (`/fork`, `/clone`), OpenCode's `session.parent_id`, and Claude Code
  teammates' `teamName`/`agentName`. Filled at discovery time, not only on load. Writing
  lineage into native formats is not implemented yet.

## [0.14.4](https://github.com/skillsynchq/contextleleo/compare/v0.14.3...v0.14.4) - 2026-09-13

### Fixed

- Sanitize replayed tool names ([#49](https://github.com/skillsynchq/contextleleo/pull/49))

## [0.14.3](https://github.com/skillsynchq/contextleleo/compare/v0.14.2...v0.14.3) - 2026-09-12

### Fixed

- Export freeform tool inputs as objects ([#44](https://github.com/skillsynchq/contextleleo/pull/44))

## [0.14.2](https://github.com/skillsynchq/contextleleo/compare/v0.14.1...v0.14.2) - 2026-09-11

### Fixed

- Unique transcript entry ids across consecutive assistants ([#41](https://github.com/skillsynchq/contextleleo/pull/41))
- Roll back failed mints with deleteAgent ([#42](https://github.com/skillsynchq/contextleleo/pull/42))

## [0.14.1](https://github.com/skillsynchq/contextleleo/compare/v0.14.0...v0.14.1) - 2026-09-11

### Fixed

- Discover all agents and title from profile name ([#40](https://github.com/skillsynchq/contextleleo/pull/40))

## [0.14.0](https://github.com/skillsynchq/contextleleo/compare/v0.13.0...v0.14.0) - 2026-09-11

### Added

- Add Grok Bot transcript harness ([#39](https://github.com/skillsynchq/contextleleo/pull/39))

### Fixed

- Map old_string/new_string to camelCase in denormalize_cursor_args ([#23](https://github.com/skillsynchq/contextleleo/pull/23))
- Preserve custom and unknown tool casing across OpenCode and Pi ([#31](https://github.com/skillsynchq/contextleleo/pull/31))

## [0.13.0](https://github.com/skillsynchq/contextleleo/compare/v0.12.1...v0.13.0) - 2026-09-04

### Added

- Add `resume` alias for `continue` command ([#16](https://github.com/skillsynchq/contextleleo/pull/16))
- Add interactive context cropping ([#21](https://github.com/skillsynchq/contextleleo/pull/21))

### Fixed

- Drop non-standard format annotations from tool schemas ([#17](https://github.com/skillsynchq/contextleleo/pull/17))
- Derive artifact search origin from message role ([#26](https://github.com/skillsynchq/contextleleo/pull/26))

## [0.12.1](https://github.com/skillsynchq/contextleleo/compare/v0.12.0...v0.12.1) - 2026-09-01

### Changed

- Search patterns match literally instead of fuzzily.
- Discovery is cheaper, and the picker's echo stays off the search path.
- Cursor Desktop discovery and loads scale with the session instead of the
  whole database.

## [0.12.0](https://github.com/skillsynchq/contextleleo/compare/v0.11.0...v0.12.0) - 2026-08-25

### Added

- ChatGPT as a live, pull-only harness: list and continue ChatGPT
  conversations in a local harness.

## [0.11.0](https://github.com/skillsynchq/contextleleo/compare/v0.10.0...v0.11.0) - 2026-08-24

### Added

- fx (Vercel's coding agent) harness.
- `contextleleo view` draws images inline on kitty-graphics terminals and ships a
  built-in pager with controls over what it shows.
- Claude Chat sessions load directly by UUID.

### Changed

- `view` output is tuned for terminals.

### Fixed

- Terminal query helpers are gated to unix.

## [0.10.0](https://github.com/skillsynchq/contextleleo/compare/v0.9.1...v0.10.0) - 2026-08-21

### Added

- Claude Chat as a live, pull-only harness, off by default and reachable only
  through an explicit `--from`.
- `contextleleo export` for moving sessions between machines.
- `contextleleo init` installs the ctrl+shift+r session picker into your shell.

### Changed

- Launched harnesses receive the real tty.

### Fixed

- The wasm bundle is built from the contextleleo package.

## [0.9.1](https://github.com/skillsynchq/contextleleo/compare/v0.9.0...v0.9.1) - 2026-08-20

### Added

- The CLI is also a library (`contextleleo-cli`), exposing its commands as clap
  types and a `run_session` entry point.
- A persistent search cache for `query`.

## [0.9.0](https://github.com/skillsynchq/contextleleo/compare/v0.8.1...v0.9.0) - 2026-08-20

### Added

- Cowork, Claude desktop's local agent mode, as a harness.

## [0.8.1](https://github.com/skillsynchq/contextleleo/compare/v0.8.0...v0.8.1) - 2026-08-19

### Fixed

- Foreign `tool_result` blocks are flattened when writing claude_code.

### Changed

- README translations moved under `docs/translations/`.

## [0.8.0](https://github.com/skillsynchq/contextleleo/compare/v0.7.0...v0.8.0) - 2026-08-19

### Added

- Simple, an interchange pseudo-harness for agents without a native store.
- `continue` accepts a Simple document from a file or stdin.

## [0.7.0](https://github.com/skillsynchq/contextleleo/compare/v0.6.0...v0.7.0) - 2026-08-18

### Added

- README translations in twelve languages.

### Changed

- The harness list is a capability matrix.
- opencode import satisfies the stricter session and message schema.
- CLI quality gaps found against replay-cli are closed.

## [0.6.0](https://github.com/skillsynchq/contextleleo/compare/v0.5.0...v0.6.0) - 2026-08-17

### Added

- Cursor desktop harness for the IDE app's `state.vscdb` sessions.
- Documentation of every harness's on-disk transcript format.

### Changed

- Stores and the CLI are hardened against hostile session files.
- The Claude Code summary line is anchored to a real leaf.
- npm publishing uses trusted publishing and is triggered by version tags
  again.

## [0.5.0](https://github.com/skillsynchq/contextleleo/compare/v0.4.3...v0.5.0) - 2026-08-08

### Added

- Claude Code local commands are modelled as `Tool::Command`.
- Experimental `--move` for `contextleleo continue`.

## [0.4.3](https://github.com/skillsynchq/contextleleo/compare/v0.4.2...v0.4.3) - 2026-08-03

### Added

- `Session::updated_at`.

## [0.4.2](https://github.com/skillsynchq/contextleleo/compare/v0.4.1...v0.4.2) - 2026-07-30

### Fixed

- Harness session stores resolve correctly on Windows.

## [0.4.1](https://github.com/skillsynchq/contextleleo/compare/v0.4.0...v0.4.1) - 2026-07-20

### Added

- `contextleleo view`, and `#range` refs for `view` and `continue`.

### Fixed

- Exported codex rollouts name a real `model_provider`.
- Non-`ses` session ids are re-shaped for opencode export.

## [0.4.0](https://github.com/skillsynchq/contextleleo/compare/v0.3.0...v0.4.0) - 2026-07-17

### Added

- amp and antigravity harnesses.
- An MCP server with a token-conscious text projection.
- `Span` for pointing into sessions with zero-copy fragment resolution.
- `completion` subcommand; harness names are advertised to completers.
- `--cwd` scopes `list` and `query` to a folder.

### Changed

- `query` indexes in parallel and interactive navigation is responsive.

## [0.3.0](https://github.com/skillsynchq/contextleleo/compare/v0.2.0...v0.3.0) - 2026-07-06

### Added

- Grok CLI harness.
- `contextleleo::search`: fuzzy and substring search with a hot index.
- `contextleleo::local` and the `query` command with an fzf-style picker.
- `Store::delete` on every harness store.

### Changed

- The CLI is a separate workspace crate on clap.
- Literal occurrences rank above every gapped fuzzy alignment.
- MSRV tracks the latest stable Rust.

## [0.2.0](https://github.com/skillsynchq/contextleleo/compare/v0.1.0...v0.2.0) - 2026-07-01

### Changed

- The public API is hierarchical.

## [0.1.0](https://github.com/skillsynchq/contextleleo/releases/tag/v0.1.0) - 2026-07-01

### Added

- Canonical session model and core traits.
- claude_code, codex, pi, campfire, opencode, and Cursor harnesses.
- `contextleleo` CLI with `list` and cross-harness `continue`.
- Composable `TextCodec` layer and WASM bindings.
