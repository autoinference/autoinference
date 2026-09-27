#!/usr/bin/env node
const { spawnSync } = require('child_process');
const fs = require('fs');
const { binaryPath } = require('../index.js');
if (!fs.existsSync(binaryPath)) {
  console.error('autoinference: binary not installed. Re-run `npm install autoinference` (postinstall fetches it), or install with `cargo install autoinference`.');
  process.exit(1);
}
const r = spawnSync(binaryPath, process.argv.slice(2), { stdio: 'inherit' });
process.exit(r.status ?? 1);
