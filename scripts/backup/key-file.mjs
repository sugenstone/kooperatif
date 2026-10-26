// key-file.mjs — KOOPERATIF_EXPORT_KEY_FILE loader.
// Fail-closed: symlink rejection (lstat), regular-file + fstat checks,
// 0600 permission enforcement on POSIX, 32-byte key material validation.
// Never logs key material or path contents.
import fs from 'node:fs';
import crypto from 'node:crypto';
import { ArchiveError } from './archive-crypto.mjs';

export const ENV_KEY_FILE = 'KOOPERATIF_EXPORT_KEY_FILE';
export const KEY_BYTES = 32;

export function loadExportKey(env = process.env) {
  const path = env[ENV_KEY_FILE];
  if (!path) throw new ArchiveError('keyfile-unset', `${ENV_KEY_FILE} is not set`);

  let st;
  try {
    st = fs.lstatSync(path);
  } catch {
    throw new ArchiveError('keyfile-unreadable', 'key file cannot be stat\'ed');
  }
  if (st.isSymbolicLink()) throw new ArchiveError('keyfile-symlink', 'key file must not be a symlink');

  // O_RDONLY + fstat for TOCTOU: the fd is checked after open.
  let fd;
  try {
    fd = fs.openSync(path, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
  } catch {
    throw new ArchiveError('keyfile-unreadable', 'key file cannot be opened');
  }
  try {
    const fst = fs.fstatSync(fd);
    if (!fst.isFile()) throw new ArchiveError('keyfile-not-regular', 'key file is not a regular file');
    if (fst.size !== KEY_BYTES && fst.size !== 2 * KEY_BYTES) {
      throw new ArchiveError('keyfile-size', `key file must be ${KEY_BYTES} bytes or ${2 * KEY_BYTES} hex chars`);
    }
    if (process.platform !== 'win32') {
      const mode = fst.mode & 0o777;
      if (mode !== 0o600) throw new ArchiveError('keyfile-perms', `mode ${mode.toString(8)} != 600`);
    }
    const raw = Buffer.alloc(fst.size);
    fs.readSync(fd, raw, 0, fst.size, 0);
    const key = fst.size === KEY_BYTES ? raw : (() => {
      const s = raw.toString('ascii').trim();
      if (!/^[0-9a-fA-F]{64}$/.test(s)) throw new ArchiveError('keyfile-format', 'hex key must be 64 chars');
      return Buffer.from(s, 'hex');
    })();
    if (key.length !== KEY_BYTES) throw new ArchiveError('keyfile-format', 'decoded key is not 32 bytes');
    const keyId = crypto.createHash('sha256').update(key).digest('hex').slice(0, 16);
    return { key, keyId };
  } finally {
    fs.closeSync(fd);
  }
}
