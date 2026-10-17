# Material Icon Theme

- **Source:** Material Icon Theme (the VS Code extension's icon set), npm package `material-icon-theme`
- **Version:** 5.39.0
- **Upstream:** https://github.com/material-extensions/vscode-material-icon-theme
- **License:** MIT (see `LICENSE` in this folder)
- **Used by:** Desktop file-type and folder icons — `clients/desktop/src/lib/file-icons.ts`,
  `clients/desktop/src/components/FileIcon.tsx`
- **How:** installed as a dependency, not copied into the tree. The app bundles the
  package's unmodified SVGs (`icons/*.svg`) as separate assets and looks them up with
  the package's own manifest (`dist/material-icons.json`): exact file names first, then
  the longest known extension, then the generic `file`/`folder` icon; light-theme
  overrides where the manifest has them.
- **Modifications:** none to the icons or the manifest.
