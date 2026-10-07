// STEP-012 focused E2E: Investment & Investment Cash-Flow through the
// real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → funded cash account → UI creates an investment
//   (identity only, ZERO money moves) → UI posts an acquisition
//   funding leg (exactly ONE 'investment_funding' outflow; NOT
//   expense/payment/transfer; operational summary untouched) → UI
//   records a valuation (ZERO movements, history only) → UI posts an
//   investment income (exactly ONE 'investment_income' inflow; NO
//   `incomes` row — income/expense summary untouched) → UI disposes
//   the investment with an agreed consideration + ONE proceeds leg
//   (exactly ONE 'investment_disposal' inflow; status → disposed).
//
// Boundaries asserted end-to-end:
//   - creating an Investment moves NO money;
//   - acquisition ≠ Expense; income leg ≠ operational Income;
//   - valuation is informational — it never moves cash;
//   - agreed consideration ≠ received cash (no gain is computed);
//   - every cash leg = exactly one movement, never double-counted.
// Run: node scripts/e2e-step012.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_012';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_012 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_012;'
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

	// --- 3. Fixture: funded cash account ----------------------------------
	const cash = await apiPost('/api/financial-accounts', {
		name: 'E2E Yatırım Kasası',
		accountType: 'cash'
	});
	const bank = await apiPost('/api/financial-accounts', {
		name: 'E2E Yatırım Bankası',
		accountType: 'bank'
	});
	// Fund via a real Income — investment flows must NEVER reuse that path.
	const categories = await apiGet('/api/financial-categories/options?categoryType=income');
	const incomeCategory = categories.find((c) => c.name === 'Diğer Gelir');
	await apiPost('/api/incomes', {
		financialAccountId: cash.id,
		categoryId: incomeCategory.id,
		amount: '600000.00',
		description: 'E2E yatırım fonlama',
		idempotencyKey: 'e2e-012-fund-1'
	});
	log('fixture: kasa 600.000,00 (tek income hareketi)');

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

	// --- 4. UI: create the investment — identity only --------------------
	await page.goto(`${WEB}/yatirimlar/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#inv-name').fill('E2E Depo Binası');
	await page.locator('#inv-type').selectOption('real_estate');
	await page.locator('#inv-location').fill('Organize Sanayi Bölgesi');
	const [createResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/api/investments') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Yeni Yatırım' }).click()
	]);
	if (!createResponse.ok()) {
		fail(`create → ${createResponse.status()}: ${await createResponse.text()}`);
	}
	const investment = await createResponse.json();
	await page.waitForURL(`**/yatirimlar/${investment.id}`);
	await page.getByText('Aktif').first().waitFor();
	log(`investment #${investment.investmentNumber} created — UI`);

	let moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (moves.items.length !== 1 || moves.items[0].sourceType !== 'income') {
		fail(`investment creation moved money: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '600000.00') fail(`balance after create: ${cashDetail.balance}`);
	log('yatırım kimliği SIFIR para hareketi');

	// --- 5. UI: acquisition funding — exactly one outflow ---------------
	await page.getByRole('button', { name: 'Finansman Kaydet' }).click();
	await page.locator('#fnd-account').selectOption(cash.id);
	await page.locator('#fnd-amount').fill('500.000,00');
	await page.locator('#fnd-ref').fill('SÖZ-E2E-1');
	const [fundingResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/fundings') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Finansmanı Kaydet' }).click()
	]);
	if (!fundingResponse.ok()) {
		fail(`funding → ${fundingResponse.status()}: ${await fundingResponse.text()}`);
	}
	await page.getByText('500.000,00 ₺').first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '100000.00') {
		fail(`balance after funding: ${cashDetail.balance}`);
	}
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const fundingMoves = moves.items.filter((m) => m.sourceType === 'investment_funding');
	if (fundingMoves.length !== 1 || fundingMoves[0].direction !== 'outflow') {
		fail(`funding provenance: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	let summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '600000.00' || summary.expenseTotal !== '0.00') {
		fail(`acquisition leaked into expense: ${JSON.stringify(summary)}`);
	}
	log('edinim ≠ gider/tahsilat/transfer — tek investment_funding çıkışı');

	// --- 6. UI: valuation — zero money ----------------------------------
	await page.getByRole('button', { name: 'Değerleme Kaydet' }).click();
	await page.locator('#val-date').fill(todayIstanbul());
	await page.locator('#val-amount').fill('620.000,00');
	await page.locator('#val-method').fill('Emsal karşılaştırma');
	await page.locator('#val-source').fill('Eksper raporu');
	const [valResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/valuations') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Değerlemeyi Kaydet' }).click()
	]);
	if (!valResponse.ok()) {
		fail(`valuation → ${valResponse.status()}: ${await valResponse.text()}`);
	}
	await page.getByText('Emsal karşılaştırma').first().waitFor();

	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (moves.items.length !== 2) {
		fail(`valuation moved money: ${moves.items.length} movements`);
	}
	summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '600000.00') {
		fail(`valuation leaked into income: ${JSON.stringify(summary)}`);
	}
	log('değerleme = sıfır hareket; tahmini değer ≠ nakit/kâr');

	// --- 7. UI: investment income — exactly one inflow ------------------
	await page.getByRole('button', { name: 'Yatırım Geliri Kaydet' }).click();
	await page.locator('#inc-account').selectOption(cash.id);
	await page.locator('#inc-amount').fill('4.500,00');
	await page.locator('#inc-desc').fill('E2E ilk kira');
	const [incomeResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/incomes') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Geliri Kaydet', exact: true }).click()
	]);
	if (!incomeResponse.ok()) {
		fail(`income → ${incomeResponse.status()}: ${await incomeResponse.text()}`);
	}
	await page.getByText('E2E ilk kira').first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '104500.00') {
		fail(`balance after income: ${cashDetail.balance}`);
	}
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const incomeMoves = moves.items.filter((m) => m.sourceType === 'investment_income');
	if (incomeMoves.length !== 1 || incomeMoves[0].direction !== 'inflow') {
		fail(`income provenance: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '600000.00') {
		fail(`investment income double-counted as operational income: ${JSON.stringify(summary)}`);
	}
	log('yatırım geliri = tek investment_income girişi; operasyonel gelir özeti DEĞİŞMEDİ');

	// --- 8. UI: dispose — agreed price + one actual proceeds leg --------
	await page.getByRole('button', { name: 'Yatırımı Tasfiye Et' }).click();
	await page.locator('#disp-date').fill(todayIstanbul());
	await page.locator('#disp-consideration').fill('650.000,00');
	await page.locator('#disp-counterparty').fill('E2E Alıcı AŞ');
	await page.locator('#disp-leg-acc-0').selectOption(bank.id);
	await page.locator('#disp-leg-amount-0').fill('650.000,00');
	const [disposeResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/dispose') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Tasfiyeyi Kaydet' }).click()
	]);
	if (!disposeResponse.ok()) {
		fail(`dispose → ${disposeResponse.status()}: ${await disposeResponse.text()}`);
	}
	await page.getByText('Tasfiye Edildi').first().waitFor();

	const disposed = await apiGet(`/api/investments/${investment.id}`);
	if (disposed.status !== 'disposed') fail(`investment status: ${disposed.status}`);
	if (disposed.disposal.considerationAmount !== '650000.00') {
		fail(`consideration: ${disposed.disposal.considerationAmount}`);
	}
	if (disposed.disposal.proceeds.length !== 1 || disposed.totalProceeds !== '650000.00') {
		fail(`proceeds: ${JSON.stringify(disposed.disposal)}`);
	}
	const bankDetail = await apiGet(`/api/financial-accounts/${bank.id}`);
	if (bankDetail.balance !== '650000.00') fail(`bank balance: ${bankDetail.balance}`);
	const bankMoves = await apiGet(
		`/api/financial-accounts/${bank.id}/movements?pageSize=50`
	);
	const proceedsMoves = bankMoves.items.filter((m) => m.sourceType === 'investment_disposal');
	if (proceedsMoves.length !== 1 || proceedsMoves[0].direction !== 'inflow') {
		fail(`proceeds provenance: ${JSON.stringify(bankMoves.items.map((m) => m.sourceType))}`);
	}
	summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '600000.00' || summary.expenseTotal !== '0.00') {
		fail(`disposal leaked into income/expense: ${JSON.stringify(summary)}`);
	}
	log('tasfiye: satış bedeli ≠ tahsilat; tek investment_disposal girişi; kâr HESAPLANMADI');

	// --- 9. List + no actions on a disposed investment -------------------
	await page.goto(`${WEB}/yatirimlar`, { waitUntil: 'networkidle' });
	await page.locator('table').getByText('Tasfiye Edildi').first().waitFor();
	await page.getByText('E2E Depo Binası').first().waitFor();
	await page.goto(`${WEB}/yatirimlar/${investment.id}`, { waitUntil: 'networkidle' });
	if (await page.getByRole('button', { name: 'Finansman Kaydet' }).isVisible().catch(() => false)) {
		fail('disposed investment must not offer new funding');
	}
	log('liste + tasfiye sonrası eylem kapanışı doğrulandı');

	await browser.close();
	log('PASS');
} catch (error) {
	fail(error?.message ?? String(error));
} finally {
	await shutdown();
}
