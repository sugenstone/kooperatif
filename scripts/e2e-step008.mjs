// STEP-008 focused E2E: a deterministic browser workflow through the
// real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture family + shareholder + open Period with
//   assessments + a CASH and a BANK financial account (API) → UI Yeni
//   Tahsilat posts a Payment INTO the cash account → account detail
//   shows the derived balance and the Payment-sourced movement → UI
//   transfer moves value cash → bank (paired legs, derived balances)
//   → transfer reversal restores both balances (history preserved)
//   → payment reversal restores the cash account to zero.
//
// Financial boundary asserted: a Payment is NOT an Account Movement and
// an account balance is NEVER a stored number — it is the sum of valid
// movements only.
// Run: node scripts/e2e-step008.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_008';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_008 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_008;'
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
	let csrfToken = (await loginResponse.json()).csrfToken;

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
	const apiGet = async (path) => {
		const response = await page.request.get(`${API}${path}`, {
			headers: { origin: WEB }
		});
		if (!response.ok()) {
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json();
	};

	// --- 3. Fixtures: cash + bank accounts, family, member, open period ---
	const cash = await apiPost('/api/financial-accounts', {
		name: 'E2E Kasa',
		accountType: 'cash',
		currency: 'TRY'
	});
	const bank = await apiPost('/api/financial-accounts', {
		name: 'E2E Banka',
		accountType: 'bank',
		currency: 'TRY',
		bankName: 'E2E Bankası A.Ş.'
	});
	const seq = Math.floor(2000 + Math.random() * 7000);
	const family = await apiPost('/api/families', { sequenceNumber: seq });
	const member = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ali', lastName: 'Aile' },
		family: { mode: 'existing', familyId: family.id }
	});
	const period = await apiPost('/api/periods', {
		name: 'Tahsilat Dönemi',
		collectionStartDate: '2026-01-01',
		dueDate: '2026-01-31',
		ruleType: 'per_shareholder',
		baseAmount: '300.00',
		assessmentEffectiveDate: '2026-01-01'
	});
	await apiPost(`/api/periods/${period.id}/generate-assessments`, null);
	log(`fixture: cash ${cash.id}, bank ${bank.id}, family ${seq} member ${member.id}`);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// --- 4. New collection: the destination account is REQUIRED -----------
	await page.goto(`${WEB}/tahsilatlar/yeni`, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Yeni kişi olarak kaydet' }).click();
	await page.locator('#payer-first').fill('Ödeyen');
	await page.locator('#payer-last').fill('Üçüncü Kişi');

	await page.getByRole('button', { name: 'Aileye göre' }).click();
	await page.getByPlaceholder('Aile ara (aile no, üye adı)').fill(String(seq));
	await page.getByRole('button', { name: 'Ara', exact: true }).click();
	await page.getByRole('button', { name: `Aile No: ${seq}` }).click();
	await page.getByText('Aile Toplam Kalan').waitFor();
	await page.getByRole('button', { name: 'Üyelerin açık borçları' }).first().click();
	await page.getByText('Dağıtılacak Tutar').first().waitFor();
	await page.locator('tbody input[type="checkbox"]').first().check();

	await page.locator('#pay-amount').fill('300,00');
	await page.locator('#pay-account').selectOption(cash.id);
	await page.getByRole('button', { name: 'Tahsilatı Kaydet' }).click();
	await page.waitForURL('**/tahsilatlar/*');
	await page.getByText('Kayıtlı').first().waitFor();
	await page.getByText('E2E Kasa').first().waitFor();
	const paymentUrl = page.url();
	log('payment posted INTO the cash account — account shown on the payment');

	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '300.00') {
		fail(`cash balance after payment: ${cashDetail.balance}`);
	}
	log('cash balance = 300,00 (derived from the single payment movement)');

	// --- 5. Account detail: derived balance + movement history ------------
	await page.goto(`${WEB}/finansal-hesaplar/${cash.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Hesap Hareketleri').waitFor();
	await page.getByText('300,00 ₺').first().waitFor();
	await page.getByRole('cell', { name: 'Giriş' }).first().waitFor();
	await page.getByText('Tahsilat').first().waitFor();
	log('account detail shows derived balance + the Payment-sourced inflow');

	// --- 6. Transfer cash → bank through the UI ---------------------------
	await page.goto(`${WEB}/transferler/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#tr-source').selectOption(cash.id);
	await page.locator('#tr-destination').selectOption(bank.id);
	await page.locator('#tr-amount').fill('250,00');
	await page.getByRole('button', { name: 'Transferi Kaydet' }).click();
	await page.waitForURL('**/transferler/*');
	await page.getByRole('heading', { name: /Transfer Detayı/ }).waitFor();
	await page.getByText('Kayıtlı').first().waitFor();
	const transferUrl = page.url();
	log('transfer posted: 250,00 cash → bank (one op, two movement legs)');

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	const bankDetail = await apiGet(`/api/financial-accounts/${bank.id}`);
	if (cashDetail.balance !== '50.00' || bankDetail.balance !== '250.00') {
		fail(`balances after transfer: cash=${cashDetail.balance} bank=${bankDetail.balance}`);
	}
	log('derived balances: cash 50,00 / bank 250,00');

	const cashMovements = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const bankMovements = await apiGet(`/api/financial-accounts/${bank.id}/movements?pageSize=50`);
	if (cashMovements.items.length !== 2 || bankMovements.items.length !== 1) {
		fail(`movement counts: cash=${cashMovements.items.length} bank=${bankMovements.items.length}`);
	}

	// --- 7. Insufficient-funds guard through the API ----------------------
	// The UI form login replaced the session cookie — refresh the token.
	const relogin = await page.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
	});
	csrfToken = (await relogin.json()).csrfToken;
	const overdraw = await page.request.post(`${API}/api/account-transfers`, {
		headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrfToken },
		data: {
			sourceAccountId: cash.id,
			destinationAccountId: bank.id,
			amount: '999999.00',
			idempotencyKey: crypto.randomUUID()
		}
	});
	if (overdraw.status() !== 409 && overdraw.status() !== 422) {
		fail(`overdraw not rejected: ${overdraw.status()}`);
	}
	log(`insufficient-funds transfer rejected (${overdraw.status()}) — no negative balance`);

	// --- 8. Transfer reversal restores both balances ----------------------
	await page.goto(transferUrl, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Transferi Ters Kaydet' }).click();
	await page.locator('#transfer-reason').fill('e2e hatalı transfer');
	const [transferReverseResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('/reverse')),
		page.getByRole('button', { name: 'Transferi Ters Kaydet' }).click()
	]);
	if (!transferReverseResponse.ok()) {
		fail(`transfer reverse → ${transferReverseResponse.status()}`);
	}
	await page.getByText('Ters Kayıt', { exact: true }).first().waitFor();
	await page.getByText('e2e hatalı transfer').first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	const bankAfter = await apiGet(`/api/financial-accounts/${bank.id}`);
	if (cashDetail.balance !== '300.00' || bankAfter.balance !== '0.00') {
		fail(`balances after reversal: cash=${cashDetail.balance} bank=${bankAfter.balance}`);
	}
	log('transfer reversed — originals preserved, derived balances restored');

	// --- 9. Payment reversal restores the cash account ---------------------
	await page.goto(paymentUrl, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Ters Kayıt Yap' }).click();
	await page.locator('#payment-reason').fill('e2e hatalı tahsilat');
	const [paymentReverseResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().includes('reverse')),
		page.getByRole('button', { name: 'Ters Kayıt Yap' }).click()
	]);
	if (!paymentReverseResponse.ok()) {
		fail(`payment reverse → ${paymentReverseResponse.status()}`);
	}
	await page.getByText('Ters Kayıt', { exact: true }).first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '0.00') {
		fail(`cash balance after payment reversal: ${cashDetail.balance}`);
	}
	const history = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (!history.items.every((m) => m.status === 'reversed')) {
		fail('reversed movements still counted as active');
	}
	log('payment reversed — every cash movement reversed, balance derives to 0,00');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
