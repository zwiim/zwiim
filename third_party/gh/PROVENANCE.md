# GitHub CLI

- **Source:** GitHub CLI (`gh`), the pinned release in `scripts/gh/fetch.mjs`
- **Version:** 2.102.1
- **Upstream:** https://github.com/cli/cli
- **License:** MIT (see `LICENSE` in this folder)
- **Used by:** the `gh` binary shipped next to the Host in the Desktop bundle and
  the Host update archive
- **How:** downloaded and checked against its pinned sha256 at build time, not
  copied into the tree. This folder only holds its texts for the third-party
  notices: `LICENSE` from the release archive, and `DEPENDENCIES.txt`, the output
  of `gh licenses` (the licenses built into the binary).
- **Updating:** after changing `GH_VERSION`, run
  `node scripts/gh/fetch.mjs --update-notices`, then
  `node scripts/licenses/notices.mjs`.
- **Modifications:** none.
