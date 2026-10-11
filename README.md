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
