# Contributing to Zwiim

Thanks for helping. Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- For anything beyond a small fix, open an issue first so we can agree on the
  approach.
- Report security problems privately: see [SECURITY.md](SECURITY.md).

## Contributor License Agreement

Before your first pull request can be merged, you must sign the
[CLA](docs/community/CLA.md). A bot asks you on the pull request. You keep your
copyright, and the CLA lets the project relicense and transfer contributions.

## Making a change

1. Fork the repository and create a branch.
2. See [Build from source](README.md#build-from-source).
3. Keep each pull request to one purpose, with small commits.
4. Add tests for behaviour changes.
5. Run the checks below, then open a pull request against `master`.

```sh
grep -v '^#' .github/rustfmt-owned.txt | xargs rustfmt --edition 2021 --check
cargo clippy -p zwiim-host -p zwiim-cloud -p zwiim-desktop --all-targets -- -D clippy::correctness -D clippy::suspicious
cargo test -p zwiim-host -p zwiim-cloud -p zwiim-desktop
npm run typecheck --workspace clients/desktop
npm test --workspace clients/desktop
node scripts/licenses/notices.mjs --check
```

The notices check needs `cargo install --locked cargo-about --version 0.9.2 --features cli`.

## Project rules

- **The Host owns the truth.** Clients send commands and render events. They do
  not run host operations themselves.
- **Local work never depends on the Cloud.** Remote access can fail without
  stopping anything local.
- **Clients use the Zwiim protocol** (`/zwiim/v1`), not internal APIs. Add
  protocol operations only when a feature needs them.
- **Visual changes follow [DESIGN.md](DESIGN.md).**
- **No copyleft dependencies.** Shipped code must stay compatible with
  Apache-2.0, and CI enforces this. If you copy code from elsewhere, keep its
  licence and notices, and say so in your pull request.
- **User-facing changes get a line in [CHANGELOG.md](CHANGELOG.md)** under
  `[Unreleased]`.
