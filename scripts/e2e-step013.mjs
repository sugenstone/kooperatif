// STEP-013 focused E2E: Social Aid, Donation & Restricted Fund through
// the real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → two cash accounts (aid cash holds unrelated
//   cooperative money too) → UI creates a social aid fund (identity
//   only, ZERO money moves) → UI posts a donation from an EXTERNAL
//   donor (exactly ONE 'social_aid_donation' inflow; restricted
//   availability grows; NOT payment/credit/income/transfer) → UI posts
//   an aid disbursement to an external beneficiary (exactly ONE
//   'social_aid_disbursement' outflow; restricted + physical balances
//   fall) → API over-disbursement rejected on BOTH dimensions →
//   disbursement reversal restores money and restriction → donation
//   reversal is refused while restricted money is consumed → fund
//   closes only at zero availability.
//
// Boundaries asserted end-to-end:
//   - Fund ≠ Financial Account: fund creation moves NO money;
//   - donation ≠ Payment/Allocation/Credit/Income/Transfer;
//   - aid ≠ Expense/Transfer — Social Aid never merges into
//     cooperative finance;
//   - restricted money cannot be spent merely because unrelated cash
//     or another account's restricted balance exists;
//   - reversals preserve history, never delete;
//   - every cash event = exactly one movement, never double-counted.
// Run: node scripts/e2e-step013.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_013';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_013 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_013;'
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
		return response;
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

	// --- 3. Fixture: aid cash account holding UNRELATED cooperative money
	const cash = await (
		await apiPost('/api/financial-accounts', { name: 'E2E Yardım Kasası', accountType: 'cash' })
	).json();
	const other = await (
		await apiPost('/api/financial-accounts', { name: 'E2E Genel Kasa', accountType: 'cash' })
	).json();
	const categories = await apiGet('/api/financial-categories/options?categoryType=income');
	const incomeCategory = categories.find((c) => c.name === 'Diğer Gelir');
	let response = await apiPost('/api/incomes', {
		financialAccountId: cash.id,
		categoryId: incomeCategory.id,
		amount: '50000.00',
		description: 'E2E kooperatif geliri',
		idempotencyKey: 'e2e-013-income-1'
	});
	if (!response.ok()) fail(`income fixture: ${await response.text()}`);
	response = await apiPost('/api/incomes', {
		financialAccountId: other.id,
		categoryId: incomeCategory.id,
		amount: '80000.00',
		description: 'E2E genel kasa',
		idempotencyKey: 'e2e-013-income-2'
	});
	if (!response.ok()) fail(`income fixture 2: ${await response.text()}`);
	log('fixture: yardım kasası 50.000 (kooperatif parası) + genel kasa 80.000');

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');
	{
		const response = await page.request.post(`${API}/api/auth/login`, {
			headers: { 'content-type': 'application/json', origin: WEB },
			data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
		});
		csrf = (await response.json()).csrfToken;
	}

	// --- 4. UI: create the fund — restriction identity only ---------------
	await page.goto(`${WEB}/sosyal-yardim/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#fund-name').fill('E2E Eğitim Fonu');
	await page.locator('#fund-description').fill('Burs ve eğitim destekleri');
	const [createResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/api/social-aid/funds') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Yeni Fon' }).click()
	]);
	if (!createResponse.ok()) {
		fail(`create fund → ${createResponse.status()}: ${await createResponse.text()}`);
	}
	const fund = await createResponse.json();
	await page.waitForURL(`**/sosyal-yardim/${fund.id}`);
	await page.getByText('Aktif').first().waitFor();
	log(`fund #${fund.fundNumber} created — UI`);

	let moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	if (moves.items.length !== 1 || moves.items[0].sourceType !== 'income') {
		fail(`fund creation moved money: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	let cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '50000.00') fail(`balance after create: ${cashDetail.balance}`);
	let fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	if (fundDetail.available !== '0.00' || fundDetail.accounts.length !== 0) {
		fail(`fresh fund availability: ${JSON.stringify(fundDetail.accounts)}`);
	}
	log('fon kimliği SIFIR para hareketi; tahsisli bakiye 0,00');

	// --- 5. UI: donation from an EXTERNAL donor — exactly one inflow ------
	await page.getByRole('button', { name: 'Bağış Kaydet' }).click();
	await page.locator('#don-display').fill('E2E Hayırsever A.Ş.');
	await page.locator('#don-account').selectOption(cash.id);
	await page.locator('#don-amount').fill('20.000,00');
	await page.locator('#don-ref').fill('MKB-E2E-1');
	const [donationResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/api/social-aid/donations') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Bağışı Kaydet' }).click()
	]);
	if (!donationResponse.ok()) {
		fail(`donation → ${donationResponse.status()}: ${await donationResponse.text()}`);
	}
	const donation = await donationResponse.json();
	await page.getByText('E2E Hayırsever A.Ş.').first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '70000.00') fail(`balance after donation: ${cashDetail.balance}`);
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const donationMoves = moves.items.filter((m) => m.sourceType === 'social_aid_donation');
	if (donationMoves.length !== 1 || donationMoves[0].direction !== 'inflow') {
		fail(`donation provenance: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	if (fundDetail.available !== '20000.00' || fundDetail.totalDonated !== '20000.00') {
		fail(`fund availability after donation: ${fundDetail.available}`);
	}
	const pair = fundDetail.accounts.find((a) => a.financialAccountId === cash.id);
	if (!pair || pair.available !== '20000.00' || pair.physicalBalance !== '70000.00') {
		fail(`per-account restricted view: ${JSON.stringify(fundDetail.accounts)}`);
	}
	let summary = await apiGet('/api/income-expense/summary');
	if (summary.incomeTotal !== '130000.00' || summary.expenseTotal !== '0.00') {
		fail(`donation leaked into operational income: ${JSON.stringify(summary)}`);
	}
	log('bağış = tek social_aid_donation girişi; operasyonel gelir DEĞİŞMEDİ; kimlik: dış bağışçı');

	// --- 6. UI: aid disbursement — exactly one outflow --------------------
	await page.getByRole('button', { name: 'Yardım Ödemesi Kaydet' }).click();
	await page.locator('#dis-display').fill('E2E Öğrenci Ailesi');
	await page.locator('#dis-account').selectOption(cash.id);
	await page.getByText(/Bu hesaptaki tahsisli bakiye/).waitFor();
	await page.locator('#dis-amount').fill('12.000,00');
	await page.locator('#dis-reason').fill('Burs ödemesi');
	const [disbursementResponse] = await Promise.all([
		page.waitForResponse(
			(r) =>
				r.url().endsWith('/api/social-aid/disbursements') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Ödemeyi Kaydet' }).click()
	]);
	if (!disbursementResponse.ok()) {
		fail(`disbursement → ${disbursementResponse.status()}: ${await disbursementResponse.text()}`);
	}
	const disbursement = await disbursementResponse.json();
	await page.getByText('E2E Öğrenci Ailesi').first().waitFor();

	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '58000.00') {
		fail(`balance after disbursement: ${cashDetail.balance}`);
	}
	moves = await apiGet(`/api/financial-accounts/${cash.id}/movements?pageSize=50`);
	const aidMoves = moves.items.filter((m) => m.sourceType === 'social_aid_disbursement');
	if (aidMoves.length !== 1 || aidMoves[0].direction !== 'outflow') {
		fail(`disbursement provenance: ${JSON.stringify(moves.items.map((m) => m.sourceType))}`);
	}
	fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	if (fundDetail.available !== '8000.00' || fundDetail.totalDisbursed !== '12000.00') {
		fail(`fund availability after disbursement: ${fundDetail.available}`);
	}
	summary = await apiGet('/api/income-expense/summary');
	if (summary.expenseTotal !== '0.00') {
		fail(`aid leaked into operational expense: ${JSON.stringify(summary)}`);
	}
	log('yardım = tek social_aid_disbursement çıkışı; gider özeti DEĞİŞMEDİ; tahsisli 8.000');

	// --- 7. Over-disbursement rejected on BOTH dimensions -----------------
	// Fund restricted: 8.000 in cash — 30.000 aid must fail even though
	// the account physically holds 58.000 (restricted ≠ physical).
	response = await apiPost('/api/social-aid/disbursements', {
		fundId: fund.id,
		beneficiaryDisplayName: 'X',
		financialAccountId: cash.id,
		amount: '30000.00',
		occurredAt: new Date(Date.now() - 60000).toISOString(),
		reason: 'aşım denemesi',
		idempotencyKey: 'e2e-013-over'
	});
	if (response.status() !== 409) {
		fail(`over-disbursement accepted: ${response.status()} ${await response.text()}`);
	}
	// Restricted money never lands elsewhere: genel kasa holds 80.000
	// physical but ZERO restricted credit for this fund.
	response = await apiPost('/api/social-aid/disbursements', {
		fundId: fund.id,
		beneficiaryDisplayName: 'X',
		financialAccountId: other.id,
		amount: '5000.00',
		occurredAt: new Date(Date.now() - 60000).toISOString(),
		reason: 'yanlış hesap denemesi',
		idempotencyKey: 'e2e-013-cross'
	});
	if (response.status() !== 409) {
		fail(`cross-account misuse accepted: ${response.status()} ${await response.text()}`);
	}
	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '58000.00') fail(`balance after rejects: ${cashDetail.balance}`);
	log('aşım + yanlış-hesap harcaması REDDEDİLDİ (tahsisli ≠ fiziksel bakiye)');

	// --- 8. UI: disbursement reversal restores both dimensions -----------
	await page.getByRole('button', { name: 'Ödemeyi Ters Kaydet' }).click();
	await page.locator('#rev-dis-reason').fill('Yanlış tutar');
	{
		const buttons = page.getByRole('button', { name: 'Ödemeyi Ters Kaydet' });
		const [reverseResponse] = await Promise.all([
			page.waitForResponse((r) => r.url().includes('/reverse') && r.request().method() === 'POST'),
			buttons.last().click()
		]);
		if (!reverseResponse.ok()) {
			fail(`disbursement reversal → ${reverseResponse.status()}: ${await reverseResponse.text()}`);
		}
	}
	await page.getByText('Yanlış tutar').first().waitFor();
	cashDetail = await apiGet(`/api/financial-accounts/${cash.id}`);
	if (cashDetail.balance !== '70000.00') fail(`balance after reversal: ${cashDetail.balance}`);
	fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	if (fundDetail.available !== '20000.00' || fundDetail.disbursements.length !== 1) {
		fail(`reversal did not restore/history lost: ${fundDetail.available}`);
	}
	log('yardım ters kaydı: fiziksel + tahsisli bakiye GERİ YÜKLENDİ; tarihçe korundu');

	// --- 9. Donation reversal is refused while restricted money consumed --
	response = await apiPost('/api/social-aid/disbursements', {
		fundId: fund.id,
		beneficiaryDisplayName: 'E2E Aile 2',
		financialAccountId: cash.id,
		amount: '8000.00',
		occurredAt: new Date(Date.now() - 60000).toISOString(),
		reason: 'Gıda desteği',
		idempotencyKey: 'e2e-013-aid-2'
	});
	if (!response.ok()) fail(`second disbursement: ${await response.text()}`);
	// Restricted left: 12.000 — reversing the 20.000 donation would
	// invalidate consumed restricted money.
	response = await apiPost(`/api/social-aid/donations/${donation.id}/reverse`, {
		reason: 'İade denemesi'
	});
	if (response.status() !== 409) {
		fail(`consumed donation reversal accepted: ${response.status()}`);
	}
	log('tüketilmiş tahsisli para içeren bağış ters kaydı REDDEDİLDİ');

	// --- 10. Fund close gated on zero restricted availability -------------
	response = await apiPost(`/api/social-aid/funds/${fund.id}/close`, {});
	if (response.status() !== 409) {
		fail(`close with availability accepted: ${response.status()}`);
	}
	// Spend the remainder (12.000), then close succeeds and blocks
	// further postings.
	response = await apiPost('/api/social-aid/disbursements', {
		fundId: fund.id,
		beneficiaryDisplayName: 'E2E Aile 3',
		financialAccountId: cash.id,
		amount: '12000.00',
		occurredAt: new Date(Date.now() - 60000).toISOString(),
		reason: 'Kalan tahsisin dağıtımı',
		idempotencyKey: 'e2e-013-aid-3'
	});
	if (!response.ok()) fail(`final disbursement: ${await response.text()}`);
	response = await apiPost(`/api/social-aid/funds/${fund.id}/close`, {});
	if (!response.ok()) fail(`close at zero: ${response.status()} ${await response.text()}`);
	response = await apiPost('/api/social-aid/donations', {
		fundId: fund.id,
		donorDisplayName: 'X',
		financialAccountId: cash.id,
		amount: '100.00',
		occurredAt: new Date(Date.now() - 60000).toISOString(),
		idempotencyKey: 'e2e-013-after-close'
	});
	if (response.status() !== 400) {
		fail(`donation to closed fund accepted: ${response.status()}`);
	}
	fundDetail = await apiGet(`/api/social-aid/funds/${fund.id}`);
	if (fundDetail.status !== 'closed' || fundDetail.available !== '0.00') {
		fail(`closed fund state: ${fundDetail.status} / ${fundDetail.available}`);
	}
	log('fon kapanışı: tahsisli bakiye >0 iken RED; sıfırda kapanır; sonrası kayıt kapalı');

	// --- 11. Register + permission-gated UI -------------------------------
	const donations = await apiGet(`/api/social-aid/donations?fundId=${fund.id}`);
	if (donations.totalCount !== 1 || donations.items[0].status !== 'posted') {
		fail(`donation register: ${JSON.stringify(donations)}`);
	}
	const disbursements = await apiGet(`/api/social-aid/disbursements?fundId=${fund.id}`);
	if (disbursements.totalCount !== 3) {
		fail(`disbursement register: ${disbursements.totalCount}`);
	}
	const reversed = disbursements.items.find((d) => d.id === disbursement.id);
	if (!reversed || reversed.status !== 'reversed' || reversed.movementStatus !== 'reversed') {
		fail(`reversed history: ${JSON.stringify(reversed)}`);
	}
	await page.goto(`${WEB}/sosyal-yardim`, { waitUntil: 'networkidle' });
	await page.getByText('E2E Eğitim Fonu').first().waitFor();
	await page.locator('table').getByText('Kapatıldı').first().waitFor();
	await page.goto(`${WEB}/sosyal-yardim/${fund.id}`, { waitUntil: 'networkidle' });
	if (await page.getByRole('button', { name: 'Bağış Kaydet' }).isVisible().catch(() => false)) {
		fail('closed fund must not offer new donation');
	}
	log('kayıtlar, ters-kayıt tarihçesi ve kapalı fon eylem kapanışı doğrulandı');

	await browser.close();
	log('PASS');
} catch (error) {
	fail(error?.message ?? String(error));
} finally {
	await shutdown();
}
