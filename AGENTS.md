# AGENTS.md

Guidance for AI coding agents working on depee, a Rust TUI (ratatui +
crossterm + tokio) for inspecting and updating Node.js dependencies. Read
`README.md` for user-facing behavior; this file covers how the code is organized
and the rules that keep it that way.

## Commands

```sh
cargo fmt
cargo test
cargo clippy --all-targets   # keep at zero warnings
```

CI and releases use the pinned toolchain `1.99.0` with `--locked`. Do not add
dependencies casually; `cargo audit` runs on every release.

## Architecture

The code follows the usual ratatui split: **state** (`app`), **rendering**
(`ui`), and **input** (`event`), on top of domain modules that know nothing
about the TUI.

```
src/
├── main.rs          Bootstrap, terminal init/restore, the draw/select loop
├── cli.rs           clap arguments
├── event.rs         Blocking crossterm reader → async channel
├── app/             Application state and every state transition
│   ├── mod.rs       App, ProjectInfo, Mode, Confirm, Sort, MetadataStatus,
│   │                UpdatePrompt; queries (visible, target, update_request…)
│   ├── input.rs     Key handling, one handler per mode; nav_delta / step
│   ├── message.rs   Msg enum and App::handle_message (background results)
│   └── tasks.rs     Spawning background work (scan, metadata, update, probes)
├── ui/              Rendering only
│   ├── mod.rs       Layout, header, footer hints
│   ├── list.rs      Package table
│   ├── details.rs   Details pane and version picker
│   ├── confirm.rs   Update confirmation and compatibility inspection
│   ├── update_prompt.rs  Startup self-update screen
│   └── text.rs      DetailLines builder, wrapping, formatting (ago, count…)
├── package.rs       Package, Release, Metadata, Change, Kind
├── project.rs       package.json (Manifest) and node_modules lookups
├── pm/              Package managers: detection, `outdated` parsing, update commands
├── registry.rs      npm Registry fetch + 12h disk cache; pure JSON → Metadata parser
├── compat.rs        npm range matching and dependency diffs
├── cache.rs         Cache directory and --clear-cache
└── self_update.rs   GitHub release check, skip/remind, installer
```

### Dependency direction

`main` → `app` / `ui` / `event` → domain modules (`package`, `project`, `pm`,
`registry`, `compat`, `cache`, `self_update`).

- Domain modules must not import from `app` or `ui`.
- `ui` reads `&App` and never mutates it. The only exceptions are draw-time
  caches behind interior mutability: `Confirm::scroll_max` (`Cell`, recorded
  by the confirmation draw so scrolling can be bounded) and
  `App::installed_versions` (`RefCell`, cleared when a scan finishes).
- `app` holds all state changes. Rendering decisions do not belong there.

### State and modes

- The main screen is in exactly one `Mode`: `Browse`, `Search(query)`,
  `VersionPicker`, or `Confirm(Confirm)`. Do not add parallel boolean flags.
  State that only exists in one mode lives in the variant (for example
  `Confirm::pinned`, the version chosen in the picker).
- `update_prompt` is a separate full-screen gate shown before the main screen.
- `target_latest` and `upgrade` are user preferences that persist across
  confirmations. A pinned version or Latest forces an upgrade through
  `App::upgrading()` without changing the `upgrade` toggle.
- `App::update_request()` is the single source of truth for what an update
  runs. The confirmation screen and `start_update` both use it.
- `selected` is an index into `packages`, not into the visible list.
  `visible()` returns package indexes in display order.

### Adding a mode or screen

1. Add the variant to `Mode` (with its own state, if any).
2. Add a `handle_*_key` method in `app/input.rs` and dispatch to it from
   `handle_key`.
3. Add its hints in `footer_hints` (`ui/mod.rs`) and its drawing in
   `draw_content` or a new `ui/*.rs` file.
4. Update the key table in `README.md`.

### Background work

1. Spawn the task in `app/tasks.rs`, capturing clones of what it needs (never `&App`).
2. Send the result as a `Msg` variant, with errors as `String` (`format!("{e:#}")`).
3. Apply it in `app/message.rs`.

Tasks never touch `App` directly. Registry requests are limited by a semaphore
(`METADATA_CONCURRENCY`). Failures must stay visible as failures (for example
`MetadataStatus::Failed`) instead of looking like "no data".

## UI conventions

- Footer hints write navigation as `↑/↓ or j/k …`. Ctrl+p/Ctrl+n work
  everywhere through `nav_delta` but are not advertised. The exception is
  Search, where letters are typed into the query, so its hint mentions
  Ctrl+n/p.
- Ctrl+c quits from every screen. In the confirmation, Ctrl chords must not
  trigger letter shortcuts (Ctrl+n scrolls; it is not `n`).
- Compatibility states never rely on color alone. Each line starts with a mark:
  `✓` compatible, `!` needs review, `✗` incompatible, `?` unknown,
  `-` not specified.
- Build detail panes with `DetailLines` (`section`, `field`, `value`, `blank`)
  instead of pushing raw `Line`s. Measure and pad text by display width
  (`Span::width`, `text::pad`), not by byte length.
- Missing values render as `N/A` (`text::or_na`, `ago`, `count`).
- Minimum terminal size is 50×12. At 120 columns and wider, the list and
  details sit side by side.

## Testing

- Tests live next to the code in `#[cfg(test)] mod tests`.
- Shared fixtures: `package::fixtures::{package, release}` and
  `app::fixtures::app()`. The fixture app keeps its message receiver
  unconnected, so spawned tasks are harmless.
- Use `tempfile::tempdir()` for filesystem tests. Never touch the real cache
  or config.
- Use `#[tokio::test]` when the code under test spawns tasks (for example
  handling `Msg::Scan`, which requests metadata).
- Test rendering with `ratatui::backend::TestBackend` (see `ui/mod.rs` tests).
- Keep parsing and decision logic in pure functions (`parse_outdated`,
  `registry::parse_document`, `self_update::check_for_updates`) and test them
  directly. Keep network and process I/O thin.

### Refactoring safely

For behavior-preserving refactors of the UI, first capture snapshots: render
representative states at several sizes with `TestBackend`, write each buffer
(`format!("{:?}", buffer)` includes styles) to a scratch directory, then repeat
after the change and `diff -r`. Remove the temporary harness afterwards. The
2026-10 module split was verified this way with no differences.

## Domain notes and pitfalls

- **Package managers:** each `outdated` output differs. Yarn Classic emits
  NDJSON with a `table` row, and pnpm's `wanted` is the lockfile resolution, so
  `scan` runs `pnpm outdated --compatible` separately. `outdated` exits with
  code 1 when something is outdated; that is not an error.
- **Compatible updates:** Yarn Berry's bare `yarn up <name>` jumps to latest,
  so in-range updates pin the existing range (`yarn up name@<range>`). Classic
  uses `yarn upgrade`. `pnpm update` and `bun update` rewrite the manifest
  lower bound (`compatible_update_rewrites_range`).
- **npm ranges:** `compat::requirement_matches` translates npm syntax to
  `semver::VersionReq`. A bare version is an exact match in npm. Return `None`
  (unknown) rather than guessing when any part cannot be parsed.
- **Commands:** run them as argument arrays (`tokio::process::Command`), never
  through a shell. The one exception, the self-update installer, passes its URL
  as `$1` so it is never parsed as shell code.
- **Self-update:** the installer URL is pinned to the release tag
  (`v{version}/install.sh`, protected by a ruleset). Do not point it at a
  branch. Only `v`-prefixed semver tags count as releases.
- **install.sh and docs:** use current GitHub CLI flags directly (for example
  `gh attestation verify --hostname`). Do not add compatibility guards for old
  `gh` versions.
- **Known quirk:** for pnpm, the `{"packages": [...]}` fallback in
  `parse_outdated` is unreachable because objects are parsed as name-keyed
  maps first. It has been left as is.

## Workflow

- Commit messages are short imperative subjects (for example "Add startup update
  prompt"). Features go through pull requests on `feat/*` branches.
- Releases bump `version` in `Cargo.toml` (and `Cargo.lock`) in a
  "Release vX.Y.Z" commit, then push a `vX.Y.Z` tag to trigger
  `.github/workflows/release.yml`.
- Keep `README.md` (keys, behavior, limitations) in sync with user-visible
  changes.
