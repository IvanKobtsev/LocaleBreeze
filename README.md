# LocaleBreeze

LocaleBreeze is a cross-editor language server for context-aware i18n navigation and completion. Its Rust server understands full i18next keys and translators returned by scoped hooks without requiring a complete TypeScript type-checker.

## Install

- [Install LocaleBreeze for JetBrains IDEs](https://plugins.jetbrains.com/plugin/33774-localebreeze)
- [Install LocaleBreeze for Visual Studio Code](https://marketplace.visualstudio.com/items?itemName=ivankobtsev.locale-breeze)

Both extensions run the analysis locally: no project source or translations leave your machine.

## Why LocaleBreeze

- Completion by full key, relative key, or key fragment, prioritized above other suggestions.
- Rendered hover previews from the default translation file, including direct scope children and a clear message for missing keys.
- Go to Definition from full keys, scoped keys, and scope declarations to JSON dictionaries.
- Find References from dictionary properties back to full and scoped calls.
- Optional unused-key hints on unreferenced default-locale dictionary leaves.
- Incremental synchronization for TypeScript, TSX, JavaScript, JSX, and translation JSON.
- The same fast, Rust-powered language intelligence in VS Code and JetBrains IDEs.
- Copy a full translation key path directly from its JSON dictionary entry.

## MVP limitations

LocaleBreeze is currently an MVP focused on literal i18next translation flows. It is most useful when your project follows the supported patterns below and can accept these boundaries:

- Only literal scopes and keys are resolved.
- Bindings are followed inside their lexical block; passed, returned, imported, or reassigned translators are not followed.
- One dictionary pattern and logical translation module are supported per workspace.
- Only string-valued JSON leaves are indexed; arrays and non-string leaves are ignored.
- Missing-key diagnostics, rename, hover, CodeLens, namespaces, and cross-file data flow are not available yet.
- The JetBrains integration currently targets the 2026.2 IDE line.

## Supported usage

Supported source forms:

```tsx
const i18n = useScopedTranslation('Page.Login');
i18n.t('submit');

const { t } = useScopedTranslation('Page.Login');
t('submit');

i18next.t('Page.Login.submit');
```

## Configuration

Copy `locale-breeze.example.json` to `locale-breeze.json` at the workspace root and adjust the dictionary pattern. The pattern must contain exactly one `{locale}` token, and the configured default locale must have a matching file.

Set `"unusedKeys": true` to fade unreferenced leaf keys in the default-locale dictionary and list them as hints in VS Code's Problems panel. Since LocaleBreeze intentionally analyzes only supported literal translation flows, dynamic or otherwise unsupported references are not counted as uses.

Set `"translationKeyTypes": ["TranslationKey"]` to recognize string literals with an explicit matching type annotation, `as` assertion, or `satisfies` clause. Set `"translationKeyProps": ["transKey"]` to recognize literal values of matching JSX attributes and object properties. These checks are lexical and do not start a TypeScript type checker.

The checked-in schema is [`schemas/config-v1.schema.json`](schemas/config-v1.schema.json).

## Contributing

The sections below are for contributors building or developing LocaleBreeze itself. Extension users do not need these steps.

### Build and run

```text
cargo build --release -p locale-breeze
target/release/locale-breeze lsp --stdio
```

The native executable is also distributed through npm:

```text
npm install --save-dev @locale-breeze/language-server
npx locale-breeze lsp --stdio
```

The npm launcher installs only the binary for the current operating system and architecture.

### Package the editor integrations

With the six native npm-package binaries populated and the VS Code dependencies installed, run:

```text
node scripts/package-editors.mjs
```

This produces six platform-specific VSIX files and one all-platform WebStorm plugin ZIP under `dist/editors/`. Pass `--vscode-only` or `--jetbrains-only` to build only one editor integration. The JetBrains build requires JDK 25 through `JAVA_HOME`.

For VS Code development, install the dependencies in `editors/vscode`, run its compile script, and either copy the server into `bin/<platform>-<arch>/` or set `localeBreeze.server.path`.

### Development

Run `cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings`. CI validates Rust on Windows, macOS, and Linux and type-checks the VS Code client.
