# Workspace fixtures

Small monorepos for manually checking depee against real package managers.
Each has a root, `packages/app` (`@fx/app`) and `packages/lib` (`@fx/lib`).
`app` and `lib` both depend on `semver` with different ranges (`^6` and `^5`),
which is what exposes per-member filtering problems.

| Directory      | Manager        | Release-age gate                         |
| -------------- | -------------- | ---------------------------------------- |
| `npm`          | npm 11         | `.npmrc` `min-release-age=3` (days)      |
| `pnpm`         | pnpm 11        | `minimumReleaseAge: 4320` (minutes)      |
| `yarn-berry`   | Yarn 4.10.3    | `npmMinimalAgeGate: 4320` (minutes)      |
| `yarn-classic` | Yarn 1.22      | none available                           |
| `bun`          | Bun            | `minimumReleaseAge` (ignored by Bun 1.3.1) |

Install with scripts disabled, then check publish dates:

```sh
(cd npm && npm install --ignore-scripts)
(cd pnpm && pnpm install --ignore-scripts)
(cd yarn-classic && yarn install --ignore-scripts)
(cd yarn-berry && YARN_ENABLE_SCRIPTS=0 yarn install)
(cd bun && bun install --ignore-scripts)
./check-release-age.sh
```

Then run depee from a root (opens the member picker) or a member:

```sh
cargo run -- --cwd fixtures/workspaces/pnpm
cargo run -- --cwd fixtures/workspaces/npm --workspace @fx/lib
```

`node_modules` is ignored; lockfiles are committed so installs are reproducible.
