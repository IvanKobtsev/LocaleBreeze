# LocaleBreeze

LocaleBreeze is a cross-editor language server for context-aware i18n navigation and completion. Its Rust server understands full i18next keys and translators returned by scoped hooks without requiring a complete TypeScript type-checker.

## MVP capabilities

- Completion by full key, relative key, key fragment, or translated value from any locale.
- Go to Definition from full keys, scoped keys, and scope declarations to JSON dictionaries.
- Find References from dictionary properties back to full and scoped calls.
- Incremental synchronization for TypeScript, TSX, JavaScript, JSX, and translation JSON.
- Thin VS Code integration; no project source or translations leave the machine.

Supported source forms:

```tsx
const i18n = useScopedTranslation('Page.Login');
i18n.t('submit');

const { t } = useScopedTranslation('Page.Login');
t('submit');

i18next.t('Page.Login.submit');
```

Only literal scopes and keys are resolved. Bindings are followed inside their lexical block; passed, returned, imported, or reassigned translators are intentionally outside the MVP.

## Configuration

Copy `locale-breeze.example.json` to `locale-breeze.json` at the workspace root and adjust the dictionary pattern. The pattern must contain exactly one `{locale}` token, and the configured default locale must have a matching file.

The checked-in schema is [`schemas/config-v1.schema.json`](schemas/config-v1.schema.json). The public raw-GitHub URL in the example becomes usable when this repository is published; it replaces the unprovisioned `localebreeze.dev` URL from the design draft.

## Build and run

```text
cargo build --release -p locale-breeze
target/release/locale-breeze lsp --stdio
```

The native executable is also distributed through npm:

```text
npm install --save-dev @locale-breeze/language-server
npx locale-breeze lsp --stdio
```

The npm launcher installs only the binary for the current operating system and architecture. Release tags publish six platform packages plus the launcher package through GitHub Actions with npm provenance.

For VS Code development, install the dependencies in `editors/vscode`, run its compile script, and either copy the server into `bin/<platform>-<arch>/` or set `localeBreeze.server.path`.

## Known MVP limits

- One dictionary pattern and logical translation module per workspace.
- String-valued JSON leaves only; arrays and non-string leaves are ignored.
- No diagnostics, rename, hover, CodeLens, namespaces, or cross-file data-flow yet.
- The JetBrains launcher is planned after the server protocol is stabilized.

## Development

Run `cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings`. CI validates Rust on Windows, macOS, and Linux and type-checks the VS Code client.
