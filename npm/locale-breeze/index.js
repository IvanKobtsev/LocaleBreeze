'use strict';

const platformPackages = {
  'win32-x64': ['@locale-breeze/language-server-win32-x64', 'locale-breeze.exe'],
  'win32-arm64': ['@locale-breeze/language-server-win32-arm64', 'locale-breeze.exe'],
  'darwin-x64': ['@locale-breeze/language-server-darwin-x64', 'locale-breeze'],
  'darwin-arm64': ['@locale-breeze/language-server-darwin-arm64', 'locale-breeze'],
  'linux-x64': ['@locale-breeze/language-server-linux-x64', 'locale-breeze'],
  'linux-arm64': ['@locale-breeze/language-server-linux-arm64', 'locale-breeze']
};

function resolveServerPath(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const selected = platformPackages[key];
  if (!selected) {
    throw new Error(`LocaleBreeze does not provide a binary for ${key}. Supported targets: ${Object.keys(platformPackages).join(', ')}`);
  }
  try {
    return require.resolve(`${selected[0]}/bin/${selected[1]}`);
  } catch (cause) {
    const error = new Error(
      `The optional package ${selected[0]} is missing. Reinstall @locale-breeze/language-server without disabling optional dependencies.`
    );
    error.cause = cause;
    throw error;
  }
}

module.exports = { serverPath: resolveServerPath(), resolveServerPath };
