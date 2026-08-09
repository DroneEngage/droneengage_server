const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const out = path.join(root, 'build');

const exclude = new Set([
  path.join(root, 'build'),
  path.join(root, 'node_modules'),
  path.join(root, '.git'),
  path.join(root, '.vscode'),
  path.join(root, 'logs'),
  path.join(root, 'downloads'),
  path.join(root, 'ssl_local'),
  path.join(root, 'test'),
  path.join(root, 'wiki')
]);

function copy(src, dst) {
  if (exclude.has(src)) return;

  const lstat = fs.lstatSync(src);

  // Preserve symlinks as-is (skip broken ones).
  if (lstat.isSymbolicLink()) {
    let target;
    try {
      target = fs.readlinkSync(src);
    } catch (e) {
      return;
    }
    const dstDir = path.dirname(dst);
    if (!fs.existsSync(dstDir)) fs.mkdirSync(dstDir, { recursive: true });
    try {
      fs.symlinkSync(target, dst);
    } catch (e) {
      // ignore already-exists / broken target
    }
    return;
  }

  const stat = fs.statSync(src);
  if (stat.isDirectory()) {
    if (!fs.existsSync(dst)) fs.mkdirSync(dst, { recursive: true });
    for (const entry of fs.readdirSync(src)) {
      copy(path.join(src, entry), path.join(dst, entry));
    }
  } else {
    const dstDir = path.dirname(dst);
    if (!fs.existsSync(dstDir)) fs.mkdirSync(dstDir, { recursive: true });
    fs.copyFileSync(src, dst);
  }
}

if (fs.existsSync(out)) {
  fs.rmSync(out, { recursive: true, force: true });
}
fs.mkdirSync(out, { recursive: true });

for (const entry of fs.readdirSync(root)) {
  copy(path.join(root, entry), path.join(out, entry));
}

console.log('Build output written to:', out);
