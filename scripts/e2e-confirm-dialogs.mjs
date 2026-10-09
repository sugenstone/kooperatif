// REQ-027 / M0-02 real-browser E2E — application confirmation dialogs.
//
// Representative coverage: one destructive/financial operation per page
// that previously used native window.confirm. For every operation the
// script proves both branches against the REAL stack + database:
//   * "Vazgeç" (cancel) → dialog closes, NO mutation persists;
//   * "Onayla" (confirm) → the intended mutation persists.
//
// Pages covered (1 op each, 6/6 pages):
//   donemler  — Dönemi Sil      (DELETE /api/periods/{id})
//   hissedarlar — Kaydı İptal Et (POST status-change → voided)
//   hisseler  — Hisseyi İptal Et (POST status-change → voided)
//   sosyal-yardim — Fonu İptal Et (POST cancel, reason required)
//   yatirimlar — Yatırımı İptal Et (POST cancel, reason required)
//   hisse-iadeleri — Talebi İptal Et (POST cancel, reason required)
//
// Vitest specs cover the remaining 14 operations' interaction contract
// (dialog opens, cancel is a no-op, confirm issues the request).
//
// Self-contained: prepares its own disposable database, picks free ports.
// Run: pnpm e2e:dialogs   (requires the dev Docker postgres service)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import net from 'node:net';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const DB_NAME = 'kooperatif_e2e_dialogs';
const DB_URL = `postgres://kooperatif:kooperatif_dev_password@localhost:5494/${DB_NAME}`;

const freePort = () =>
	new Promise((resolve, reject) => {
		const s = net.createServer().listen(0, '127.0.0.1', () => {
			const p = s.address().port;
			s.close(() => resolve(p));
		});
		s.on('error', reject);
	});
const API_PORT = await freePort();
const WEB_PORT = await freePort();
const API = `http://localhost:${API_PORT}`;
const WEB = `http://localhost:${WEB_PORT}`;

const log = (m) => console.log(`[dialogs] ${m}`);
const results = [];
const check = (name, ok, detail = '') => {
	results.push({ name, ok });
	console.log(`[dialogs] ${ok ? 'PASS' : 'FAIL'} — ${name}${detail ? ` (${detail})` : ''}`);
	if (!ok) process.exitCode = 1;
};

const compose = (args) =>
	spawnSync('docker', ['compose', '-f', `${ROOT}/docker/compose.yaml`, ...args], {
		stdio: 'pipe',
		encoding: 'utf8'
	});
const sql = (q) =>
	compose(['exec', '-T', 'postgres', 'psql', '-U', 'kooperatif', '-d', DB_NAME, '-t', '-A', '-c', q])
		.stdout.trim();

async function waitFor(url, timeoutMs = 120000) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		try {
			const r = await fetch(url);
			if (r.ok || r.status === 401 || r.status === 404) return;
		} catch {}
		await sleep(500);
	}
	throw new Error(`timeout ${url}`);
}

log('preparing database');
compose(['up', '-d', 'postgres']);
for (let i = 0; i < 60; i++) {
	if (compose(['exec', '-T', 'postgres', 'pg_isready', '-U', 'kooperatif']).status === 0) break;
	await sleep(1000);
}
compose([
	'exec', '-T', 'postgres', 'psql', '-U', 'kooperatif', '-d', 'postgres',
	'-c', `DROP DATABASE IF EXISTS ${DB_NAME} WITH (FORCE);`,
	'-c', `CREATE DATABASE ${DB_NAME};`
]);

const serverEnv = {
	...process.env,
	KOOPERATIF_ENV: 'development',
	KOOPERATIF_HOST: '127.0.0.1',
	KOOPERATIF_PORT: String(API_PORT),
	KOOPERATIF_DATABASE_URL: DB_URL,
	KOOPERATIF_CORS_ORIGINS: WEB,
	KOOPERATIF_ALLOWED_ORIGINS: WEB,
	KOOPERATIF_ARGON2_M_COST: '8192',
	KOOPERATIF_ARGON2_T_COST: '1',
	KOOPERATIF_ARGON2_P_COST: '1'
};
const cargo = (args, opts = {}) =>
	spawnSync(
		'cargo',
		['run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--', ...args],
		{ env: serverEnv, stdio: 'pipe', encoding: 'utf8', ...opts }
	);
let res = cargo(['migrate']);
if (res.status !== 0) throw new Error(`migrate: ${res.stderr}`);
res = spawnSync(
	'cargo',
	['run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--',
		'create-user', '--username', 'dlgadmin', '--display-name', 'Dialog Admin', '--password-stdin'],
	{ env: serverEnv, input: 'dlg-parola-123', stdio: ['pipe', 'pipe', 'pipe'], encoding: 'utf8' }
);
if (res.status !== 0) throw new Error(`create-user: ${res.stderr}`);
res = cargo(['grant-role', '--username', 'dlgadmin', '--role', 'Sistem Yöneticisi']);
if (res.status !== 0) throw new Error(`grant-role: ${res.stderr}`);

log(`starting backend :${API_PORT} + web :${WEB_PORT}`);
const server = spawn(`cargo run --manifest-path "${ROOT}/apps/server/Cargo.toml" --quiet`, {
	env: serverEnv, stdio: 'pipe', shell: true
});
server.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port ${WEB_PORT} --strictPort`, {
	env: { ...process.env, PUBLIC_API_BASE_URL: API }, stdio: 'pipe', shell: true
});
web.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));

const killTree = (c) => {
	if (!c?.pid) return;
	if (process.platform === 'win32') {
		spawnSync('taskkill', ['/pid', String(c.pid), '/T', '/F'], { stdio: 'pipe' });
	} else c.kill('SIGTERM');
};
const shutdown = async () => {
	killTree(server);
	killTree(web);
	await sleep(1000);
	compose(['exec', '-T', 'postgres', 'psql', '-U', 'kooperatif', '-d', 'postgres',
		'-c', `DROP DATABASE IF EXISTS ${DB_NAME} WITH (FORCE);`]);
};
process.on('SIGINT', async () => { await shutdown(); process.exit(1); });

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	const browser = await chromium.launch({ headless: true });
	const seedCtx = await browser.newContext({ locale: 'tr-TR' });
	const seed = await seedCtx.newPage();
	const loginResponse = await seed.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'dlgadmin', password: 'dlg-parola-123' }
	});
	if (!loginResponse.ok()) throw new Error(`login: ${loginResponse.status()}`);
	const csrf = (await loginResponse.json()).csrfToken;
	const apiPost = async (path, body) => seed.request.post(`${API}${path}`, {
		headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
		data: body ?? {}
	});

	// ---- API fixtures ----------------------------------------------------
	const postJson = async (path, body) => {
		const r = await apiPost(path, body);
		if (!r.ok()) throw new Error(`${path}: ${r.status()} ${await r.text()}`);
		return r.json();
	};
	const mkShareholder = async (firstName, seq) =>
		(
			await postJson('/api/shareholders', {
				person: { mode: 'new', firstName, lastName: 'Dlg' },
				family: { mode: 'new', sequenceNumber: seq }
			})
		).id;
	const shA = await mkShareholder('DlgAli', 900);
	// P2 voids this one; shA must stay active for the share/return ops.
	const shVoid = await mkShareholder('DlgVeli', 901);
	const mkShare = async (shareholderId) =>
		(
			await postJson('/api/shares', {
				shareholderId,
				acquisitionType: 'founder',
				effectiveAt: '2026-01-01T00:00:00Z'
			})
		).id;
	const shareA = await mkShare(shA);
	const period = (
		await (
			await apiPost('/api/periods', {
				name: 'Dlg Dönem', collectionStartDate: '2026-07-01', dueDate: '2026-07-31',
				ruleType: 'per_shareholder', baseAmount: '100.00', assessmentEffectiveDate: '2026-07-01'
			})
		).json()
	).id;
	const fund = (
		await (
			await apiPost('/api/social-aid/funds', {
				name: 'Dlg Fon', idempotencyKey: 'dlg-fund-1'
			})
		).json()
	).id;
	const investment = (
		await (
			await apiPost('/api/investments', {
				name: 'Dlg Yatırım', investmentType: 'real_estate', idempotencyKey: 'dlg-inv-1'
			})
		).json()
	).id;
	// A separate share owns the return: initiating it moves that share to
	// return_pending, which must not hide the void button on shareA.
	const shareB = await mkShare(shA);
	const retRes = await apiPost('/api/share-returns', {
		shareId: shareB,
		effectiveReturnDate: new Date().toISOString().slice(0, 10),
		idempotencyKey: 'dlg-ret-1'
	});
	if (!retRes.ok()) throw new Error(`share-return fixture: ${retRes.status()} ${await retRes.text()}`);
	const shareReturn = (await retRes.json()).id;
	log(`fixtures: period=${period} sh=${shA} share=${shareA} fund=${fund} inv=${investment} ret=${shareReturn}`);

	// ---- browser ----------------------------------------------------------
	const ctx = await browser.newContext({ locale: 'tr-TR', viewport: { width: 1440, height: 900 } });
	const page = await ctx.newPage();
	await page.goto(`${WEB}/login`, { waitUntil: 'domcontentloaded' });
	await page.waitForLoadState('networkidle');
	await page.fill('#username', 'dlgadmin');
	await page.fill('#password', 'dlg-parola-123');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL(`${WEB}/`, { timeout: 30000 });
	log('logged in');

	const dialog = () => page.getByRole('alertdialog');
	// cancel via the dialog; returns true when the dialog appeared
	const cancelDialog = async () => {
		const d = dialog();
		try {
			await d.waitFor({ state: 'visible', timeout: 8000 });
		} catch {
			return false;
		}
		await d.getByRole('button', { name: 'Vazgeç' }).click();
		await d.waitFor({ state: 'hidden', timeout: 8000 }).catch(() => {});
		return true;
	};
	const confirmDialog = async () => {
		const d = dialog();
		await d.waitFor({ state: 'visible', timeout: 8000 });
		await d.getByRole('button', { name: 'Onayla' }).click();
	};

	// === 1. donemler — Dönemi Sil =========================================
	await page.goto(`${WEB}/donemler/${period}`, { waitUntil: 'domcontentloaded' });
	const delBtn = () => page.getByRole('button', { name: 'Dönemi Sil' });
	await delBtn().click();
	check('P1 cancel: dialog opens', await cancelDialog());
	check('P1 cancel: period row still exists', sql(`SELECT count(*) FROM periods WHERE id='${period}'`) === '1');
	await delBtn().click();
	await confirmDialog();
	await page.waitForURL(`${WEB}/donemler`, { timeout: 15000 });
	check('P1 confirm: period deleted', sql(`SELECT count(*) FROM periods WHERE id='${period}'`) === '0');

	// === 2. hissedarlar — Kaydı İptal Et ===================================
	await page.goto(`${WEB}/hissedarlar/${shVoid}`, { waitUntil: 'domcontentloaded' });
	const voidSh = () => page.getByRole('button', { name: 'Kaydı İptal Et' });
	await voidSh().click();
	check('P2 cancel: dialog opens', await cancelDialog());
	check('P2 cancel: shareholder still active',
		sql(`SELECT status FROM shareholders WHERE id='${shVoid}'`) === 'active',
		sql(`SELECT status FROM shareholders WHERE id='${shVoid}'`));
	await voidSh().click();
	await confirmDialog();
	await page.waitForTimeout(1200);
	check('P2 confirm: shareholder voided',
		sql(`SELECT status FROM shareholders WHERE id='${shVoid}'`) === 'voided',
		sql(`SELECT status FROM shareholders WHERE id='${shVoid}'`));

	// === 3. hisseler — Hisseyi İptal Et ====================================
	await page.goto(`${WEB}/hisseler/${shareA}`, { waitUntil: 'domcontentloaded' });
	const voidShare = () => page.getByRole('button', { name: 'Hisseyi İptal Et' });
	await voidShare().click();
	check('P3 cancel: dialog opens', await cancelDialog());
	check('P3 cancel: share still active',
		sql(`SELECT status FROM shares WHERE id='${shareA}'`) === 'active',
		sql(`SELECT status FROM shares WHERE id='${shareA}'`));
	await voidShare().click();
	await confirmDialog();
	await page.waitForTimeout(1200);
	check('P3 confirm: share voided',
		sql(`SELECT status FROM shares WHERE id='${shareA}'`) === 'voided',
		sql(`SELECT status FROM shares WHERE id='${shareA}'`));

	// === 4. sosyal-yardim — Fonu İptal Et =================================
	await page.goto(`${WEB}/sosyal-yardim/${fund}`, { waitUntil: 'domcontentloaded' });
	// ghost button opens the cancel panel; the panel's own button opens the dialog
	await page.getByRole('button', { name: 'Fonu İptal Et' }).first().click();
	await page.fill('#cancel-reason', 'e2e iptal');
	const fundCancelSubmit = () => page.locator('#cancel-reason').locator('..').locator('..').getByRole('button', { name: 'Fonu İptal Et' });
	await fundCancelSubmit().click();
	check('P4 cancel: dialog opens', await cancelDialog());
	check('P4 cancel: fund still active',
		sql(`SELECT status FROM social_aid_funds WHERE id='${fund}'`) === 'active',
		sql(`SELECT status FROM social_aid_funds WHERE id='${fund}'`));
	await fundCancelSubmit().click();
	await confirmDialog();
	await page.waitForTimeout(1200);
	check('P4 confirm: fund cancelled',
		sql(`SELECT status FROM social_aid_funds WHERE id='${fund}'`) === 'cancelled',
		sql(`SELECT status FROM social_aid_funds WHERE id='${fund}'`));

	// === 5. yatirimlar — Yatırımı İptal Et ================================
	await page.goto(`${WEB}/yatirimlar/${investment}`, { waitUntil: 'domcontentloaded' });
	await page.getByRole('button', { name: 'Yatırımı İptal Et' }).first().click();
	await page.fill('#cancel-reason', 'e2e iptal');
	const invCancelSubmit = () => page.locator('#cancel-reason').locator('..').locator('..').getByRole('button', { name: 'Yatırımı İptal Et' });
	await invCancelSubmit().click();
	check('P5 cancel: dialog opens', await cancelDialog());
	check('P5 cancel: investment still active',
		sql(`SELECT status FROM investments WHERE id='${investment}'`) === 'active',
		sql(`SELECT status FROM investments WHERE id='${investment}'`));
	await invCancelSubmit().click();
	await confirmDialog();
	await page.waitForTimeout(1200);
	check('P5 confirm: investment cancelled',
		sql(`SELECT status FROM investments WHERE id='${investment}'`) === 'cancelled',
		sql(`SELECT status FROM investments WHERE id='${investment}'`));

	// === 6. hisse-iadeleri — Talebi İptal Et ==============================
	await page.goto(`${WEB}/hisse-iadeleri/${shareReturn}`, { waitUntil: 'domcontentloaded' });
	await page.getByRole('button', { name: 'Talebi İptal Et' }).first().click();
	await page.fill('#cancel-reason', 'e2e iptal');
	const retCancelSubmit = () => page.locator('#cancel-reason').locator('..').locator('..').getByRole('button', { name: 'Talebi İptal Et' });
	await retCancelSubmit().click();
	check('P6 cancel: dialog opens', await cancelDialog());
	check('P6 cancel: return still pending',
		sql(`SELECT status FROM share_returns WHERE id='${shareReturn}'`) !== 'cancelled',
		sql(`SELECT status FROM share_returns WHERE id='${shareReturn}'`));
	await retCancelSubmit().click();
	await confirmDialog();
	await page.waitForTimeout(1200);
	check('P6 confirm: return cancelled',
		sql(`SELECT status FROM share_returns WHERE id='${shareReturn}'`) === 'cancelled',
		sql(`SELECT status FROM share_returns WHERE id='${shareReturn}'`));

	await browser.close();
	await shutdown();
	const failed = results.filter((r) => !r.ok).length;
	console.log(`[dialogs] DONE — ${results.length - failed}/${results.length} checks passed`);
	process.exit(process.exitCode ?? (failed ? 1 : 0));
} catch (e) {
	await shutdown();
	console.error('[dialogs] ERROR', e);
	process.exit(1);
}
