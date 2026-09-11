# LocaleBreeze for WebStorm

This is a thin WebStorm launcher for the LocaleBreeze Rust language server. The current release targets WebStorm 2026.2.1 through the end of the 2026.2 line.

## Development

Install a JDK 25 and run `gradlew runIde`. The plugin uses its bundled LocaleBreeze executable. In the development WebStorm instance, open **Settings | Tools | LocaleBreeze** to choose an optional workspace-specific configuration path and, when needed, override the configuration file's unused-key behavior.

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
