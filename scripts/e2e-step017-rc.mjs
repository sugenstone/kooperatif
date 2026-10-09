// STEP-017D — Release Candidate verification through the real stack
// (PostgreSQL + Rust API + SvelteKit + Playwright).
//
// Deterministic pilot fixture + journeys A–K:
//   A shareholder onboarding → B assessment/collection → C credit →
//   D accounts/transfer → E income/expense → F share return →
//   G investment → H social aid → I governance → J reporting parity →
//   K live TV — plus cross-browser smoke (Chromium + Firefox + WebKit)
// and DOM-level accessibility assertions.
//
// Run: node scripts/e2e-step017-rc.mjs   (requires Docker postgres up)
import { chromium, firefox, webkit } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8097';
const WEB = 'http://localhost:5197';
const DB_NAME = 'kooperatif_e2e_017rc';
const DB_URL = `postgres://kooperatif:kooperatif_dev_password@localhost:5494/${DB_NAME}`;

const log = (msg) => console.log(`[rc] ${msg}`);
const fail = (msg) => {
	console.error(`[rc] FAIL: ${msg}`);
	process.exitCode = 1;
};
const eq = (name, got, want) => {
	if (String(got) !== String(want)) fail(`${name}: expected ${want}, got ${got}`);
};

async function waitFor(url, timeoutMs = 90000) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		try {
			const response = await fetch(url);
			if (response.ok || response.status === 401 || response.status === 404) return true;
		} catch {}
		await sleep(500);
	}
	throw new Error(`timeout waiting for ${url}`);
}

const compose = (args) =>
	spawnSync('docker', ['compose', '-f', `${ROOT}/docker/compose.yaml`, ...args], {
		stdio: 'pipe',
		encoding: 'utf8'
	});

log('preparing database');
compose(['up', '-d', 'postgres']);
for (let i = 0; i < 30; i++) {
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
	KOOPERATIF_PORT: '8097',
	KOOPERATIF_DATABASE_URL: DB_URL,
	KOOPERATIF_CORS_ORIGINS: WEB,
	KOOPERATIF_ALLOWED_ORIGINS: WEB,
	KOOPERATIF_ARGON2_M_COST: '8192',
	KOOPERATIF_ARGON2_T_COST: '1',
	KOOPERATIF_ARGON2_P_COST: '1'
};
const cargo = (args) =>
	spawnSync(
		'cargo',
		['run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--', ...args],
		{ env: serverEnv, stdio: 'pipe', encoding: 'utf8' }
	);
let result = cargo(['migrate']);
if (result.status !== 0) fail(`migrate: ${result.stderr}`);
const { execFileSync } = await import('node:child_process');
try {
	execFileSync('cargo', [
		'run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--',
		'create-user', '--username', 'rcadmin', '--display-name', 'RC Yoneticisi', '--password-stdin'
	], { env: serverEnv, input: 'rc-parola-cok-gizli-1', stdio: ['pipe', 'pipe', 'pipe'] });
} catch (error) {
	fail(`create-user: ${error.stderr?.toString()}`);
}
result = cargo(['grant-role', '--username', 'rcadmin', '--role', 'Sistem Yöneticisi']);
if (result.status !== 0) fail(`grant-role: ${result.stderr}`);

log('starting backend + web');
const server = spawn(`cargo run --manifest-path "${ROOT}/apps/server/Cargo.toml" --quiet`, {
	env: serverEnv, stdio: 'pipe', shell: true
});
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port 5197 --strictPort`, {
	env: { ...process.env, PUBLIC_API_BASE_URL: API }, stdio: 'pipe', shell: true
});
server.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));
web.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));

const killTree = (child) => {
	if (!child?.pid) return;
	if (process.platform === 'win32') {
		spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'pipe' });
	} else child.kill('SIGTERM');
};
const shutdown = async () => {
	killTree(server);
	killTree(web);
	await sleep(1000);
};
process.on('SIGINT', async () => {
	await shutdown();
	process.exit(1);
});

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	const browser = await chromium.launch({ headless: true });
	const ctx = await browser.newContext({ locale: 'tr-TR' });
	const api = await ctx.newPage();
	const loginResponse = await api.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'rcadmin', password: 'rc-parola-cok-gizli-1' }
	});
	if (!loginResponse.ok()) throw new Error(`login: ${loginResponse.status()}`);
	const csrf = (await loginResponse.json()).csrfToken;
	const apiPost = async (path, body) => {
		const r = await api.request.post(`${API}${path}`, {
			headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
			data: body
		});
		return r;
	};
	const apiGet = async (path) => {
		const r = await api.request.get(`${API}${path}`, { headers: { origin: WEB } });
		if (!r.ok()) fail(`GET ${path} → ${r.status()}: ${await r.text()}`);
		return r.json();
	};
	const post = async (path, body, label) => {
		const r = await apiPost(path, body);
		if (!r.ok()) fail(`${label} → ${r.status()}: ${await r.text()}`);
		return r.json();
	};

	// ============ PILOT FIXTURE + Journey A — onboarding ================
	log('A: shareholder onboarding');
	const mkShareholder = async (first, last, seq, key) =>
		post('/api/shareholders', {
			person: { mode: 'new', firstName: first, lastName: last },
			family: { mode: 'new', sequenceNumber: seq }
		}, `shareholder ${key}`);
	// Duplicate person names must coexist (realistic pilot data).
	const sh1 = await mkShareholder('Ayşe', 'Yılmaz', 9001, 'rc-sh-1');
	const sh1dup = await mkShareholder('Ayşe', 'Yılmaz', 9002, 'rc-sh-2');
	const sh2 = await mkShareholder('Mehmet', 'Demir', 9003, 'rc-sh-3');
	const personOf = async (sh) => (await apiGet(`/api/shareholders/${sh.id}`)).personId;
	const share1 = await post('/api/shares', {
		shareholderId: sh1.id, acquisitionType: 'founder',
		effectiveAt: '2025-06-01T00:00:00Z'
	}, 'share1');
	await post('/api/shares', {
		shareholderId: sh2.id, acquisitionType: 'later_acquisition', acquisitionFee: '500.00',
		effectiveAt: '2025-06-01T00:00:00Z'
	}, 'share2');

	// ============ Journey B — period, assessments, payments ============
	log('B: period + assessments + payments');
	const period = await post('/api/periods', {
		name: 'RC Dönem 1', collectionStartDate: '2026-03-01', dueDate: '2026-03-31',
		ruleType: 'per_shareholder', baseAmount: '1000.00',
		assessmentEffectiveDate: '2026-03-01'
	}, 'period');
	const gen = await apiPost(`/api/periods/${period.id}/generate-assessments`);
	if (!gen.ok()) fail(`generate: ${await gen.text()}`);
	const assessments = (await apiGet(`/api/periods/${period.id}/assessments?pageSize=100`)).items;
	const a1 = assessments.find((a) => a.shareholder.shareholderId === sh1.id);
	const a2 = assessments.find((a) => a.shareholder.shareholderId === sh2.id);
	if (!a1 || !a2) fail('assessments missing for shareholders');

	const kasa = (await post('/api/financial-accounts', { name: 'RC Kasa', accountType: 'cash' }, 'kasa')).id;
	const banka = (await post('/api/financial-accounts', { name: 'RC Banka', accountType: 'bank' }, 'banka')).id;

	// Partial payment against a1 (400 of 1000).
	const p1 = await post('/api/payments', {
		payerFirstName: 'Veli', payerLastName: 'Ödeyen', amount: '400.00', method: 'cash',
		destinationAccountId: kasa,
		allocations: [{ assessmentId: a1.id, amount: '400.00' }],
		idempotencyKey: 'rc-pay-1'
	}, 'payment partial');
	// Full payment against a2 (1000).
	await post('/api/payments', {
		payerFirstName: 'Veli', payerLastName: 'Ödeyen', amount: '1000.00', method: 'bank_transfer',
		destinationAccountId: banka,
		allocations: [{ assessmentId: a2.id, amount: '1000.00' }],
		idempotencyKey: 'rc-pay-2'
	}, 'payment full');
	const reportRow = async (id) =>
		(await apiGet('/api/reports/assessments?pageSize=100')).items.find((r) => r.id === id);
	eq('a1 remaining after partial', (await reportRow(a1.id)).remainingAmount, '600.00');

	// ============ Journey C — overpayment → credit → apply ==============
	log('C: excess payment → shareholder credit → application');
	const p3 = await post('/api/payments', {
		payerFirstName: 'Veli', payerLastName: 'Ödeyen', amount: '700.00', method: 'cash',
		destinationAccountId: kasa,
		allocations: [{ assessmentId: a1.id, amount: '600.00' }],
		idempotencyKey: 'rc-pay-3'
	}, 'payment overpay');
	// Assign the 100 remainder to sh1 explicitly.
	await post(`/api/payments/${p3.payment.id}/credits`, {
		shareholderId: sh1.id, amount: '100.00', idempotencyKey: 'rc-credit-1'
	}, 'assign credit');
	// Second period for the same shareholder → apply the credit.
	const period2 = await post('/api/periods', {
		name: 'RC Dönem 2', collectionStartDate: '2026-04-01', dueDate: '2026-04-30',
		ruleType: 'per_shareholder', baseAmount: '100.00',
		assessmentEffectiveDate: '2026-04-01'
	}, 'period2');
	const gen2 = await apiPost(`/api/periods/${period2.id}/generate-assessments`);
	if (!gen2.ok()) fail(`generate2: ${await gen2.text()}`);
	const assessments2 = (await apiGet(`/api/periods/${period2.id}/assessments?pageSize=100`)).items;
	const a1b = assessments2.find((a) => a.shareholder.shareholderId === sh1.id);
	const movesBefore = (await apiGet(`/api/financial-accounts/${kasa}/movements?pageSize=100`)).items.length;
	// Held credit auto-offsets the newly generated assessment (STEP-009):
	// the application is created by generation itself and moves no money.
	eq('a1b auto-settled by held credit', (await reportRow(a1b.id)).remainingAmount, '0.00');
	const movesAfter = (await apiGet(`/api/financial-accounts/${kasa}/movements?pageSize=100`)).items.length;
	eq('credit application moves no money', movesAfter, movesBefore);
	const apps = await apiGet(`/api/assessments/${a1b.id}/credit-applications`);
	if (!apps.length) fail('no credit application recorded for a1b');
	// A manual duplicate application on the settled debt is rejected.
	const dup = await apiPost(`/api/assessments/${a1b.id}/credit-applications`, {
		amount: '100.00', idempotencyKey: 'rc-capply-dup'
	});
	if (dup.ok()) fail('duplicate credit application accepted');

	// ============ Journey D — accounts + transfer =======================
	log('D: transfers between accounts');
	await post('/api/account-transfers', {
		sourceAccountId: kasa, destinationAccountId: banka, amount: '150.00',
		idempotencyKey: 'rc-tr-1'
	}, 'transfer');
	eq('kasa balance', (await apiGet(`/api/financial-accounts/${kasa}`)).balance, '950.00');
	eq('banka balance', (await apiGet(`/api/financial-accounts/${banka}`)).balance, '1150.00');

	// ============ Journey E — income/expense ============================
	log('E: operational income/expense');
	const incCat = (await apiGet('/api/financial-categories/options?categoryType=income')).find((c) => c.name === 'Diğer Gelir');
	const expCat = (await apiGet('/api/financial-categories/options?categoryType=expense'))[0];
	await post('/api/incomes', {
		financialAccountId: kasa, categoryId: incCat.id, amount: '200.00',
		description: 'RC aidat dışı gelir', idempotencyKey: 'rc-inc-1'
	}, 'income');
	await post('/api/expenses', {
		financialAccountId: banka, categoryId: expCat.id, amount: '50.00',
		description: 'RC kırtasiye', idempotencyKey: 'rc-exp-1'
	}, 'expense');
	eq('kasa after income', (await apiGet(`/api/financial-accounts/${kasa}`)).balance, '1150.00');
	eq('banka after expense', (await apiGet(`/api/financial-accounts/${banka}`)).balance, '1100.00');

	// ============ Journey F — share return ==============================
	log('F: share return → entitlement → settlement');
	const ret = await post('/api/share-returns', {
		shareId: share1.id, effectiveReturnDate: '2026-03-15', idempotencyKey: 'rc-ret-1'
	}, 'initiate return');
	const retDetail = await apiGet(`/api/share-returns/${ret.id}`);
	await post(`/api/share-returns/${ret.id}/finalize`, {
		entitlements: [{ entitlementType: 'principal', amount: '300.00' }],
		expectedUpdatedAt: retDetail.updatedAt
	}, 'finalize return');
	const retEnts = (await apiGet(`/api/share-returns/${ret.id}`)).entitlements;
	const principal = retEnts.find((e) => e.entitlementType === 'principal');
	await post(`/api/share-return-entitlements/${principal.id}/settlements`, {
		financialAccountId: kasa, amount: '300.00', idempotencyKey: 'rc-settle-1'
	}, 'settlement');
	eq('kasa after settlement', (await apiGet(`/api/financial-accounts/${kasa}`)).balance, '850.00');
	// The share left the ACTIVE pool; entitlement spent.
	eq('share1 status', (await apiGet(`/api/shares/${share1.id}`)).status, 'closed');

	// ============ Journey G — investment ================================
	log('G: investment fund → valuation → income');
	const inv = await post('/api/investments', {
		name: 'RC Depo', investmentType: 'real_estate', idempotencyKey: 'rc-inv-1'
	}, 'investment');
	await post(`/api/investments/${inv.id}/fundings`, {
		financialAccountId: kasa, amount: '500.00',
		occurredAt: '2026-03-20T10:00:00Z', idempotencyKey: 'rc-fund-1'
	}, 'funding');
	const invMoves = (await apiGet(`/api/financial-accounts/${kasa}/movements?pageSize=100`)).items.length;
	await post(`/api/investments/${inv.id}/valuations`, {
		valuationDate: '2026-03-25', amount: '700.00', method: 'Emsal', idempotencyKey: 'rc-val-1'
	}, 'valuation');
	eq('valuation is noncash', (await apiGet(`/api/financial-accounts/${kasa}/movements?pageSize=100`)).items.length, invMoves);
	await post(`/api/investments/${inv.id}/incomes`, {
		financialAccountId: kasa, amount: '25.00', occurredAt: '2026-03-26T10:00:00Z',
		description: 'RC kira', idempotencyKey: 'rc-ivinc-1'
	}, 'investment income');
	eq('kasa after investment flows', (await apiGet(`/api/financial-accounts/${kasa}`)).balance, '375.00');

	// ============ Journey H — social aid ================================
	log('H: social aid restricted balances');
	const fund = await post('/api/social-aid/funds', {
		name: 'RC Yardım Fonu', idempotencyKey: 'rc-fund-aid-1'
	}, 'fund');
	await post('/api/social-aid/donations', {
		fundId: fund.id, donorDisplayName: 'RC Bağışçı', financialAccountId: kasa,
		amount: '400.00', occurredAt: '2026-03-27T10:00:00Z', idempotencyKey: 'rc-don-1'
	}, 'donation');
	await post('/api/social-aid/disbursements', {
		fundId: fund.id, beneficiaryDisplayName: 'RC Aile', financialAccountId: kasa,
		amount: '150.00', occurredAt: '2026-03-28T10:00:00Z',
		reason: 'Gıda desteği', idempotencyKey: 'rc-dis-1'
	}, 'disbursement');
	const fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	eq('restricted available', fundDetail.available, '250.00');
	eq('kasa after aid', (await apiGet(`/api/financial-accounts/${kasa}`)).balance, '625.00');

	// ============ Journey I — governance ================================
	log('I: governance evidence, zero money');
	const body = await post('/api/governance/bodies', {
		name: 'RC Yönetim Kurulu', bodyType: 'board', idempotencyKey: 'rc-body-1'
	}, 'body');
	const mem = await post(`/api/governance/bodies/${body.id}/memberships`, {
		personId: await personOf(sh2), startedAt: '2026-01-01T00:00:00Z', idempotencyKey: 'rc-mem-1'
	}, 'membership');
	const decision = await post('/api/governance/decisions', {
		bodyId: body.id, title: 'RC Karar', decisionText: 'RC karar metni',
		decisionOn: '2026-03-29', idempotencyKey: 'rc-dec-1'
	}, 'decision');
	const open = await apiPost(`/api/governance/decisions/${decision.id}/open`, {});
	if (!open.ok()) fail(`open decision: ${await open.text()}`);
	const vote = await apiPost(`/api/governance/decisions/${decision.id}/votes`, {
		personId: await personOf(sh2), choice: 'approve', idempotencyKey: 'rc-vote-1'
	});
	if (!vote.ok()) fail(`vote: ${await vote.text()}`);
	const fin = await apiPost(`/api/governance/decisions/${decision.id}/finalize`, { outcome: 'approved' });
	if (!fin.ok()) fail(`finalize decision: ${await fin.text()}`);
	const decDetail = await apiGet(`/api/governance/decisions/${decision.id}`);
	eq('decision status', decDetail.status, 'approved');

	// ============ Journey J — canonical reporting parity ================
	log('J: reports overview parity');
	const ov = await apiGet('/api/reports/overview');
	eq('overview cash', ov.financialAccountsBalance, '1725.00'); // 625 kasa + 1100 banka
	eq('overview assessed', ov.assessmentsTotal, '3300.00'); // 3 shareholders: 3x1000 + 3x100
	eq('overview outstanding debt', ov.outstandingAssessmentDebt, '1200.00'); // sh1dup dönem1 (1000) + dönem2 open (2x100)
	eq('overview payments total', ov.postedPaymentsTotal, '2100.00');
	eq('overview credit', ov.availableShareholderCredit, '0.00');
	eq('overview income', ov.operationalIncomeTotal, '200.00');
	eq('overview expense', ov.operationalExpenseTotal, '50.00');
	eq('overview investments funded', ov.investmentTotalFunded, '500.00');
	eq('overview investment income', ov.investmentIncomeTotal, '25.00');
	eq('overview aid restricted', ov.socialAidRestrictedAvailable, '250.00');
	eq('overview active shareholders', ov.activeShareholderCount, '3');
	eq('overview active shares', ov.activeShareCount, '1'); // share1 closed via return
	eq('overview decided', ov.decisionsApproved, '1');

	// ============ Journey K — live TV (admin has reports.read) =========
	log('K: live TV display');
	await api.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await api.getByLabel('Kullanıcı adı').fill('rcadmin');
	await api.getByLabel('Parola').fill('rc-parola-cok-gizli-1');
	await api.getByRole('button', { name: 'Giriş Yap' }).click();
	await api.waitForURL(`${WEB}/`, { timeout: 30000 });
	await api.goto(`${WEB}/canli-ekran`, { waitUntil: 'domcontentloaded' });
	await api.waitForSelector('[data-testid="tv-display"]', { timeout: 30000 });
	await api.waitForFunction(
		() => document.querySelector('[data-testid="tv-status"]')?.textContent?.trim() === 'Bağlı',
		null, { timeout: 30000 }
	);
	const tvCash = (await api.textContent('[data-testid="tv-cash"]'))?.trim();
	if (tvCash && !tvCash.includes('1.725')) fail(`TV cash shows ${tvCash}, expected 1.725,00`);

	// ============ Browser journey — UI navigation =======================
	log('UI: navigation + list → detail drill');
	await api.goto(`${WEB}/hissedarlar`, { waitUntil: 'networkidle' });
	await api.getByRole('link', { name: /Ayşe Yılmaz/ }).first().click();
	await api.waitForURL(/\/hissedarlar\//);
	const h1 = await api.locator('h1').first().textContent();
	if (!h1?.includes('Ayşe')) fail(`shareholder detail h1: ${h1}`);

	// ============ Accessibility assertions ==============================
	log('a11y: labels, aria, keyboard');
	// Login form labels are programmatically associated.
	const anon = await browser.newContext({ locale: 'tr-TR' });
	const lp = await anon.newPage();
	await lp.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await lp.getByLabel('Kullanıcı adı').fill('x');
	await lp.getByLabel('Parola').fill('y');
	// Skip link exists and becomes focusable.
	const skip = lp.locator('a[href="#main-content"]');
	if (!(await skip.count())) fail('skip link missing');
	// Keyboard: tab order reaches the submit button without traps.
	let focused = '';
	for (let i = 0; i < 8; i++) {
		await lp.keyboard.press('Tab');
		focused = await lp.evaluate(
			() => `${document.activeElement?.tagName}:${document.activeElement?.getAttribute('type') ?? ''}`
		);
		if (focused === 'BUTTON:submit') break;
	}
	if (focused !== 'BUTTON:submit') fail(`tab order never reached submit, last=${focused}`);
	await anon.close();

	// Mobile menu exposes aria-expanded.
	const mctx = await browser.newContext({ locale: 'tr-TR', viewport: { width: 390, height: 844 } });
	const mp = await mctx.newPage();
	await mp.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await mp.getByLabel('Kullanıcı adı').fill('rcadmin');
	await mp.getByLabel('Parola').fill('rc-parola-cok-gizli-1');
	await mp.getByRole('button', { name: 'Giriş Yap' }).click();
	await mp.waitForURL(`${WEB}/`, { timeout: 30000 });
	const menuBtn = mp.getByRole('button', { name: 'Menü' });
	if ((await menuBtn.getAttribute('aria-expanded')) !== 'false') fail('menu aria-expanded initial');
	await menuBtn.click();
	// shadcn sidebar opens an off-canvas dialog on mobile.
	const sheet = mp.getByRole('dialog');
	await sheet.waitFor({ state: 'visible' });
	if ((await menuBtn.getAttribute('aria-expanded')) !== 'true')
		fail('menu aria-expanded after open');
	// Sheet closes via Escape and the trigger reflects it again.
	await mp.keyboard.press('Escape');
	await sheet.waitFor({ state: 'hidden' });
	if ((await menuBtn.getAttribute('aria-expanded')) !== 'false')
		fail('menu aria-expanded after close');
	// Active-route marking on desktop.
	await mctx.close();
	const dctx = await browser.newContext({ locale: 'tr-TR', viewport: { width: 1440, height: 900 } });
	const dp = await dctx.newPage();
	await dp.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await dp.getByLabel('Kullanıcı adı').fill('rcadmin');
	await dp.getByLabel('Parola').fill('rc-parola-cok-gizli-1');
	await dp.getByRole('button', { name: 'Giriş Yap' }).click();
	await dp.waitForURL(`${WEB}/`, { timeout: 30000 });
	const current = dp.locator('[data-slot="sidebar"] a[aria-current="page"]');
	if (!(await current.count())) fail('no aria-current nav link on desktop');
	await dctx.close();

	// ============ Cross-browser smoke ==================================
	for (const [name, engine] of [['firefox', firefox], ['webkit', webkit]]) {
		try {
			log(`cross-browser: ${name}`);
			const b = await engine.launch({ headless: true });
			const c = await b.newContext({ locale: 'tr-TR' });
			const p = await c.newPage();
			await p.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
			await p.getByLabel('Kullanıcı adı').fill('rcadmin');
			await p.getByLabel('Parola').fill('rc-parola-cok-gizli-1');
			await p.getByRole('button', { name: 'Giriş Yap' }).click();
			await p.waitForURL(`${WEB}/`, { timeout: 30000 });
			await p.goto(`${WEB}/hissedarlar`, { waitUntil: 'networkidle' });
			if (!(await p.locator('[data-slot="sidebar"]').count()))
				fail(`${name}: sidebar nav missing`);
			const rows = await p.locator('table tbody tr').count();
			if (rows < 2) fail(`${name}: shareholder list rendered ${rows} rows`);
			await b.close();
		} catch (error) {
			fail(`${name} smoke: ${error.message}`);
		}
	}

	await browser.close();
	await shutdown();
	if (process.exitCode) console.log('[rc] FAILURES — see above');
	else console.log('[rc] PASS — journeys A–K + cross-browser + a11y');
} catch (error) {
	await shutdown();
	console.error('[rc] ERROR', error);
	process.exit(1);
}
