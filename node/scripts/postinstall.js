// Fetch the platform binary for this package version from the GitHub Release.
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const { version, binaryPath } = require('../index.js');

const targets = {
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
};
const key = `${process.platform}-${process.arch}`;
const target = targets[key];
if (!target) {
  console.warn(`autoinference: no prebuilt binary for ${key}; install with \`cargo install autoinference\`.`);
  process.exit(0);
}
const url = `https://github.com/autoinference/autoinference/releases/download/v${version}/autoinference-${target}.tar.gz`;
(async () => {
  try {
    const res = await fetch(url, { redirect: 'follow' });
    if (!res.ok) throw new Error(`${res.status} ${res.statusText} for ${url}`);
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'autoinference-'));
    const tgz = path.join(tmp, 'a.tar.gz');
    fs.writeFileSync(tgz, Buffer.from(await res.arrayBuffer()));
    execFileSync('tar', ['-xzf', tgz, '-C', tmp]);
    fs.mkdirSync(path.dirname(binaryPath), { recursive: true });
    fs.copyFileSync(path.join(tmp, 'autoinference'), binaryPath);
    fs.chmodSync(binaryPath, 0o755);
    console.log(`autoinference ${version} installed for ${target}`);
  } catch (e) {
    console.warn(`autoinference: could not fetch prebuilt binary (${e.message}). Install with \`cargo install autoinference\`.`);
  }
})();
