// STEP-009 focused E2E: Shareholder Credit / "Fazla Ödeme" through the
// real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture family + shareholder + open Period with one
//   assessment + a CASH account → a 500,00 Payment settles the 300,00
//   assessment and leaves a 200,00 remainder → UI "Kalanı Avans Olarak
//   Ata" assigns the remainder to the shareholder EXPLICITLY → a second
//   period's assessment is generated and the held credit auto-offsets it
//   → the automatic application is reversed through the UI → the same
//   assessment is settled again via MANUAL apply → Payment reversal is
//   blocked because the credit is partially consumed.
//
// Financial boundaries asserted end-to-end:
//   - money location ≠ credit ownership: the cash account shows ONE
//     inflow movement (the Payment) and its derived balance never
//     changes during credit assign/apply/reverse operations;
//   - credit application is NOT a Payment and creates NO movement;
//   - unassigned remainder is never silently attributed — it stays on
//     the Payment until an explicit beneficiary is chosen.
// Run: node scripts/e2e-step009.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8098';
const WEB = 'http://localhost:5198';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_009';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_009 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_009;'
	],
	{ stdio: 'pipe' }
);

const serverEnv = {
	...process.env,
	KOOPERATIF_ENV: 'development',
	KOOPERATIF_HOST: '127.0.0.1',
	KOOPERATIF_PORT: '8098',
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
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port 5198 --strictPort`, {
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

const activeMovementCount = async (accountId) => {
	const movements = await apiGet(
		`/api/financial-accounts/${accountId}/movements?pageSize=50`
	);
	return {
		total: movements.items.length,
		active: movements.items.filter((m) => m.status === 'active').length
	};
};

let apiGet; // hoisted for activeMovementCount; assigned inside try

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
	const csrfToken = (await loginResponse.json()).csrfToken;

	const apiPost = async (path, body) => {
		const response = await page.request.post(`${API}${path}`, {
			headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrfToken },
			data: body
		});
		if (!response.ok()) {
			throw new Error(`POST ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json().catch(() => ({}));
	};
	apiGet = async (path) => {
		const response = await page.request.get(`${API}${path}`, {
			headers: { origin: WEB }
		});
		if (!response.ok()) {
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json();
	};

	// --- 3. Fixtures: cash account, family, member, period 1 ------------
	const cash = await apiPost('/api/financial-accounts', {
		name: 'E2E Kasa',
		accountType: 'cash',
		currency: 'TRY'
	});
	const seq = Math.floor(2000 + Math.random() * 7000);
	const family = await apiPost('/api/families', { sequenceNumber: seq });
	const member = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ali', lastName: 'Aile' },
		family: { mode: 'existing', familyId: family.id }
	});
	const period1 = await apiPost('/api/periods', {
		name: 'Ocak Dönemi',
		collectionStartDate: '2026-01-01',
		dueDate: '2026-01-31',
		ruleType: 'per_shareholder',
		baseAmount: '300.00',
		assessmentEffectiveDate: '2026-01-01'
	});
	await apiPost(`/api/periods/${period1.id}/generate-assessments`, null);
	const open1 = await apiGet(`/api/shareholders/${member.id}/open-assessments`);
	if (open1.length !== 1 || open1[0].amount !== '300.00') {
		fail(`open assessments after generation: ${JSON.stringify(open1)}`);
	}
	const assessment1 = open1[0];
	log(`fixture: cash ${cash.id}, family ${seq}, member ${member.id}, assessment ${assessment1.id}`);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// The UI login replaced the session cookie — refresh the token.
	let csrf = csrfToken;
	const relogin = async () => {
		const response = await page.request.post(`${API}/api/auth/login`, {
			headers: { 'content-type': 'application/json', origin: WEB },
			data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
		});
		csrf = (await response.json()).csrfToken;
	};
	const apiPostWith = (path, body) =>
		page.request
			.post(`${API}${path}`, {
				headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
				data: body
			})
			.then(async (r) => {
				if (!r.ok()) throw new Error(`POST ${path} → ${r.status()}: ${await r.text()}`);
				return r.json().catch(() => ({}));
			});
	await relogin();

	// --- 4. Payment 500,00 → 300 allocated, 200 remainder -----------------
	const created = await apiPostWith('/api/payments', {
		payerFirstName: 'Ödeyen',
		payerLastName: 'Üçüncü Kişi',
		amount: '500.00',
		method: 'cash',
		destinationAccountId: cash.id,
		idempotencyKey: crypto.randomUUID(),
		allocations: [{ assessmentId: assessment1.id, amount: '300.00' }]
	});
	const payment = created.payment;
	if (payment.unallocatedAmount !== '200.00') {
		fail(`payment remainder: ${payment.unallocatedAmount}`);
	}
	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '500.00') fail(`cash balance: ${cashDetail.balance}`);
	log('payment posted: 500,00 in, 300,00 allocated — 200,00 remainder stays ON the payment');

	// --- 5. UI: assign the remainder as credit to the shareholder --------
	await page.goto(`${WEB}/tahsilatlar/${payment.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Fazla Ödemeler').waitFor();
	await page.getByRole('button', { name: 'Kalanı Avans Olarak Ata' }).first().click();
	await page.getByPlaceholder('Hissedar ara (ad, vasi, aile no)').fill('Ali');
	await page.getByRole('button', { name: 'Ara', exact: true }).click();
	await page.getByRole('button', { name: /Ali Aile/ }).click();
	await page.locator('#assign-amount').waitFor();
	const [assignResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/credits') && r.request().method() === 'POST'),
		page.getByRole('button', { name: 'Avansı Kaydet' }).click()
	]);
	if (!assignResponse.ok()) {
		fail(`credit assign → ${assignResponse.status()}: ${await assignResponse.text()}`);
	}
	await page.getByText('Hak Sahibi Hissedar').waitFor();
	await page.getByText('200,00 ₺').first().waitFor();
	log('remainder explicitly assigned to the shareholder — "Fazla Ödeme" visible');

	let ledger = await apiGet(`/api/shareholders/${member.id}/credits`);
	if (ledger.summary.available !== '200.00' || ledger.summary.creditCount !== 1) {
		fail(`ledger after assign: ${JSON.stringify(ledger.summary)}`);
	}
	let moves = await activeMovementCount(cash.id);
	if (moves.total !== 1 || moves.active !== 1) {
		fail(`cash movements after credit assign: ${JSON.stringify(moves)}`);
	}
	log('credit assign created NO account movement — money location untouched');

	// --- 6. New period: generation auto-offsets the held credit ---------
	const period2 = await apiPostWith('/api/periods', {
		name: 'Şubat Dönemi',
		collectionStartDate: '2026-02-01',
		dueDate: '2026-02-28',
		ruleType: 'per_shareholder',
		baseAmount: '150.00',
		assessmentEffectiveDate: '2026-02-01'
	});
	await apiPostWith(`/api/periods/${period2.id}/generate-assessments`, null);

	ledger = await apiGet(`/api/shareholders/${member.id}/credits`);
	if (ledger.summary.available !== '50.00' || ledger.summary.totalApplied !== '150.00') {
		fail(`ledger after auto-offset: ${JSON.stringify(ledger.summary)}`);
	}
	const autoApps = ledger.applications.filter(
		(a) => a.status === 'active' && a.mode === 'automatic'
	);
	if (autoApps.length !== 1 || autoApps[0].amount !== '150.00') {
		fail(`automatic applications: ${JSON.stringify(autoApps)}`);
	}
	const assessment2 = autoApps[0].assessmentId;
	moves = await activeMovementCount(cash.id);
	if (moves.total !== 1 || moves.active !== 1) {
		fail(`cash movements after auto-offset: ${JSON.stringify(moves)}`);
	}
	log('auto-offset settled the new assessment: credit 200→50, still ONE movement');

	// --- 7. UI: reverse the automatic application ------------------------
	await page.goto(`${WEB}/tahakkuklar/${assessment2}`, { waitUntil: 'networkidle' });
	await page.getByText('Avans Mahsupları').waitFor();
	await page.getByRole('cell', { name: 'Otomatik' }).waitFor();
	await page.getByRole('button', { name: 'Mahsubu Geri Al' }).click();
	await page.locator('#app-reason').fill('e2e hatalı otomatik mahsup');
	const [appReverseResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/reverse')),
		page.getByRole('button', { name: 'Mahsubu Geri Al' }).last().click()
	]);
	if (!appReverseResponse.ok()) {
		fail(`application reverse → ${appReverseResponse.status()}`);
	}
	await page.getByRole('cell', { name: 'Ters Kayıt' }).first().waitFor();

	ledger = await apiGet(`/api/shareholders/${member.id}/credits`);
	if (ledger.summary.available !== '200.00') {
		fail(`ledger after application reversal: ${JSON.stringify(ledger.summary)}`);
	}
	const reopened = await apiGet(`/api/shareholders/${member.id}/open-assessments`);
	if (!reopened.some((a) => a.id === assessment2 && a.remainingAmount === '150.00')) {
		fail(`assessment not reopened: ${JSON.stringify(reopened)}`);
	}
	log('automatic application reversed — debt reopened, credit restored to 200,00');

	// --- 8. UI: MANUAL apply settles the assessment again -----------------
	await page.getByRole('button', { name: 'Avans ile Mahsup' }).click();
	await page.locator('#apply-amount').waitFor();
	const [applyResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('credit-applications') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Mahsup Et' }).click()
	]);
	if (!applyResponse.ok()) {
		fail(`manual apply → ${applyResponse.status()}: ${await applyResponse.text()}`);
	}
	await page.getByRole('cell', { name: 'Manuel' }).waitFor();

	ledger = await apiGet(`/api/shareholders/${member.id}/credits`);
	if (ledger.summary.available !== '50.00' || ledger.summary.totalApplied !== '150.00') {
		fail(`ledger after manual apply: ${JSON.stringify(ledger.summary)}`);
	}
	moves = await activeMovementCount(cash.id);
	if (moves.total !== 1 || moves.active !== 1) {
		fail(`cash movements after manual apply: ${JSON.stringify(moves)}`);
	}
	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '500.00') fail(`cash balance: ${cashDetail.balance}`);
	log('manual apply settled 150,00 — credit 50,00 left, STILL one movement, balance 500,00');

	// --- 9. Consumed credit blocks the source payment reversal -----------
	const reverseResponse = await page.request.post(`${API}/api/payments/${payment.id}/reverse`, {
		headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
		data: { reason: 'e2e ters kayıt denemesi' }
	});
	if (reverseResponse.status() !== 409) {
		fail(`payment reversal with consumed credit: ${reverseResponse.status()}`);
	}
	log('payment reversal blocked (409) — consumed credit protects history');

	// --- 10. Shareholder detail: derived credit summary -------------------
	await page.goto(`${WEB}/hissedarlar/${member.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Kullanılabilir Avans', { exact: true }).waitFor();
	await page.getByText('50,00 ₺').first().waitFor();
	log('shareholder detail shows derived available credit: 50,00 ₺');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
