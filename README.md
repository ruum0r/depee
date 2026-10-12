# depee

`depee` is a Rust terminal dashboard for inspecting outdated dependencies in a
Node.js project. It reads `package.json`, detects npm, pnpm, Yarn, or Bun, and
uses that package manager's CLI to find outdated packages.

## Run

```sh
cargo run --release
cargo run --release -- --cwd ./my-project
cargo run --release -- --pm pnpm
```

The `--pm` option accepts `npm`, `pnpm`, `yarn`, or `bun`. It overrides
`packageManager` in `package.json` and lockfile detection. The selected package
manager must be installed and available on `PATH`.

## Install (macOS and Linux)

Release binaries are built for macOS (Apple Silicon and Intel) and Linux
(x86_64 and ARM64). Once a release has been published, Rust and Cargo are not
required to install or run its binary. Install the latest release with:

```sh
curl -fsSL https://raw.githubusercontent.com/ruum0r/depee/main/install.sh | sh
```

The installer puts `depee` in `~/.local/bin`. Set `DEPEE_INSTALL_DIR` to choose a
different directory. Make sure the install directory is on your `PATH`.

The binary still needs npm, pnpm, Yarn, or Bun on `PATH` to inspect and update a
project. Pushing a version tag such as `v0.1.0` builds and publishes the release
archives used by the installer.

The installer checks the archive against the release's `SHA256SUMS`. This
detects transfer errors; the checksum alone does not authenticate the publisher.
The release workflow also attaches signed build provenance. If GitHub CLI is
installed and authenticated, the installer verifies that provenance. You can
also verify it manually after downloading an archive:

```sh
gh attestation verify depee-aarch64-apple-darwin.tar.gz --repo ruum0r/depee \
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
| `?` | Show the key summary |
| `q` | Quit |

The package table shows the current version, newest stable patch and minor
versions, newest stable release, and the publication age of the latest tagged
release. Registry lookups run in the background with at most four requests at
once.

Registry details are loaded asynchronously and cached for 12 hours under the
platform cache directory. Network failures leave the package list usable.
Updates are run as argument arrays through the detected package manager and
only after confirmation.

## Current limitations

- `outdated` output formats vary between package manager releases. npm and
  Bun's JSON output and the common pnpm/Yarn JSON forms are supported; unusual
  CLI output may need parser adjustments.
- Yarn Plug'n'Play and workspaces are not managed as a separate project model.
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
