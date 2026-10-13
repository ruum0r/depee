# depee

`depee` is a Rust terminal dashboard for inspecting outdated dependencies in a
Node.js project. It reads `package.json`, detects npm, pnpm, Yarn, or Bun, and
uses that package manager's CLI to find outdated packages.

## Run

```sh
cargo run --release
cargo run --release -- --cwd ./my-project
cargo run --release -- --pm pnpm
cargo run --release -- --workspace web
```

The `--pm` option accepts `npm`, `pnpm`, `yarn`, or `bun`. It overrides
`packageManager` in `package.json` and lockfile detection. The selected package
manager must be installed and available on `PATH`.

Inside a workspace (monorepo) member such as `packages/web`, depee looks upward
for the workspace root: the nearest directory with `pnpm-workspace.yaml` or a
`package.json` that declares `workspaces`. The search stops at the repository
root (the first directory containing `.git`). The package manager is then
detected from the root's lockfiles and `.yarnrc.yml`, and from the
`packageManager` field of the member or, failing that, the root. Scans and
updates still run in the member directory, so they cover only the member's
dependencies. The header shows the member path, for example
`web (workspace: packages/web)`, or `(workspace: root)` at the root.

Members are listed from the root's `package.json` `workspaces` (an array or
`{ "packages": [...] }`) and the `packages:` list of `pnpm-workspace.yaml`.
Patterns may use `*` within a directory name, `**` for any number of
directories, and a leading `!` to exclude matches. A member is a directory with
a `package.json`; it is shown by its relative path and package name. Patterns
with `..` or absolute paths are ignored, and the search stays below the root:
it skips `node_modules`, dot-directories, and symlinks, and stops after eight
directory levels.

Started at a workspace root that has members, depee first opens the member
picker; Esc there inspects the root itself. Press `w` to switch to another
member later; packages are then rescanned in the chosen member. Pass
`--workspace <name|path>` (a package name or a path relative to the root, `.`
for the root) to start in a member directly. Switching is unavailable while an
update runs, and updates wait until the member's scan has finished.

## Install (macOS and Linux)

Release binaries are built for macOS (Apple Silicon and Intel) and Linux
(x86_64 and ARM64). Once a release has been published, Rust and Cargo are not
required to install or run its binary. Install the latest release with:

```sh
curl -fsSL https://raw.githubusercontent.com/ruum0r/depee/main/install.sh | sh
```

The installer puts `depee` in `~/.local/bin`. Set `DEPEE_INSTALL_DIR` to choose a
different directory. Make sure the install directory is on your `PATH`. Set
`DEPEE_VERSION` (for example `0.3.0`) to install a specific release instead of
the latest one.

When a newer version is available, `depee` opens with an update screen. Choose
**Update now** to run that release's installer (fetched from its version tag)
into the directory of the running `depee` binary, **Skip this version** to stop being asked about that release, or
**Remind me later** (Esc) to continue to the dependency list.

`depee` checks GitHub Releases in the background at most once every 24 hours.
A newly found release is offered on the next launch, and a failed check does
not block the app. Update checks are enabled by
default. Disable them in `~/.config/depee/config.toml`:

```toml
check_for_updates = false
```

The same config path is used on macOS and Linux. The check only requests the
latest release metadata from GitHub; it does not send project information.

The binary still needs npm, pnpm, Yarn, or Bun on `PATH` to inspect and update a
project. Pushing a version tag such as `v0.1.0` builds and publishes the release
archives used by the installer. GitHub automatically generates release notes
from merged pull requests, grouped by labels configured in
`.github/release.yml`. Add the `skip-changelog` label to exclude a pull request.

The installer checks the archive against the release's `SHA256SUMS`. This
detects transfer errors; the checksum alone does not authenticate the publisher.
The release workflow also attaches signed build provenance. If GitHub CLI is
installed and authenticated, the installer verifies that provenance. You can
also verify it manually after downloading an archive:

```sh
gh attestation verify depee-aarch64-apple-darwin.tar.gz --repo ruum0r/depee \
  --hostname github.com \
  --cert-identity-regex '^https://github\.com/ruum0r/depee/\.github/workflows/release\.yml@refs/tags/v'
```

Replace the archive name with the one for your platform.

## Keys

| Key | Action |
| --- | --- |
| `↑` / `↓`, `j` / `k` | Select a package |
| `Ctrl+n` / `Ctrl+p` | Next / previous package (and release) |
| `/` | Filter packages by name |
| `s` | Cycle sort field (name, change type, publication date) |
| `S` | Reverse sort direction |
| `v` | Pick a published version |
| `u` | Review and confirm an update |
| `l` in confirmation | Switch Wanted / Latest target |
| `m` in confirmation | Switch Compatible Update / Upgrade |
| `c` | Open repository releases or package homepage |
| `r` | Refresh package data |
| `e` | Show the last update error |
| `w` | Choose a workspace member (inside a workspace) |
| `Enter` / `Esc` in member picker | Switch to the member / cancel (at startup: inspect the root) |
| `?` | Show the key summary |
| `q`, `Ctrl+c` | Quit |

The package table shows the current version, newest stable patch and minor
versions, newest stable release, and the publication age of the latest tagged
release. Registry lookups run in the background with at most four requests at
once.

Registry details, including weekly download counts, are loaded asynchronously
and cached for 12 hours under the
platform cache directory (`~/.cache/depee` on macOS). Network failures leave the
package list usable; a failed download count shows N/A and is retried next time.
Run `depee --clear-cache` to remove registry metadata and update-check cache;
they will be fetched again as needed.
Updates are run as argument arrays through the detected package manager and
only after confirmation.

## Current limitations

- `outdated` output formats vary between package manager releases. npm and
  Bun's JSON output and the common pnpm/Yarn JSON forms are supported; unusual
  CLI output may need parser adjustments.
- Yarn Berry (2+) has no built-in `outdated` command, so depee cannot list
  outdated packages for Yarn 2+ projects yet and reports an error instead.
  Updates through `yarn up` are still supported.
- Yarn Plug'n'Play is not managed as a separate project model.
- Workspaces are inspected one member at a time (switch with `w` or
  `--workspace`); there is no combined view of all members.
- Member patterns support `*`, `**`, and `!` exclusions only; brace and
  character-class globs are not expanded.
- Registry metadata is fetched from the public npm Registry, so private
  registries and packages may not provide details.
- Update commands inherit the package manager's behavior; there is no automatic
  rollback if an update fails.
- This initial version does not include vulnerability scanning or bulk updates.

## Development

```sh
cargo fmt
cargo test
cargo clippy
```
