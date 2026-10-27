// coop-export-driver.mjs — real PostgreSQL session driver for the export.
// Spawns `docker exec -i <container> psql -tAXq -v ON_ERROR_STOP=1 -d <db>`,
// feeds the single-snapshot script on stdin and yields stdout lines.
// Boundary-checked: nonzero exit or missing E markers propagate as errors.
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import { ArchiveError } from './archive-crypto.mjs';

export const DEFAULT_CONTAINER = 'kooperatif-dev-postgres-1';
export const DEFAULT_PGUSER = 'kooperatif';

export function createDockerPsqlSession({ container = DEFAULT_CONTAINER, database, user = DEFAULT_PGUSER } = {}) {
  if (!database) throw new ArchiveError('driver-args', 'database required');

  async function* sessionDriver(script) {
    const proc = spawn('docker', [
      'exec', '-i', container,
      'psql', '-U', user, '-d', database,
      '-tAXq', '-v', 'ON_ERROR_STOP=1',
    ], { stdio: ['pipe', 'pipe', 'pipe'] });

    const stderr = [];
    proc.stderr.on('data', (d) => stderr.push(d));
    proc.stdin.write(script);
    proc.stdin.end();

    const rl = readline.createInterface({ input: proc.stdout, crlfDelay: Infinity });
    let exited = null;
    proc.on('exit', (code, sig) => { exited = { code, sig }; });
    for await (const line of rl) {
      yield line;
    }
    // stdout ended: surface process failure
    const err = Buffer.concat(stderr).toString('utf8').trim();
    if (err) throw new ArchiveError('driver-stderr', err.slice(0, 500));
    if (exited && exited.code !== 0) {
      throw new ArchiveError('driver-exit', `psql exited ${exited.code}`);
    }
  }

  // execSql for audit writes: returns true on successful INSERT.
  async function execSql(sql) {
    return await new Promise((resolve) => {
      const p = spawn('docker', [
        'exec', '-i', container,
        'psql', '-U', user, '-d', database, '-tAXq', '-c', sql,
      ], { stdio: ['ignore', 'pipe', 'pipe'] });
      p.on('error', () => resolve(false));
      p.on('exit', (code) => resolve(code === 0));
    });
  }

  // runScript: pipe a multi-statement script via stdin (-f -); BEGIN/COMMIT
  // boundaries are honored per-statement (unlike -c's implicit-tx mode).
  async function runScript(script) {
    return await new Promise((resolve) => {
      const err = [];
      const p = spawn('docker', [
        'exec', '-i', container,
        'psql', '-U', user, '-d', database, '-tAXq', '-v', 'ON_ERROR_STOP=1', '-f', '-',
      ], { stdio: ['pipe', 'ignore', 'pipe'] });
      p.stderr.on('data', (d) => err.push(d));
      p.on('error', () => resolve(false));
      p.stdin.write(script);
      p.stdin.end();
      p.on('exit', (code) => {
        if (code !== 0) console.error('[restore stderr]', Buffer.concat(err).toString('utf8').slice(0, 400));
        resolve(code === 0);
      });
    });
  }

  // query helper for probes/preflight (single -c call, returns text rows).
  async function query(sql) {
    return await new Promise((resolve, reject) => {
      const out = [];
      const p = spawn('docker', [
        'exec', '-i', container,
        'psql', '-U', user, '-d', database, '-tAXq', '-c', sql,
      ], { stdio: ['ignore', 'pipe', 'pipe'] });
      p.stdout.on('data', (d) => out.push(d));
      p.on('error', reject);
      p.on('exit', (code) => {
        if (code !== 0) return reject(new ArchiveError('driver-exit', `probe failed ${code}`));
        resolve(Buffer.concat(out).toString('utf8').split('\n').filter((l) => l.length > 0));
      });
    });
  }

  return { sessionDriver, execSql, query, runScript };
}
