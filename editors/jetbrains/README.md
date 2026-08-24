# LocaleBreeze for WebStorm

This is a thin WebStorm launcher for the LocaleBreeze Rust language server. The current release targets WebStorm 2026.2.1 through the end of the 2026.2 line.

## Development

Install a JDK 25 and run `gradlew runIde`. In the development WebStorm instance, open **Settings | Tools | LocaleBreeze** and select a locally built `locale-breeze` executable. The configuration path is optional; when empty, the server reads `locale-breeze.json` from the project root.

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

The Gradle build copies that tree into the plugin's `bin/` directory. A configured server path overrides the bundled executable.

From the repository root, run `node scripts/package-editors.mjs`. It validates all native binaries, creates six platform-specific VSIX files, bundles every binary into the JetBrains ZIP, and verifies the JetBrains archive structure. Artifacts are written to `dist/editors/`.
