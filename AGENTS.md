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
│   ├── mod.rs       App, ProjectInfo, Workspace, Mode, Confirm, WorkspacePicker,
│   │                Sort, MetadataStatus, UpdatePrompt; queries (visible,
│   │                target, update_request…); switch_workspace
│   ├── input.rs     Key handling, one handler per mode; nav_delta / step
│   ├── message.rs   Msg enum and App::handle_message (background results)
│   └── tasks.rs     Spawning background work (scan, metadata, update, probes)
├── ui/              Rendering only
│   ├── mod.rs       Layout, header, footer hints
│   ├── list.rs      Package table
│   ├── details.rs   Details pane and version picker
│   ├── confirm.rs   Update confirmation and compatibility inspection
│   ├── update_prompt.rs  Startup self-update screen
│   ├── workspace.rs Workspace member picker
│   └── text.rs      DetailLines builder, wrapping, formatting (ago, count…)
├── package.rs       Package, Release, Metadata, Change, Kind
├── project.rs       package.json (Manifest), node_modules lookups, workspace root
├── workspace.rs     Workspace members: pnpm-workspace.yaml parsing, globs, walk
├── pm/              Package managers: detection, `outdated` parsing, update commands
├── registry.rs      npm Registry fetch + 12h disk cache; pure JSON → Metadata parser
├── compat.rs        npm range matching and dependency diffs
├── cache.rs         Cache directory and --clear-cache
└── self_update.rs   GitHub release check, skip/remind, installer
```

### Dependency direction

`main` → `app` / `ui` / `event` → domain modules (`package`, `project`,
`workspace`, `pm`, `registry`, `compat`, `cache`, `self_update`).

- Domain modules must not import from `app` or `ui`.
- `ui` reads `&App` and never mutates it. The only exceptions are draw-time
  caches behind interior mutability: `Confirm::scroll_max` (`Cell`, recorded
  by the confirmation draw so scrolling can be bounded) and
  `App::installed_versions` (`RefCell`, cleared when a scan finishes).
- `app` holds all state changes. Rendering decisions do not belong there.

### State and modes

- The main screen is in exactly one `Mode`: `Browse`, `Search(query)`,
  `VersionPicker`, `Confirm(Confirm)`, or `WorkspacePicker(WorkspacePicker)`.
  Do not add parallel boolean flags.
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

Tasks never touch `App` directly. Project-scoped messages (`Scan`, `Metadata`,
`PmVersion`, `Update`) carry the `App::epoch` they were spawned in;
`switch_workspace` bumps it, and `handle_message` drops older results so they
never land on another member. Registry requests are limited by a semaphore
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
- **Yarn Berry outdated:** Berry has no `outdated` command; it prints a
  colored `Usage Error: Couldn't find a script named "outdated"` to stdout and
  exits 1. `pm::yarn_output::parse_ndjson` turns that (and any failing Yarn
  output with no JSON line) into an error with ANSI codes stripped, so it never
  shows as an empty list. Detection is from the output, not
  `ProjectInfo::yarn_berry`: the flavor is a guess, and a Berry project may
  provide `outdated` through a plugin or script.
- **Workspaces:** `project::find_workspace_root` walks up from `cwd` (stopping
  after the first `.git`) to the nearest `pnpm-workspace.yaml` or `workspaces`
  declaration. `ProjectInfo::load` detects the package manager there (the
  member's `packageManager`, else the root's, then the root's lockfiles and
  `.yarnrc.yml`), but `ProjectInfo::cwd` stays the member: `outdated` and updates run there.
  `workspace::members` lists the root and its members once at load; switching
  (`ProjectInfo::switch_to`) reuses that list and `load_in`, so detection rules
  stay in one place and `--pm` is kept. The walk must stay read-only and below
  the root: no symlinks, no `..`/absolute patterns, bounded depth and entries.
  `pnpm-workspace.yaml` is read by a line-based parser for the `packages:`
  key only; there is no YAML dependency. Switching is refused while updating,
  and `start_update` refuses while a scan runs.
- **Workspace rows:** npm and Yarn Classic `outdated` list every member's
  dependencies even when run in one member. npm's `dependent` is the member's
  directory name (and a package used by several members becomes an array of
  objects); Yarn Classic's `Workspace` column is the package name. `scan`
  keeps the scanned project's rows via `own_entries` (rows without a dependent
  are kept; duplicates prefer the installed `current`). `npm outdated
  --workspace` does not filter this. pnpm already scopes to the member.
- **Fixtures:** `fixtures/workspaces` holds small npm, pnpm, Yarn Classic,
  Yarn Berry and Bun monorepos for manual checks against real package managers
  (see its README). Install with scripts disabled and run
  `check-release-age.sh` afterwards; never commit `node_modules`.
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
