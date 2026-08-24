#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { serverPath } = require('./index.js');

const child = spawn(serverPath, process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal));
}
child.on('error', error => {
  console.error(`Failed to start LocaleBreeze: ${error.message}`);
  process.exitCode = 1;
});
child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
});

