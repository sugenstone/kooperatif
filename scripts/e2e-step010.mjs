// STEP-010 focused E2E: Income & Expense management through the real
// stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture CASH account → UI posts a 5.000,00 Income →
//   the account balance rises by exactly 5.000,00 through ONE inflow
//   movement → UI posts a 1.250,00 Expense → balance drops by exactly
//   1.250,00 through ONE outflow movement → the operational summary
//   shows 5.000/1.250/3.750 (NOT the account balance) → the Expense is
//   reversed through the UI → its movement is reversed (history kept)
//   and the balance returns to 5.000,00 → a Payment posts to the same
//   account and is NOT counted as Income → the summary stays correct.
//
// Financial boundaries asserted end-to-end:
//   - Income → exactly ONE incoming movement; Expense → exactly ONE
//     outgoing movement; no second ledger;
//   - account balance stays derived from ACTIVE movements only;
//   - Income/Expense totals exclude Payments (no double counting);
//   - reversal preserves rows and movement provenance.
// Run: node scripts/e2e-step010.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_010';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_010 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_010;'
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
			{ env: serverEnv, input: 'e2e-parola-cok-gizli-1', stdio: ['pipe', 'pipe', 'pipe'] }
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
		spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'pipe' });
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
			headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
			data: body
		});
		if (!response.ok()) {
			throw new Error(`POST ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json().catch(() => ({}));
	};
	const apiGet = async (path) => {
		const response = await page.request.get(`${API}${path}`, { headers: { origin: WEB } });
		if (!response.ok()) {
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json();
	};
	const movementsOf = async (accountId) => {
		const list = await apiGet(`/api/financial-accounts/${accountId}/movements?pageSize=50`);
		return {
			total: list.items.length,
			active: list.items.filter((m) => m.status === 'active').length,
			items: list.items
		};
	};

	// --- 3. Fixture: cash account ---------------------------------------
	const cash = await apiPost('/api/financial-accounts', {
		name: 'E2E Kasa',
		accountType: 'cash',
		currency: 'TRY'
	});
	log(`fixture: cash account ${cash.id}`);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// The UI login replaced the session cookie — refresh the token.
	const relogin = async () => {
		const response = await page.request.post(`${API}/api/auth/login`, {
			headers: { 'content-type': 'application/json', origin: WEB },
			data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
		});
		csrf = (await response.json()).csrfToken;
	};
	await relogin();

	// --- 4. UI: post Income 5.000,00 -------------------------------------
	await page.goto(`${WEB}/gelirler/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#inc-account').selectOption({ label: 'E2E Kasa (Kasa)' });
	await page.locator('#inc-category').selectOption({ label: 'Diğer Gelir' });
	await page.locator('#inc-amount').fill('5.000,00');
	await page.locator('#inc-description').fill('Stant kirası geliri');
	await page.locator('#inc-ref').fill('FTR-2026-01');
	const [incomeResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/api/incomes') && r.request().method() === 'POST'),
		page.getByRole('button', { name: 'Geliri Kaydet' }).click()
	]);
	if (!incomeResponse.ok()) {
		fail(`income post → ${incomeResponse.status()}: ${await incomeResponse.text()}`);
	}
	const income = await incomeResponse.json();
	await page.waitForURL(`**/gelirler/${income.id}`);
	await page.getByText('5.000,00 ₺').first().waitFor();
	log(`income posted: #${income.entryNumber} — detail shows 5.000,00 ₺`);

	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '5000.00') fail(`balance after income: ${cashDetail.balance}`);
	let moves = await movementsOf(cash.id);
	if (moves.total !== 1 || moves.active !== 1 || moves.items[0].sourceType !== 'income') {
		fail(`movements after income: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	log('balance 5.000,00 via exactly ONE income-sourced inflow movement');

	// --- 5. UI: post Expense 1.250,00 ------------------------------------
	await page.goto(`${WEB}/giderler/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#exp-account').selectOption({ label: 'E2E Kasa (Kasa)' });
	await page.locator('#exp-category').selectOption({ label: 'Elektrik / Su / İletişim' });
	await page.locator('#exp-amount').fill('1.250,00');
	await page.locator('#exp-description').fill('Elektrik faturası Ocak');
	await page.locator('#exp-counterparty').fill('Elektrik Dağıtım A.Ş.');
	const [expenseResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/api/expenses') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Gideri Kaydet' }).click()
	]);
	if (!expenseResponse.ok()) {
		fail(`expense post → ${expenseResponse.status()}: ${await expenseResponse.text()}`);
	}
	const expense = await expenseResponse.json();
	await page.waitForURL(`**/giderler/${expense.id}`);
	await page.getByText('1.250,00 ₺').first().waitFor();
	log(`expense posted: #${expense.entryNumber} — detail shows 1.250,00 ₺`);

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '3750.00') fail(`balance after expense: ${cashDetail.balance}`);
	moves = await movementsOf(cash.id);
	if (moves.total !== 2 || moves.active !== 2) {
		fail(`movements after expense: total ${moves.total}, active ${moves.active}`);
	}
	log('balance 3.750,00 — income + expense = two distinct movements');

	// --- 6. Operational summary: NOT the account balance -----------------
	const summary = await apiGet('/api/income-expense/summary');
	if (
		summary.incomeTotal !== '5000.00' ||
		summary.expenseTotal !== '1250.00' ||
		summary.net !== '3750.00'
	) {
		fail(`summary: ${JSON.stringify(summary)}`);
	}
	log('operational summary: gelir 5.000,00 / gider 1.250,00 / net 3.750,00');

	// --- 7. Lists --------------------------------------------------------
	await page.goto(`${WEB}/gelirler`, { waitUntil: 'networkidle' });
	await page.getByRole('cell', { name: 'Diğer Gelir' }).waitFor();
	await page.goto(`${WEB}/giderler`, { waitUntil: 'networkidle' });
	await page.getByRole('cell', { name: 'Elektrik / Su / İletişim' }).waitFor();
	log('income and expense lists render the posted entries');

	// --- 8. UI: reverse the expense --------------------------------------
	await page.goto(`${WEB}/giderler/${expense.id}`, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Gideri Ters Kaydet' }).click();
	await page.locator('#expense-reason').fill('e2e yanlış fatura dönemi');
	const [reverseResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/reverse')),
		page.getByRole('button', { name: 'Gideri Ters Kaydet' }).click()
	]);
	if (!reverseResponse.ok()) {
		fail(`expense reverse → ${reverseResponse.status()}: ${await reverseResponse.text()}`);
	}
	await page.getByText('e2e yanlış fatura dönemi').waitFor();
	log('expense reversed through the UI — reason preserved');

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '5000.00') fail(`balance after reversal: ${cashDetail.balance}`);
	moves = await movementsOf(cash.id);
	const reversedLegs = moves.items.filter((m) => m.status === 'reversed');
	if (moves.total !== 2 || moves.active !== 1 || reversedLegs.length !== 1) {
		fail(`movements after reversal: ${JSON.stringify(moves.items)}`);
	}
	if (reversedLegs[0].sourceType !== 'expense') {
		fail(`reversed movement source: ${reversedLegs[0].sourceType}`);
	}
	log('expense movement reversed — history kept, active legs = 1, balance 5.000,00');

	// --- 9. Payment boundary: a Payment is NOT Income --------------------
	const incomeCountBefore = (await apiGet('/api/incomes?pageSize=50')).totalCount;
	await apiPost('/api/payments', {
		payerFirstName: 'Ödeyen',
		payerLastName: 'Kişi',
		amount: '750.00',
		method: 'cash',
		destinationAccountId: cash.id,
		idempotencyKey: crypto.randomUUID(),
		allocations: []
	});
	const incomeCountAfter = (await apiGet('/api/incomes?pageSize=50')).totalCount;
	const summaryAfterPayment = await apiGet('/api/income-expense/summary');
	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (incomeCountAfter !== incomeCountBefore) {
		fail(`payment leaked into income list (${incomeCountBefore} → ${incomeCountAfter})`);
	}
	if (summaryAfterPayment.incomeTotal !== '5000.00') {
		fail(`summary after payment: ${JSON.stringify(summaryAfterPayment)}`);
	}
	if (summaryAfterPayment.expenseTotal !== '0.00') {
		fail(`reversed expense still counted: ${summaryAfterPayment.expenseTotal}`);
	}
	if (cashDetail.balance !== '5750.00') fail(`balance after payment: ${cashDetail.balance}`);
	log('payment posted 750,00 — balance 5.750,00, income total still 5.000,00 (no double count)');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
