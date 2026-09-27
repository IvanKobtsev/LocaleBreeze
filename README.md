# LocaleBreeze

LocaleBreeze provides context-aware i18n navigation and completion for JetBrains IDEs. Its Rust language server understands full i18next keys and translators returned by scoped hooks without requiring a complete TypeScript type-checker.

## Install

[Install LocaleBreeze for WebStorm](https://plugins.jetbrains.com/plugin/33774-localebreeze)

Android Studio uses a separate `LocaleBreeze for Android Studio` plugin package backed by LSP4IJ. Until its separate Marketplace listing is published, contributors can build it from `editors/android-studio`.

The plugin runs all analysis locally: no project source or translations leave your machine.

## Why LocaleBreeze

- Completion by full key, relative key, or key fragment, prioritized above other suggestions.
- Rendered hover previews from the default translation file, including direct scope children and a clear message for missing keys.
- Go to Definition from full keys, scoped keys, and scope declarations to JSON dictionaries.
- Find References from dictionary properties back to full and scoped calls.
- Optional unused-key hints on unreferenced default-locale dictionary leaves.
- Incremental synchronization for TypeScript, TSX, JavaScript, JSX, and translation JSON.
- Fast, Rust-powered language intelligence focused on JetBrains IDEs.
- Copy a full translation key path directly from its JSON dictionary entry.

## Supported usage

Supported source forms:

```tsx
const i18n = useScopedTranslation('Page.Login');
i18n.t('submit');

const { t } = useScopedTranslation('Page.Login');
t('submit');

const common = useScopedTranslation('common:Page.Login');
common.t('submit');

i18next.t('Page.Login.submit');

const { t: commonT } = useTranslation('common', { keyPrefix: 'Page.Login' });
commonT('submit');

useTranslation(); // Uses defaultNamespace.
i18next.t('common:Page.Login.submit');
i18next.t('Page.Login.submit', { ns: 'common' });
```

## Configuration

When a project does not contain `locale-breeze.json`, the LocaleBreeze plugin offers to create a starter configuration at the workspace root. Adjust the generated dictionary pattern and other settings for your project. The pattern is relative to the workspace root, must contain exactly one `{locale}` token, may contain one `{namespace}` token, and cannot be absolute. For example, `public/dictionaries/{locale}/{namespace}.json` matches `public/dictionaries/en/common.json`. Parent-directory (`..`) segments are supported when dictionaries live above the workspace; that external dictionary directory is indexed and watched alongside the workspace. Set `defaultNamespace` when the pattern discovers multiple namespaces; LocaleBreeze infers it when exactly one namespace exists. The configured default locale and namespace must have a matching file.

LocaleBreeze automatically recognizes literal calls to `useTranslation` from `react-i18next` and `i18next.t` from `i18next`. Namespace arrays, computed namespaces and computed `keyPrefix` values are intentionally not resolved.

Custom functions are configured independently. Scoped functions may declare one returned translation method and one returned key method, while both scoped and full-key functions may override the global namespace. Translation methods validate interpolation options; key methods only resolve and track keys. Both `scopedFunctions` and `fullKeyFunctions` are optional and default to empty lists. `keySeparator` is also optional and defaults to `.`:

```json
{
  "scopedFunctions": [
    {
      "functionName": "useScopedTranslation",
      "defaultNamespace": "common",
      "translationMethod": "t",
      "keyMethod": "key"
    }
  ],
  "fullKeyFunctions": [
    { "functionName": "translate", "defaultNamespace": "common" }
  ]
}
```

Namespace resolution prefers an explicit namespace in a full key or scoped-function declaration, then the matched function's `defaultNamespace`, then the global `defaultNamespace`. Returned methods from configured scoped functions accept only relative keys and cannot override their declaration's namespace. Both scoped method fields are optional. The old string arrays, `translationMethods` arrays, and top-level `translationMethods` setting are no longer accepted.

Unused-key hints are a user preference rather than workspace configuration. They are enabled by default and can be switched off with **Show unused keys** in the JetBrains LocaleBreeze settings. Static-prefix templates such as ``i18next.t(`SomeScope.${value}`)`` mark every child of `SomeScope` as dynamically used; other unsupported dynamic references are not counted as uses.

Set `"ignoredScopes": ["Server_Errors"]` for translation scopes owned outside the frontend. Ignored scopes and their descendants are excluded from completion, hover, navigation, references, and unused-key analysis. Referencing one from frontend source produces an `ignored scope` warning instead.

Set `"translationKeyTypes": ["TranslationKey"]` to recognize string literals with an explicit matching type annotation, `as` assertion, or `satisfies` clause. Set `"translationKeyProps": ["transKey"]` to recognize literal values of matching JSX attributes and object properties. These checks are lexical and do not start a TypeScript type checker.

The checked-in schema is [`schemas/config-v1.schema.json`](schemas/config-v1.schema.json).

## Known limitations

- LocaleBreeze does not follow translators that are passed, returned, imported, reassigned, or otherwise escape their recognized lexical scope. It does not perform general-purpose cross-file data-flow analysis.
- Only string-valued JSON leaves are indexed. Arrays and non-string values are intentionally ignored because dictionaries are treated as translation resources rather than general-purpose data stores.
- The i18next features **contexts**, **fallback keys**, and **nesting**—where translation keys are referenced from within dictionary values—are not currently supported. These features can absolutely be added if there is demand for them.

## VS Code support

LocaleBreeze is now focused on JetBrains IDEs, and active support for Visual Studio Code is no longer provided. The existing VS Code extension remains available on the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=ivankobtsev.locale-breeze), but it should be considered a legacy integration and may not receive new features or compatibility fixes.

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

This produces six platform-specific VSIX files plus independent WebStorm and Android Studio plugin ZIPs under `dist/editors/`. Pass `--vscode-only`, `--webstorm-only`, or `--android-studio-only` to build one integration; `--jetbrains-only` builds both JetBrains packages. The JetBrains builds require JDK 25 through `JAVA_HOME`.

For VS Code development, install the dependencies in `editors/vscode`, run its compile script, and either copy the server into `bin/<platform>-<arch>/` or set `localeBreeze.server.path`.

### Development

Run `cargo test --workspace --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings`. CI validates Rust on Windows, macOS, and Linux and type-checks the VS Code client.
