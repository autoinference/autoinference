// npm launcher for the autoinference Rust binary. The binary is fetched from the matching
// GitHub Release at install time (scripts/postinstall.js) and exec'd by bin/autoinference.js.
const path = require('path');
const { version } = require('./package.json');
module.exports = {
  version,
  binaryPath: path.join(__dirname, 'bin', process.platform === 'win32' ? 'autoinference.exe' : 'autoinference-bin'),
};
