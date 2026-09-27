import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = join(root, 'dist', 'editors');
const staging = join(root, 'target', 'editor-release-staging');
const vscodeRoot = join(root, 'editors', 'vscode');
const webstormRoot = join(root, 'editors', 'jetbrains');
const androidStudioRoot = join(root, 'editors', 'android-studio');
const npmPlatforms = join(root, 'npm', 'platforms');
const targets = [
  ['win32-x64', 'locale-breeze.exe'], ['win32-arm64', 'locale-breeze.exe'],
  ['darwin-x64', 'locale-breeze'], ['darwin-arm64', 'locale-breeze'],
  ['linux-x64', 'locale-breeze'], ['linux-arm64', 'locale-breeze'],
];
const modes = process.argv.includes('--vscode-only') ? new Set(['vscode'])
  : process.argv.includes('--webstorm-only') ? new Set(['webstorm'])
  : process.argv.includes('--android-studio-only') ? new Set(['android-studio'])
  : process.argv.includes('--jetbrains-only') ? new Set(['webstorm', 'android-studio'])
  : new Set(['vscode', 'webstorm', 'android-studio']);

run(process.execPath, [join(root, 'scripts', 'verify-npm-packages.mjs'), '--require-binaries']);
rmSync(staging, { recursive: true, force: true });
mkdirSync(output, { recursive: true });
if (modes.has('vscode')) packageVsCode();
if (modes.has('webstorm') || modes.has('android-studio')) stageJetBrainsBinaries();
if (modes.has('webstorm')) packageWebStorm();
if (modes.has('android-studio')) packageAndroidStudio();
console.log(`Editor release artifacts are ready in ${output}`);

function packageVsCode() {
  run(command('pnpm'), ['run', 'check'], vscodeRoot);
  run(command('pnpm'), ['run', 'compile'], vscodeRoot);
  const manifest = json(join(vscodeRoot, 'package.json'));
  const vsce = join(vscodeRoot, 'node_modules', '@vscode', 'vsce', 'vsce');
  for (const file of [vsce, join(root, 'LICENSE'), join(vscodeRoot, 'README.md'),
    join(vscodeRoot, 'CHANGELOG.md'), join(vscodeRoot, 'dist', 'extension.js')]) requireFile(file);

  for (const [target, binaryName] of targets) {
    const stage = join(staging, `vscode-${target}`);
    mkdirSync(join(stage, 'dist'), { recursive: true });
    mkdirSync(join(stage, 'bin', target), { recursive: true });
    for (const file of ['package.json', 'README.md', 'CHANGELOG.md', '.vscodeignore']) {
      copyFileSync(join(vscodeRoot, file), join(stage, file));
    }
    copyFileSync(join(root, 'LICENSE'), join(stage, 'LICENSE'));
    copyFileSync(join(vscodeRoot, 'dist', 'extension.js'), join(stage, 'dist', 'extension.js'));
    const binary = join(npmPlatforms, target, 'bin', binaryName);
    const stagedBinary = join(stage, 'bin', target, binaryName);
    requireFile(binary);
    copyFileSync(binary, stagedBinary);
    if (!target.startsWith('win32-')) chmodSync(stagedBinary, 0o755);
    const artifact = join(output, `locale-breeze-${manifest.version}-${target}.vsix`);
    rmSync(artifact, { force: true });
    run(process.execPath, [vsce, 'package', '--no-dependencies', '--target', target, '--out', artifact], stage);
    requireNonEmpty(artifact);
  }
}

function stageJetBrainsBinaries() {
  const nativeRoot = join(root, 'dist', 'jetbrains');
  rmSync(nativeRoot, { recursive: true, force: true });
  for (const [target, binaryName] of targets) {
    const source = join(npmPlatforms, target, 'bin', binaryName);
    const destination = join(nativeRoot, target, binaryName);
    requireFile(source);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(source, destination);
    if (!target.startsWith('win32-')) chmodSync(destination, 0o755);
  }
}

function packageWebStorm() {
  packageJetBrainsPlugin(webstormRoot, 'locale-breeze-jetbrains', 'locale-breeze-webstorm', []);
}

function packageAndroidStudio() {
  packageJetBrainsPlugin(androidStudioRoot, 'locale-breeze-android-studio', 'locale-breeze-android-studio', [
    'verifyNoNativeLsp', 'verifyPlugin',
  ]);
}

function packageJetBrainsPlugin(pluginRoot, builtName, artifactName, extraTasks) {
  const wrapper = join(pluginRoot, process.platform === 'win32' ? 'gradlew.bat' : 'gradlew');
  if (process.platform !== 'win32') chmodSync(wrapper, 0o755);
  run(wrapper, [
    'buildPlugin', 'test', 'verifyPluginStructure', 'verifyBundledBinaries', ...extraTasks,
    '--rerun-tasks', '--no-configuration-cache',
  ], pluginRoot);
  const version = readProperties(join(pluginRoot, 'gradle.properties')).version;
  const built = join(pluginRoot, 'build', 'distributions', `${builtName}-${version}.zip`);
  requireNonEmpty(built);
  const artifact = join(output, `${artifactName}-${version}.zip`);
  copyFileSync(built, artifact);
  requireNonEmpty(artifact);
}

function command(name) { return process.platform === 'win32' ? `${name}.cmd` : name; }
function run(executable, args, cwd = root) {
  console.log(`> ${executable} ${args.join(' ')}`);
  const needsWindowsShell = process.platform === 'win32' && /\.(?:cmd|bat)$/i.test(executable);
  const result = spawnSync(executable, args, { cwd, stdio: 'inherit', shell: needsWindowsShell });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${executable} exited with status ${result.status}`);
}
function requireFile(path) {
  if (!existsSync(path) || !statSync(path).isFile()) throw new Error(`Missing required file: ${path}`);
}
function requireNonEmpty(path) {
  requireFile(path);
  if (statSync(path).size === 0) throw new Error(`Generated an empty artifact: ${path}`);
}
function json(path) { return JSON.parse(readFileSync(path, 'utf8')); }
function readProperties(path) {
  return Object.fromEntries(readFileSync(path, 'utf8').split(/\r?\n/).filter(Boolean).map(line => {
    const separator = line.indexOf('=');
    return [line.slice(0, separator), line.slice(separator + 1)];
  }));
}
