# LocaleBreeze for WebStorm

This is a thin WebStorm launcher for the LocaleBreeze Rust language server. The current release targets WebStorm 2026.2.1 through the end of the 2026.3 line.

## Development

Install a JDK 25 and run `gradlew runIde`. The plugin uses its bundled LocaleBreeze executable and automatically enables itself when it finds `locale-breeze.json` at the workspace root or at a previously selected custom path. Otherwise, it searches indexed project content for nested configurations and offers them in the LocaleBreeze tool window without starting the language server. The tool window can also select another JSON file or create a starter configuration at the workspace root. Use **Settings | Tools | LocaleBreeze** to choose the expected configuration location, explicitly opt in or out for the workspace, or change the global **Show unused keys** preference.

When the configured dictionary directory is outside the WebStorm project content, the LocaleBreeze tool window offers **Attach dictionary directory**. The action adds that directory as a project content root so standard LSP diagnostics work there. LocaleBreeze records roots attached through this action and removes an obsolete one when the configured dictionary root changes.

## Native binaries

WebStorm releases use six native variants, each bundling only its matching language server. Binaries are staged under:

```text
dist/jetbrains/
  win32-x64/locale-breeze.exe
  win32-arm64/locale-breeze.exe
  darwin-x64/locale-breeze
  darwin-arm64/locale-breeze
  linux-x64/locale-breeze
  linux-arm64/locale-breeze
```

Gradle `buildPluginVariants` creates Windows, macOS, and Linux ZIPs for x86_64 and ARM64. Each declares matching OS and architecture requirements under the same plugin ID and uses a platform-suffixed version, preserving its `bin/<platform>/` path.

From the repository root, run `node scripts/package-editors.mjs`. It validates all native binaries, creates six platform-specific VSIX files, creates six platform-specific WebStorm ZIPs (Android Studio remains universal), and verifies the JetBrains archive structure. Artifacts are written to `dist/editors/`.
