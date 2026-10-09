// FUNC-FIX-002 real-browser regression — shareholder default collection
// account (REQ: FUNC-FIX-002 closure evidence, preserved under M0-05).
//
// Real stack: PostgreSQL (disposable kooperatif_e2e_ff002) + Rust API +
// SvelteKit dev + Chromium. Every scenario asserts BOTH the UI behaviour
// and the persisted financial result in the database.
//
// Self-contained: prepares its own database (drop/create + migrations +
// admin user), picks free ports, cleans up on exit.
//
// Run: pnpm e2e:ff002   (requires the dev Docker postgres service)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import net from 'node:net';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const DB_NAME = 'kooperatif_e2e_ff002';
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

const log = (m) => console.log(`[ff002] ${m}`);
const results = [];
const check = (name, ok, detail = '') => {
	results.push({ name, ok, detail });
	console.log(`[ff002] ${ok ? 'PASS' : 'FAIL'} — ${name}${detail ? ` (${detail})` : ''}`);
	if (!ok) process.exitCode = 1;
};

const compose = (args, opts = {}) =>
	spawnSync('docker', ['compose', '-f', `${ROOT}/docker/compose.yaml`, ...args], {
		stdio: 'pipe',
		encoding: 'utf8',
		...opts
	});

const sql = (q) =>
	compose(['exec', '-T', 'postgres', 'psql', '-U', 'kooperatif', '-d', DB_NAME, '-t', '-A', '-F', '|', '-c', q])
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

// ---- disposable stack -------------------------------------------------
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
		'create-user', '--username', 'e2eadmin', '--display-name', 'E2E Yoneticisi', '--password-stdin'],
	{ env: serverEnv, input: 'e2e-ff002-parola', stdio: ['pipe', 'pipe', 'pipe'], encoding: 'utf8' }
);
if (res.status !== 0) throw new Error(`create-user: ${res.stderr}`);
res = cargo(['grant-role', '--username', 'e2eadmin', '--role', 'Sistem Yöneticisi']);
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
		data: { username: 'e2eadmin', password: 'e2e-ff002-parola' }
	});
	if (!loginResponse.ok()) throw new Error(`login: ${loginResponse.status()}`);
	const csrf = (await loginResponse.json()).csrfToken;
	const apiPost = async (path, body) => seed.request.post(`${API}${path}`, {
		headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf }, data: body
	});
	const apiGet = (path) => seed.request.get(`${API}${path}`, { headers: { origin: WEB } });

	// ---- fixtures ----------------------------------------------------
	const postJson = async (path, body) => {
		const r = await apiPost(path, body);
		if (!r.ok()) throw new Error(`${path}: ${r.status()} ${await r.text()}`);
		return r.json();
	};
	const mkAcc = async (name) =>
		(await postJson('/api/financial-accounts', { name, accountType: 'cash' })).id;
	const IST = await mkAcc('İstanbul TL');
	const KOY = await mkAcc('Köy TL');
	const PAS = await mkAcc('Eski Kasa');

	const mkSh = async (firstName, family, def) => {
		const r = await apiPost('/api/shareholders', {
			person: { mode: 'new', firstName, lastName: 'Soyadı' },
			family,
			defaultCollectionAccountId: def ?? undefined
		});
		if (!r.ok()) throw new Error(`shareholder ${firstName}: ${await r.text()}`);
		return (await r.json()).id;
	};
	const shA = await mkSh('FFAli', { mode: 'new', sequenceNumber: 501 }, KOY);
	const famA = sql(`SELECT id FROM families WHERE sequence_number=501`);
	const shB = await mkSh('FFAyşe', { mode: 'existing', familyId: famA }, IST);
	const shC = await mkSh('FFCan', { mode: 'new', sequenceNumber: 502 });
	const shD = await mkSh('FFDeniz', { mode: 'new', sequenceNumber: 503 }, PAS);

	// Deactivate D's default AFTER assignment (Owner decision 1).
	const accStatus = (await (await apiGet(`/api/financial-accounts/${PAS}`)).json()).status;
	if (accStatus !== 'inactive') {
		const ps = await apiPost(`/api/financial-accounts/${PAS}/status-change`, { status: 'inactive' });
		if (!ps.ok()) throw new Error(`deactivate: ${await ps.text()}`);
	}

	const mkPeriod = async (name, start, due, amt, eff) => {
		const p = await (await apiPost('/api/periods', {
			name, collectionStartDate: start, dueDate: due,
			ruleType: 'per_shareholder', baseAmount: amt, assessmentEffectiveDate: eff
		})).json();
		const g = await apiPost(`/api/periods/${p.id}/generate-assessments`);
		if (!g.ok()) throw new Error(`gen ${name}: ${await g.text()}`);
		return p;
	};
	await mkPeriod('FF002 Dönem', '2026-04-01', '2026-04-30', '250.00', '2026-04-01');
	await mkPeriod('FF002 Dönem-2', '2026-05-01', '2026-05-31', '300.00', '2026-05-01');
	// two more periods so Ali keeps open debt for S2 + S3 after S1/S6
	await mkPeriod('FF002 Dönem-3', '2026-06-01', '2026-06-30', '400.00', '2026-06-01');
	await mkPeriod('FF002 Dönem-4', '2026-06-01', '2026-06-30', '150.00', '2026-06-01');
	log('fixtures ready');

	// tr-TR input → canonical decimal string ('1.250,00' → '1250.00')
	const canon = (s) => s.replace(/\./g, '').replace(',', '.');
	// DB helpers
	const payDest = (pid) => sql(`SELECT destination_account_id FROM payments WHERE id='${pid}'`);
	const mvOf = (pid) => sql(`SELECT am.account_id, am.direction, am.amount, am.status FROM account_movements am WHERE am.source_type='payment' AND am.source_id='${pid}'`);
	const shDefault = (sid) => sql(`SELECT coalesce(default_collection_account_id::text,'') FROM shareholders WHERE id='${sid}'`);
	const balance = (acc) => sql(`SELECT coalesce(sum(case when direction='inflow' then amount else -amount end),0) FROM account_movements WHERE account_id='${acc}' AND status='active'`);

	// ---- browser ------------------------------------------------------
	const ctx = await browser.newContext({ locale: 'tr-TR', viewport: { width: 1440, height: 900 } });
	const page = await ctx.newPage();
	await page.goto(`${WEB}/login`, { waitUntil: 'domcontentloaded' });
	await page.waitForLoadState('networkidle');
	await page.fill('#username', 'e2eadmin');
	await page.fill('#password', 'e2e-ff002-parola');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL(`${WEB}/`, { timeout: 30000 });
	log('logged in');

	const accTrigger = page.locator('button[aria-labelledby="pay-account-label"]');
	const accText = async () => (await accTrigger.textContent()).trim();
	const pickAccount = async (name) => {
		await accTrigger.click();
		await page.getByRole('option', { name: new RegExp(name) }).click();
	};
	const openForm = async () => {
		await page.goto(`${WEB}/tahsilatlar/yeni`, { waitUntil: 'domcontentloaded' });
		await page.waitForSelector('#pay-amount');
	};
	const pickPayerNew = async (first, last) => {
		await page.getByRole('button', { name: 'Yeni kişi olarak kaydet' }).click();
		await page.fill('#payer-first', first);
		await page.fill('#payer-last', last);
	};
	const pickDebtor = async (q, pickText) => {
		const input = page.getByPlaceholder('Hissedar ara (ad, vasi, aile no)');
		await input.fill(q);
		await page.locator('form').filter({ has: input }).getByRole('button', { name: 'Ara' }).click();
		await page.getByRole('button', { name: new RegExp(pickText) }).click();
		await page.waitForSelector('text=Kalan', { timeout: 15000 });
	};
	const submit = async (amount) => {
		await page.fill('#pay-amount', amount);
		await page.getByRole('button', { name: 'Tahsilatı Kaydet' }).click();
		await page.waitForURL(/\/tahsilatlar\/[0-9a-f-]{36}/, { timeout: 30000 });
		return page.url().split('/').pop();
	};
	// check first row, return its prefilled allocation amount
	const firstRowAmount = async () => {
		await page.locator('tbody input[type=checkbox]').first().check();
		return page.locator('tbody input:not([type=checkbox])').first().inputValue();
	};

	// === S1: single shareholder + active default → proposed KOY ========
	await openForm();
	await pickPayerNew('FFPayer', 'Bir');
	await pickDebtor('FFAli', 'FFAli');
	const a1 = await firstRowAmount();
	const t1 = await accText();
	check('S1 UI: debtor default proposed', t1.includes('Köy TL'), t1);
	const hint1 = await page.locator('text=Borçlunun varsayılan kasası önerildi').isVisible();
	check('S1 UI: proposal hint shown', hint1);
	const p1 = await submit(a1);
	check('S1 DB: payment posted to Köy TL', payDest(p1) === KOY, `${payDest(p1)} == ${KOY}`);
	check('S1 DB: inflow movement on Köy TL', (await mvOf(p1)).startsWith(`${KOY}|inflow|${canon(a1)}`), mvOf(p1));

	// === S2: same debtor, operator switches to İstanbul TL =============
	await openForm();
	await pickPayerNew('FFPayer', 'İki');
	await pickDebtor('FFAli', 'FFAli');
	const a2 = await firstRowAmount();
	check('S2 UI: default proposed again', (await accText()).includes('Köy TL'));
	await pickAccount('İstanbul TL');
	check('S2 UI: manual override accepted', (await accText()).includes('İstanbul TL'));
	const p2 = await submit(a2);
	check('S2 DB: payment posted to İstanbul TL', payDest(p2) === IST, payDest(p2));
	check('S2 DB: shareholder default still Köy TL', shDefault(shA) === KOY, shDefault(shA));
	check('S2 DB: Köy TL balance only from S1', balance(KOY) === canon(a1), balance(KOY));

	// === S6: third-party payer pays Ali's debt → debtor default ========
	// (runs before S3 so Ali still has open debt; default still Köy TL)
	await openForm();
	await pickPayerNew('Üçüncü', 'Şahıs');
	await pickDebtor('FFAli', 'FFAli');
	const a6 = await firstRowAmount();
	check('S6 UI: debtor default proposed (not payer)', (await accText()).includes('Köy TL'), await accText());
	const p6 = await submit(a6);
	const payerId = sql(`SELECT payer_person_id FROM payments WHERE id='${p6}'`);
	const aliPerson = sql(`SELECT person_id FROM shareholders WHERE id='${shA}'`);
	check('S6 DB: payer is the third party', payerId !== aliPerson && payerId !== '', `${payerId} != ${aliPerson}`);
	check('S6 DB: posted to debtor default Köy TL', payDest(p6) === KOY);

	// === S3: family collection covering two shareholders ==============
	await openForm();
	await pickPayerNew('FFPayer', 'Aile');
	await page.getByRole('button', { name: 'Aileye göre' }).click();
	const famInput = page.getByPlaceholder('Aile ara (aile no, üye adı)');
	await famInput.fill('501');
	await page.locator('form').filter({ has: famInput }).getByRole('button', { name: 'Ara' }).click();
	await page.getByRole('button', { name: /501/ }).click();
	await page.waitForSelector('text=Üyelerin açık borçları');
	const memberBtns = page.getByRole('button', { name: 'Üyelerin açık borçları' });
	const mcount = await memberBtns.count();
	for (let i = 0; i < mcount; i++) {
		const b = memberBtns.nth(i);
		if (!(await b.isDisabled())) { await b.click(); await page.waitForTimeout(800); }
	}
	check('S3 UI: rows for two debtors', (await page.locator('table tbody tr').count()) >= 2);
	check('S3 UI: NO auto account selection', (await accText()) === '—', await accText());
	check('S3 UI: multi-debtor hint shown', await page.locator('text=Birden çok hissedarın borcunu').isVisible());
	await page.getByRole('button', { name: 'Tümünü seç' }).click();
	await pickAccount('Köy TL');
	const rem = sql(`SELECT coalesce(sum(a.amount - coalesce((SELECT sum(pa.amount) FROM payment_allocations pa WHERE pa.assessment_id=a.id AND pa.status='active'),0)),0) FROM assessments a JOIN shareholder_family_memberships m ON m.shareholder_id=a.shareholder_id AND m.ended_at IS NULL WHERE m.family_id='${famA}'`);
	const p3 = await submit(rem.replace('.', ','));
	check('S3 DB: payment posted to chosen account only', payDest(p3) === KOY, payDest(p3));
	check('S3 DB: single inflow movement on Köy TL', (await mvOf(p3)).split('\n').length === 1 && (await mvOf(p3)).includes(`${KOY}|inflow`), mvOf(p3));
	check('S3 DB: no transfers created', sql(`SELECT count(*) FROM account_transfers`) === '0');
	check('S3 DB: shareholder defaults untouched', shDefault(shA) === KOY && shDefault(shB) === IST);

	// === S4: inactive default → warning + manual pick ==================
	await openForm();
	await pickPayerNew('FFPayer', 'Dört');
	await pickDebtor('FFDeniz', 'FFDeniz');
	const a4 = await firstRowAmount();
	check('S4 UI: inactive warning shown', await page.locator('text=pasif durumda').isVisible());
	check('S4 UI: no auto-selection', (await accText()) === '—');
	// inactive account must not even be selectable
	await accTrigger.click();
	const hasInactive = await page.getByRole('option', { name: /Eski Kasa/ }).count();
	await page.keyboard.press('Escape');
	check('S4 UI: inactive account absent from options', hasInactive === 0);
	await pickAccount('Köy TL');
	const p4 = await submit(a4);
	check('S4 DB: posted to operator-chosen Köy TL', payDest(p4) === KOY);
	check('S4 DB: D default still stored (Eski Kasa)', shDefault(shD) === PAS, shDefault(shD));

	// === S5: no default → explicit selection required ==================
	await openForm();
	await pickPayerNew('FFPayer', 'Beş');
	await pickDebtor('FFCan', 'FFCan');
	const a5 = await firstRowAmount();
	check('S5 UI: no proposal/warning text', !(await page.locator('text=varsayılan kasası').isVisible().catch(() => false)));
	check('S5 UI: account empty', (await accText()) === '—');
	check('S5 UI: submit blocked without account', await page.getByRole('button', { name: 'Tahsilatı Kaydet' }).isDisabled());
	await pickAccount('Köy TL');
	const p5 = await submit(a5);
	check('S5 DB: posted to Köy TL', payDest(p5) === KOY);

	// === S7: change Ali default KOY→IST via UI; history unchanged ======
	const destBefore = sql(`SELECT id,destination_account_id FROM payments ORDER BY created_at`).split('\n').join(';');
	await page.goto(`${WEB}/hissedarlar/${shA}`, { waitUntil: 'domcontentloaded' });
	await page.waitForSelector('text=Varsayılan Kasa');
	await page.getByRole('button', { name: 'Bilgileri Düzenle' }).click();
	await page.locator('button[aria-labelledby="edit-default-account-label"]').click();
	await page.getByRole('option', { name: 'İstanbul TL' }).click();
	await page.getByRole('button', { name: 'Bilgileri Kaydet' }).click();
	await page.waitForTimeout(1500);
	check('S7 DB: default now İstanbul TL', shDefault(shA) === IST, shDefault(shA));
	const destAfter = sql(`SELECT id,destination_account_id FROM payments ORDER BY created_at`).split('\n').join(';');
	check('S7 DB: historical payments unchanged', destBefore === destAfter);
	check('S7 DB: audit event recorded', sql(`SELECT count(*) FROM security_events WHERE event_type='shareholder_default_account_changed'`) !== '0', sql(`SELECT event_type FROM security_events WHERE event_type LIKE '%default%'`));
	// UI shows the change persisted
	await page.reload({ waitUntil: 'domcontentloaded' });
	let s7ui = true;
	try { await page.waitForSelector('text=İstanbul TL', { timeout: 10000 }); } catch { s7ui = false; }
	check('S7 UI: detail shows İstanbul TL', s7ui);

	// === S8: reversal posts against the ORIGINAL receiving account =====
	const p1dest = payDest(p1);
	await page.goto(`${WEB}/tahsilatlar/${p1}`, { waitUntil: 'domcontentloaded' });
	await page.waitForSelector('text=Tahsilatı Geri Al', { timeout: 15000 });
	await page.getByRole('button', { name: 'Tahsilatı Geri Al' }).click();
	await page.fill('#payment-reason', 'e2e ters kayıt');
	await page.getByRole('button', { name: 'Tahsilatı Geri Al' }).click();
	await page.waitForTimeout(1500);
	await page.waitForTimeout(1500);
	check('S8 DB: payment reversed', sql(`SELECT status FROM payments WHERE id='${p1}'`) === 'reversed', sql(`SELECT status FROM payments WHERE id='${p1}'`));
	const mv = (await mvOf(p1)).split('\n');
	check('S8 DB: original movement marked reversed on original account', mv.every((l) => l.includes(p1dest)) && mv.every((l) => l.endsWith('reversed')), mv.join(';'));
	check('S8 DB: shareholder default unaffected by reversal', shDefault(shA) === IST);

	await browser.close();
	await shutdown();
	const failed = results.filter((r) => !r.ok).length;
	console.log(`[ff002] DONE — ${results.length - failed}/${results.length} checks passed`);
	process.exit(process.exitCode ?? (failed ? 1 : 0));
} catch (e) {
	await shutdown();
	console.error('[ff002] ERROR', e);
	process.exit(1);
}
