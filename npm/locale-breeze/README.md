# @locale-breeze/language-server

Native LocaleBreeze i18n language-server binaries for Windows, macOS, and Linux.

```sh
npm install --save-dev @locale-breeze/language-server
npx locale-breeze lsp --stdio
```

Editor integrations can locate the native executable without starting it:

```js
const { serverPath } = require('@locale-breeze/language-server');
```

The package installs the binary matching the current operating system and CPU through an optional platform dependency. Do not install with optional dependencies disabled.
