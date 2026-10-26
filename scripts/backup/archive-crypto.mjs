// archive-crypto.mjs — M1-P9 cooperative technical-archive envelope v1.
// Chunked AES-256-GCM over a length-prefixed record stream.
// Pure module: no database, filesystem or process-level I/O.
import crypto from 'node:crypto';

export const ENVELOPE_VERSION = 2; // v2: HKDF per-archive key derivation
export const ALG = 'aes-256-gcm';
export const CHUNK_MAX_PLAINTEXT = 1024 * 1024;
export const FRAME_MAX = 4 * 1024 * 1024;
export const MAX_CHUNKS = 0xffffffff; // counter field is u32
export const TAG_LEN = 16;
export const HKDF_INFO = 'koop-archive-key/v2';

// Per-archive key: HKDF-SHA256(master, salt=export_id, info=const) -> 32B.
// Cross-archive nonce-reuse requires the SAME export_id, which the caller
// mints as crypto.randomUUID() at ATTEMPT time.
export function deriveArchiveKey(masterKey, exportId) {
  if (!Buffer.isBuffer(masterKey) || masterKey.length !== 32) {
    throw new ArchiveError('key-invalid', 'master key must be 32 bytes');
  }
  if (typeof exportId !== 'string' || exportId.length === 0) {
    throw new ArchiveError('export-id-invalid', 'export_id required');
  }
  return Buffer.from(
    crypto.hkdfSync('sha256', masterKey, Buffer.from(exportId, 'utf8'), Buffer.from(HKDF_INFO, 'utf8'), 32),
  );
}

export class ArchiveError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'ArchiveError';
    this.code = code;
  }
}

const canon = (o) => JSON.stringify(o); // callers construct objects with fixed key order
const toUtf8 = (o) => Buffer.from(canon(o), 'utf8');
const hex = (b) => Buffer.from(b).toString('hex');
const unhex = (s) => Buffer.from(s, 'hex');

// ---------- framing (length-prefixed binary) ----------
export function frameEncode(record) {
  const payload = toUtf8(record);
  if (payload.length > FRAME_MAX) throw new ArchiveError('frame-oversize', `frame exceeds ${FRAME_MAX}`);
  const out = Buffer.alloc(4 + payload.length);
  out.writeUInt32BE(payload.length, 0);
  payload.copy(out, 4);
  return out;
}

export function createFrameDecoder() {
  let buf = Buffer.alloc(0);
  return {
    push(chunk) {
      buf = Buffer.concat([buf, chunk]);
      const out = [];
      for (;;) {
        if (buf.length < 4) break;
        const len = buf.readUInt32BE(0);
        if (len > FRAME_MAX) throw new ArchiveError('frame-oversize', `declared length ${len}`);
        if (buf.length < 4 + len) break;
        const payload = buf.subarray(4, 4 + len);
        let rec;
        try {
          rec = JSON.parse(payload.toString('utf8'));
        } catch {
          throw new ArchiveError('frame-malformed', 'frame payload is not valid JSON');
        }
        out.push({ record: rec, raw: Buffer.from(payload) });
        buf = buf.subarray(4 + len);
      }
      return out;
    },
    finish() {
      if (buf.length !== 0) throw new ArchiveError('frame-truncated', `${buf.length} trailing bytes`);
    },
  };
}

// ---------- nonce ----------
function chunkNonce(base8, index) {
  if (index > MAX_CHUNKS) throw new ArchiveError('nonce-exhausted', 'chunk counter overflow');
  const n = Buffer.alloc(12);
  base8.copy(n, 0);
  n.writeUInt32BE(index, 8);
  return n;
}

function chunkAad(header, index, isLast, chunkType) {
  return toUtf8({
    aad: 'koop-archive-chunk',
    v: ENVELOPE_VERSION,
    key_id: header.key_id,
    export_id: header.export_id,
    idx: index,
    last: isLast ? 1 : 0,
    type: chunkType,
    hdr: header.sha256,
  });
}

// ---------- writer ----------
export function createWriter({ masterKey, keyId, exportId, targetCoopId, schemaVersion, createdAt }) {
  const key = deriveArchiveKey(masterKey, exportId);
  const nonceBase = crypto.randomBytes(8);
  const headerObj = {
    v: ENVELOPE_VERSION,
    alg: ALG,
    key_id: keyId,
    nonce_base: hex(nonceBase),
    export_id: exportId,
    target_coop_id: targetCoopId,
    schema_version: schemaVersion,
    created_at: createdAt ?? new Date().toISOString(),
  };
  const headerBytes = toUtf8(headerObj);
  headerObj.sha256 = hex(crypto.createHash('sha256').update(headerBytes).digest());
  // Header prefix: u32 len + header JSON (plaintext, AAD-bound via hdr sha256).
  const header = Buffer.alloc(4 + headerBytes.length);
  header.writeUInt32BE(headerBytes.length, 0);
  headerBytes.copy(header, 4);

  let idx = 0;
  let pending = Buffer.alloc(0);
  const emit = (plaintext, isLast, type) => {
    const nonce = chunkNonce(nonceBase, idx);
    const aad = chunkAad(headerObj, idx, isLast, type);
    const cipher = crypto.createCipheriv('aes-256-gcm', key, nonce);
    cipher.setAAD(aad);
    const ct = Buffer.concat([cipher.update(plaintext), cipher.final()]);
    const tag = cipher.getAuthTag();
    idx += 1;
    const out = Buffer.alloc(4 + ct.length + TAG_LEN);
    out.writeUInt32BE(ct.length + TAG_LEN, 0);
    ct.copy(out, 4);
    tag.copy(out, 4 + ct.length);
    return out;
  };

  return {
    header,
    get chunkIndex() { return idx; },
    pushRecord(record) {
      const f = frameEncode(record);
      if (f.length > CHUNK_MAX_PLAINTEXT) throw new ArchiveError('frame-oversize', 'record exceeds chunk capacity');
      if (pending.length + f.length > CHUNK_MAX_PLAINTEXT) {
        const c = emit(pending, false, 'data');
        pending = Buffer.alloc(0);
        return c;
      }
      pending = Buffer.concat([pending, f]);
      return null;
    },
    finalize(manifestRecord) {
      pending = Buffer.concat([pending, frameEncode({ type: 'manifest', manifest: manifestRecord })]);
      pending = Buffer.concat([pending, frameEncode({ type: 'final', end_of_archive: true })]);
      const out = emit(pending, true, 'final');
      pending = Buffer.alloc(0);
      return out;
    },
  };
}

// ---------- reader ----------
export function parseArchive({ masterKey, bytes }) {
  if (!Buffer.isBuffer(masterKey) || masterKey.length !== 32) throw new ArchiveError('key-invalid', 'master key must be 32 bytes');
  if (bytes.length < 4) throw new ArchiveError('archive-truncated', 'no header');
  const hlen = bytes.readUInt32BE(0);
  if (hlen <= 0 || hlen > FRAME_MAX) throw new ArchiveError('header-oversize', `header length ${hlen}`);
  if (bytes.length < 4 + hlen) throw new ArchiveError('archive-truncated', 'incomplete header');
  const headerBytes = bytes.subarray(4, 4 + hlen);
  let header;
  try {
    header = JSON.parse(headerBytes.toString('utf8'));
  } catch {
    throw new ArchiveError('header-malformed', 'header is not JSON');
  }
  const key = deriveArchiveKey(masterKey, header.export_id);
  if (header.v !== ENVELOPE_VERSION || header.alg !== ALG) {
    throw new ArchiveError('header-unsupported', `v=${header.v} alg=${header.alg}`);
  }
  const nonceBase = unhex(header.nonce_base ?? '');
  if (nonceBase.length !== 8) throw new ArchiveError('header-malformed', 'bad nonce_base');
  header.sha256 = hex(crypto.createHash('sha256').update(headerBytes).digest());

  const records = [];
  const decoder = createFrameDecoder();
  let off = 4 + hlen;
  let idx = 0;
  let sawFinal = false;
  let manifest = null;
  while (off < bytes.length) {
    if (bytes.length - off < 4) throw new ArchiveError('chunk-truncated', 'chunk length prefix missing');
    const clen = bytes.readUInt32BE(off);
    if (clen <= TAG_LEN || clen > CHUNK_MAX_PLAINTEXT + TAG_LEN + 64) {
      throw new ArchiveError('chunk-oversize', `declared chunk ${clen}`);
    }
    if (bytes.length - off < 4 + clen) throw new ArchiveError('chunk-truncated', 'incomplete chunk');
    const ct = bytes.subarray(off + 4, off + 4 + clen - TAG_LEN);
    const tag = bytes.subarray(off + 4 + clen - TAG_LEN, off + 4 + clen);
    const isLast = off + 4 + clen === bytes.length;
    const type = isLast ? 'final' : 'data';
    try {
      const dec = crypto.createDecipheriv('aes-256-gcm', key, chunkNonce(nonceBase, idx));
      dec.setAAD(chunkAad(header, idx, isLast, type));
      dec.setAuthTag(tag);
      const pt = Buffer.concat([dec.update(ct), dec.final()]);
      for (const { record } of decoder.push(pt)) {
        if (record.type === 'manifest') manifest = record.manifest;
        else if (record.type === 'final') sawFinal = record.end_of_archive === true;
        else records.push(record);
      }
    } catch (e) {
      if (e instanceof ArchiveError) throw e;
      throw new ArchiveError('chunk-auth-failed', `chunk ${idx} failed authentication`);
    }
    off += 4 + clen;
    idx += 1;
    if (idx > MAX_CHUNKS) throw new ArchiveError('nonce-exhausted', 'chunk counter overflow');
  }
  decoder.finish();
  if (!sawFinal || !manifest) throw new ArchiveError('archive-incomplete', 'missing final chunk or manifest');
  return { header, records, manifest, chunkCount: idx };
}
