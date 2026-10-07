// STEP-011 focused E2E: Share Return / Entitlement / Settlement through
// the real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture shareholder + backdated share + funded cash
//   account → UI creates a return request (share → "İade Bekliyor",
//   ZERO money moves) → UI finalizes with a principal entitlement +
//   undetermined profit right (share → "Kapandı", ownership closed,
//   STILL zero settlement movements) → UI partially settles from the
//   account (exactly ONE share_return_settlement outflow; not income /
//   expense / payment / transfer; summary untouched) → UI reverses the
//   settlement (history preserved, remaining restored).
//
// Boundaries asserted end-to-end:
//   - return recognition moves NO money; settlement moves exactly ONE;
//   - settlement is never Expense/Payment/Transfer and creates no credit;
//   - principal ≠ profit; NULL amount renders as "belirlenmedi";
//   - closing one share leaves the shareholder's other share active.
// Run: node scripts/e2e-step011.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_011';

const log = (msg) => console.log(`[e2e] ${msg}`);
const fail = (msg) => {
	console.error(`[e2e] FAIL: ${msg}`);
	process.exitCode = 1;
};

async function waitFor(url, timeoutMs = 60000) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		try {
			const response = await fetch(url);
			if (response.ok || response.status === 401) return true;
		} catch {}
		await sleep(500);
	}
	throw new Error(`timeout waiting for ${url}`);
}

// --- 1. Reset the E2E database and bootstrap the admin -----------------
log('preparing database');
spawnSync('docker', ['compose', '-f', `${ROOT}/docker/compose.yaml`, 'up', '-d', 'postgres'], {
	stdio: 'pipe'
});
for (let i = 0; i < 30; i++) {
	const ready = spawnSync(
		'docker',
		[
			'compose',
			'-f',
			`${ROOT}/docker/compose.yaml`,
			'exec',
			'-T',
			'postgres',
			'pg_isready',
			'-U',
			'kooperatif'
		],
		{ stdio: 'pipe' }
	);
	if (ready.status === 0) break;
	await sleep(1000);
}
spawnSync(
	'docker',
	[
		'compose',
		'-f',
		`${ROOT}/docker/compose.yaml`,
		'exec',
		'-T',
		'postgres',
		'psql',
		'-U',
		'kooperatif',
		'-d',
		'postgres',
		'-c',
		'DROP DATABASE IF EXISTS kooperatif_e2e_011 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_011;'
	],
	{ stdio: 'pipe' }
);

const serverEnv = {
	...process.env,
	KOOPERATIF_ENV: 'development',
	KOOPERATIF_HOST: '127.0.0.1',
	KOOPERATIF_PORT: '8099',
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
		{
			env: serverEnv,
			stdio: 'pipe',
			encoding: 'utf8'
		}
	);
let result = cargo(['migrate']);
if (result.status !== 0) fail(`migrate: ${result.stderr}`);
{
	const { execFileSync } = await import('node:child_process');
	try {
		execFileSync(
			'cargo',
			[
				'run',
				'--manifest-path',
				`${ROOT}/apps/server/Cargo.toml`,
				'--quiet',
				'--',
				'create-user',
				'--username',
				'e2eadmin',
				'--display-name',
				'E2E Yoneticisi',
				'--password-stdin'
			],
			{
				env: serverEnv,
				input: 'e2e-parola-cok-gizli-1',
				stdio: ['pipe', 'pipe', 'pipe']
			}
		);
	} catch (error) {
		fail(`create-user stdin: ${error.stderr?.toString()}`);
	}
}
result = cargo(['grant-role', '--username', 'e2eadmin', '--role', 'Sistem Yöneticisi']);
if (result.status !== 0) fail(`grant-role: ${result.stderr}`);

// --- 2. Start server + web ------------------------------------------------
log('starting backend');
const server = spawn(`cargo run --manifest-path "${ROOT}/apps/server/Cargo.toml" --quiet`, {
	env: serverEnv,
	stdio: 'pipe',
	shell: true
});
server.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));

log('starting web');
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port 5199 --strictPort`, {
	env: { ...process.env, PUBLIC_API_BASE_URL: API },
	stdio: 'pipe',
	shell: true
});
web.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));

const killTree = (child) => {
	if (!child?.pid) return;
	if (process.platform === 'win32') {
		spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], {
			stdio: 'pipe'
		});
	} else {
		child.kill('SIGTERM');
	}
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

const todayIstanbul = () => {
	// Europe/Istanbul business date (UTC+3, no DST since 2016).
	return new Date(Date.now() + 3 * 3600_000).toISOString().slice(0, 10);
};

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	const browser = await chromium.launch({ headless: true });
	const page = await browser.newPage({ locale: 'tr-TR' });
	page.on('console', (msg) => {
		if (process.env.E2E_VERBOSE && msg.type() === 'error') {
			console.error(`[browser] ${msg.text()}`);
		}
	});
	page.on('dialog', (dialog) => void dialog.accept());

	const loginResponse = await page.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
	});
	if (!loginResponse.ok()) throw new Error(`api login failed: ${loginResponse.status()}`);
	let csrf = (await loginResponse.json()).csrfToken;

	const apiPost = async (path, body) => {
		const response = await page.request.post(`${API}${path}`, {
			headers: {
				'content-type': 'application/json',
				origin: WEB,
				'x-csrf-token': csrf
			},
			data: body
		});
		if (!response.ok()) {
			throw new Error(`POST ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json().catch(() => ({}));
	};
	const apiGet = async (path) => {
		const response = await page.request.get(`${API}${path}`, {
			headers: { origin: WEB }
		});
		if (!response.ok()) {
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json();
	};

	// --- 3. Fixtures: shareholder + two shares + funded cash account ----
	const shareholder = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'E2E', lastName: 'İadeEden' },
		family: { mode: 'new', sequenceNumber: 910 }
	});
	const share = await apiPost('/api/shares', {
		shareholderId: shareholder.id,
		acquisitionType: 'founder',
		// Backdated: the same-day effective return needs the cutoff day
		// start not to precede the ownership start.
		effectiveAt: '2025-01-01T00:00:00Z'
	});
	const siblingShare = await apiPost('/api/shares', {
		shareholderId: shareholder.id,
		acquisitionType: 'founder',
		effectiveAt: '2025-01-01T00:00:00Z'
	});
	const cash = await apiPost('/api/financial-accounts', {
		name: 'E2E Kasa',
		accountType: 'cash'
	});
	// Fund via a real Income — the settlement must NEVER reuse that path.
	const categories = await apiGet('/api/financial-categories/options?categoryType=income');
	const incomeCategory = categories.find((c) => c.name === 'Diğer Gelir');
	await apiPost('/api/incomes', {
		financialAccountId: cash.id,
		categoryId: incomeCategory.id,
		amount: '5000.00',
		description: 'E2E fonlama',
		idempotencyKey: 'e2e-fund-1'
	});
	log(
		`fixtures: share #${share.shareNumber} + sibling #${siblingShare.shareNumber}, kasa 5.000,00`
	);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');
	const relogin = async () => {
		const response = await page.request.post(`${API}/api/auth/login`, {
			headers: { 'content-type': 'application/json', origin: WEB },
			data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
		});
		csrf = (await response.json()).csrfToken;
	};
	await relogin();

	// --- 4. UI: initiate the return ------------------------------------
	await page.goto(`${WEB}/hisse-iadeleri/yeni?share=${share.id}`, {
		waitUntil: 'networkidle'
	});
	await page.getByText(`Hisse No ${share.shareNumber}`).first().waitFor();
	await page.locator('#sr-date').fill(todayIstanbul());
	await page.locator('#sr-reason').fill('E2E çıkış');
	const [initResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/api/share-returns') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'İade Talebini Oluştur' }).click()
	]);
	if (!initResponse.ok()) {
		fail(`initiate → ${initResponse.status()}: ${await initResponse.text()}`);
	}
	const returnCase = await initResponse.json();
	await page.waitForURL(`**/hisse-iadeleri/${returnCase.id}`);
	await page.getByText('Beklemede').first().waitFor();
	log(`return #${returnCase.returnNumber} pending — initiated through the UI`);

	let shareDetail = await apiGet(`/api/shares/${share.id}`);
	if (shareDetail.status !== 'return_pending') {
		fail(`share status after initiate: ${shareDetail.status}`);
	}
	if (!shareDetail.owner?.shareholderId) fail('owner must remain until finalize');
	let sibling = await apiGet(`/api/shares/${siblingShare.id}`);
	if (sibling.status !== 'active') fail(`sibling share disturbed: ${sibling.status}`);
	let moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (moves.items.length !== 1 || moves.items[0].sourceType !== 'income') {
		fail(`initiation moved money: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	log('share → İade Bekliyor; sibling share aktif; sıfır para hareketi');

	// --- 5. UI: finalize — principal determined, profit undetermined ----
	await page.getByRole('button', { name: 'Kesinleştir' }).click();
	await page.locator('#fin-p-amount').fill('1.000,00');
	await page.locator('#fin-p-due').fill('2026-12-31');
	await page.locator('#fin-p-policy').fill('YK-2026-01 iade kararı');
	await page.locator('#fin-k').check();
	const [finResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/finalize') && r.request().method() === 'POST'),
		page.getByRole('button', { name: 'Kesinleştir' }).last().click()
	]);
	if (!finResponse.ok()) {
		fail(`finalize → ${finResponse.status()}: ${await finResponse.text()}`);
	}
	const finalized = await finResponse.json();
	await page.getByText('Kesinleşti').first().waitFor();
	await page.getByText('Ana Para Hakkı').first().waitFor();
	await page.getByText('Henüz belirlenmedi').first().waitFor();
	log('finalize: Ana Para Hakkı 1.000,00 + Kâr Payı Hakkı (belirlenmedi)');

	if (finalized.status !== 'finalized') fail(`return status: ${finalized.status}`);
	const principal = finalized.entitlements.find((e) => e.entitlementType === 'principal');
	const profit = finalized.entitlements.find((e) => e.entitlementType === 'profit');
	if (principal.amount !== '1000.00' || principal.remainingAmount !== '1000.00') {
		fail(`principal entitlement: ${JSON.stringify(principal)}`);
	}
	if (profit.amount !== null) fail('profit right must stay undetermined (NULL ≠ 0)');

	shareDetail = await apiGet(`/api/shares/${share.id}`);
	if (shareDetail.status !== 'closed') fail(`share status: ${shareDetail.status}`);
	if (shareDetail.owner !== null) fail('closed share must have no owner');
	sibling = await apiGet(`/api/shares/${siblingShare.id}`);
	if (sibling.status !== 'active') fail(`sibling share disturbed: ${sibling.status}`);
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (moves.items.length !== 1) {
		fail(`crystallization moved money: ${moves.items.length} movements`);
	}
	log('share → Kapandı; sahiplik kapandı; hak ediş tanımı SIFIR hareket');

	// --- 6. UI: partial settlement -------------------------------------
	await page.getByRole('button', { name: 'Ödeme Yap' }).click();
	await page.locator('#st-account').selectOption(cash.id);
	await page.locator('#st-amount').fill('400,00');
	const [settleResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/settlements') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Ödemeyi Kaydet' }).click()
	]);
	if (!settleResponse.ok()) {
		fail(`settle → ${settleResponse.status()}: ${await settleResponse.text()}`);
	}
	const settlement = await settleResponse.json();
	await page.getByText('Kısmen Ödendi').first().waitFor();
	await page.getByText('600,00 ₺').first().waitFor();
	log(`partial settlement #${settlement.settlementNumber}: 400,00 — kalan 600,00`);

	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '4600.00') fail(`balance after settle: ${cashDetail.balance}`);
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const settlementMoves = moves.items.filter((m) => m.sourceType === 'share_return_settlement');
	if (settlementMoves.length !== 1 || settlementMoves[0].direction !== 'outflow') {
		fail(`settlement provenance: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	const summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '5000.00' || summary.expenseTotal !== '0.00') {
		fail(`summary disturbed by settlement: ${JSON.stringify(summary)}`);
	}
	log('settlement ≠ gelir/gider/tahsilat/transfer — tek share_return_settlement çıkışı');

	// --- 7. UI: reverse the settlement ----------------------------------
	await page.getByRole('button', { name: 'Ödemeyi Ters Kaydet' }).click();
	await page.getByPlaceholder('Ters kayıt gerekçesi').fill('yanlış hesap seçimi');
	const [reverseResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/reverse') && r.request().method() === 'POST'),
		page.getByRole('button', { name: 'Ödemeyi Ters Kaydet' }).last().click()
	]);
	if (!reverseResponse.ok()) {
		fail(`reverse → ${reverseResponse.status()}: ${await reverseResponse.text()}`);
	}
	await page.getByText('Ters Kayıt').first().waitFor();
	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '5000.00') fail(`balance after reversal: ${cashDetail.balance}`);
	const detailAfter = await apiGet(`/api/share-returns/${returnCase.id}`);
	const principalAfter = detailAfter.entitlements.find((e) => e.entitlementType === 'principal');
	if (principalAfter.status !== 'open' || principalAfter.remainingAmount !== '1000.00') {
		fail(`entitlement after reversal: ${JSON.stringify(principalAfter)}`);
	}
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const reversed = moves.items.find((m) => m.sourceType === 'share_return_settlement');
	if (reversed.status !== 'reversed') fail('movement must stay with reversed history');
	log('reversal: hareket korunarak ters kaydedildi; kalan 1.000,00');

	// --- 8. List + shareholder context ----------------------------------
	await page.goto(`${WEB}/hisse-iadeleri`, { waitUntil: 'networkidle' });
	await page.locator('table').getByText('Kesinleşti').first().waitFor();
	await page.goto(`${WEB}/hissedarlar/${shareholder.id}`, {
		waitUntil: 'networkidle'
	});
	await page.getByText('Hisse İadeleri').first().waitFor();
	log('liste + hissedar bağlamı doğrulandı');

	await browser.close();
	log('PASS');
} catch (error) {
	fail(error?.message ?? String(error));
} finally {
	await shutdown();
}
