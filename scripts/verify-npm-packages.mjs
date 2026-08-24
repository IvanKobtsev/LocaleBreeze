import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const root = new URL('../', import.meta.url).pathname.replace(/^\/(.:\/)/, '$1');
const meta = JSON.parse(readFileSync(join(root, 'npm/locale-breeze/package.json'), 'utf8'));
if (process.env.GITHUB_REF_NAME?.startsWith('v') && process.env.GITHUB_REF_NAME.slice(1) !== meta.version) {
  throw new Error(`Tag ${process.env.GITHUB_REF_NAME} does not match npm ${meta.version}`);
}
const targets = [
  ['win32-x64', 'locale-breeze.exe'], ['win32-arm64', 'locale-breeze.exe'],
  ['darwin-x64', 'locale-breeze'], ['darwin-arm64', 'locale-breeze'],
  ['linux-x64', 'locale-breeze'], ['linux-arm64', 'locale-breeze']
];

for (const [target, binary] of targets) {
  const directory = join(root, 'npm/platforms', target);
  const manifest = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
  const dependency = manifest.name;
  if (manifest.version !== meta.version) throw new Error(`${target} version does not match launcher ${meta.version}`);
  if (meta.optionalDependencies[dependency] !== manifest.version) throw new Error(`${dependency} dependency does not match ${manifest.version}`);
  const binaryPath = join(directory, 'bin', binary);
  if (process.argv.includes('--require-binaries')) {
    if (!existsSync(binaryPath)) throw new Error(`Missing binary for ${target}`);
    verifyBinaryTarget(target, readFileSync(binaryPath));
  }
}
console.log(`Verified npm package manifests for ${meta.version}`);

function verifyBinaryTarget(target, bytes) {
  const [platform, arch] = target.split('-');
  if (platform === 'win32') {
    if (bytes[0] !== 0x4d || bytes[1] !== 0x5a) throw new Error(`${target} is not a PE executable`);
    const pe = bytes.readUInt32LE(0x3c);
    const machine = bytes.readUInt16LE(pe + 4);
    const expected = arch === 'x64' ? 0x8664 : 0xaa64;
    if (machine !== expected) throw new Error(`${target} has PE machine 0x${machine.toString(16)}`);
  } else if (platform === 'linux') {
    if (!bytes.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) throw new Error(`${target} is not ELF`);
    const expected = arch === 'x64' ? 0x3e : 0xb7;
    if (bytes.readUInt16LE(18) !== expected) throw new Error(`${target} has the wrong ELF architecture`);
  } else {
    if (bytes.readUInt32LE(0) !== 0xfeedfacf) throw new Error(`${target} is not 64-bit Mach-O`);
    const expected = arch === 'x64' ? 0x01000007 : 0x0100000c;
    if (bytes.readUInt32LE(4) !== expected) throw new Error(`${target} has the wrong Mach-O architecture`);
  }
}
