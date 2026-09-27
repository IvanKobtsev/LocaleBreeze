# LocaleBreeze for JetBrains IDEs

This is a thin JetBrains launcher for the LocaleBreeze Rust language server. The current release targets the 2026.2 IDE line, including Android Studio builds based on IntelliJ Platform 262. Android Studio uses the required [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) client because JetBrains' built-in LSP API is not shipped by Google; commercial JetBrains IDEs continue using the native JetBrains LSP integration and its full LocaleBreeze tool-window experience.

## Development

Install a JDK 25 and run `gradlew runIde`. The plugin uses its bundled LocaleBreeze executable and automatically enables itself when it finds `locale-breeze.json` at the workspace root or at a previously selected custom path. Otherwise, in commercial JetBrains IDEs it searches indexed project content for nested configurations and offers them in the LocaleBreeze tool window without starting the language server. The tool window can also select another JSON file or create a starter configuration at the workspace root. Use **Settings | Tools | LocaleBreeze** to choose the expected configuration location, explicitly opt in or out for the workspace, or change the global **Show unused keys** preference.

When the configured dictionary directory is outside the project content, the LocaleBreeze tool window in commercial JetBrains IDEs offers **Attach dictionary directory**. The action adds that directory as a project content root so standard LSP diagnostics work there. LocaleBreeze records roots attached through this action and removes an obsolete one when the configured dictionary root changes.

## Native binaries

One JetBrains plugin archive is cross-platform, so release builds bundle all six binaries under:

```text
dist/jetbrains/
  win32-x64/locale-breeze.exe
  win32-arm64/locale-breeze.exe
  darwin-x64/locale-breeze
  darwin-arm64/locale-breeze
  linux-x64/locale-breeze
  linux-arm64/locale-breeze
```

The Gradle build copies that tree into the plugin's `bin/` directory.

From the repository root, run `node scripts/package-editors.mjs`. It validates all native binaries, creates six platform-specific VSIX files, bundles every binary into the JetBrains ZIP, and verifies the JetBrains archive structure. Artifacts are written to `dist/editors/`.
