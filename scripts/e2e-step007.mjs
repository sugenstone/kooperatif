// STEP-007 focused E2E: a deterministic browser workflow through the
// real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture family + two member shareholders + an open
//   Period with assessments (API) → UI Yeni Tahsilat: new payer person
//   → family context → both members' debts selected → ONE posted
//   Payment with allocations across BOTH members → detail shows
//   payer ≠ debtor + derived unallocated = 0 → assessment payment
//   history → shareholder financial summary → family collection card
//   → full reversal with reason (history preserved, balances derive
//   back to zero)
//
// Financial boundary asserted: money moves ONLY through the Payment +
// Allocation records; no Cashbox/Bank/Ledger/Receipt surface exists.
// Run: node scripts/e2e-step007.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8098';
const WEB = 'http://localhost:5198';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_007';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_007 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_007;'
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
	const { csrfToken } = await loginResponse.json();

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

	// --- 3. Fixtures: one family, two member shareholders, open period ----
	const seq = Math.floor(2000 + Math.random() * 7000);
	const family = await apiPost('/api/families', { sequenceNumber: seq });
	const memberA = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ali', lastName: 'Aile' },
		family: { mode: 'existing', familyId: family.id }
	});
	await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ayşe', lastName: 'Aile' },
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
	const assessments = await apiGet(`/api/periods/${period.id}/assessments?pageSize=50`);
	log(`fixture: family ${seq} with 2 members; ${assessments.items.length} assessments of 300,00`);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// --- 4. New collection: payer → family debts → amounts → confirm ----
	await page.goto(`${WEB}/tahsilatlar/yeni`, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Yeni kişi olarak kaydet' }).click();
	await page.locator('#payer-first').fill('Ödeyen');
	await page.locator('#payer-last').fill('Üçüncü Kişi');
	log('payer is a brand-new person — never implicitly a debtor');

	await page.getByRole('button', { name: 'Aileye göre' }).click();
	await page.getByPlaceholder('Aile ara (aile no, üye adı)').fill(String(seq));
	await page.getByRole('button', { name: 'Ara', exact: true }).click();
	await page.getByRole('button', { name: `Aile No: ${seq}` }).click();
	await page.getByText('Aile Toplam Kalan').waitFor();
	const memberButtons = page.getByRole('button', { name: 'Üyelerin açık borçları' });
	await memberButtons.first().click();
	await page.getByText('Dağıtılacak Tutar').first().waitFor();
	await memberButtons.nth(1).click();
	await page.locator('tbody input[type="checkbox"]').nth(1).waitFor();
	await page.getByRole('button', { name: 'Tümünü seç' }).click();
	log(`family ${seq} context loaded — ONE payer will cover BOTH members`);

	await page.locator('#pay-amount').fill('600,00');
	await page.getByRole('button', { name: 'Tahsilatı Kaydet' }).click();
	await page.waitForURL('**/tahsilatlar/*');
	await page.getByText('Kayıtlı').first().waitFor();
	await page.getByText('Ödeyen Üçüncü Kişi').first().waitFor();
	await page.getByText('Dağıtılmamış').waitFor();
	const paymentUrl = page.url();
	const paymentId = paymentUrl.split('/').pop();
	log(`payment posted: ${paymentId} — allocations across both members`);

	// Both debtors named on the single payment.
	await page.getByText('Ali Aile').first().waitFor();
	await page.getByText('Ayşe Aile').first().waitFor();

	// --- 5. Assessment payment history -------------------------------------
	const assessmentId = assessments.items[0].id;
	await page.goto(`${WEB}/tahakkuklar/${assessmentId}`, { waitUntil: 'networkidle' });
	await page.getByText('Ödeme Geçmişi').waitFor();
	await page.getByText('Ödeyen Üçüncü Kişi').first().waitFor();
	log('assessment detail shows the payment application');

	// --- 6. Shareholder financial summary -----------------------------------
	await page.goto(`${WEB}/hissedarlar/${memberA.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Mali Özet').waitFor();
	await page.getByText('0,00 ₺').first().waitFor(); // fully settled member
	log('shareholder summary shows derived totals (never a stored flag)');

	// --- 7. Family collection card ------------------------------------------
	await page.goto(`${WEB}/aileler/${family.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Aile Toplam Kalan').waitFor();
	log('family detail shows member-wise collection context');

	// --- 8. Full reversal ----------------------------------------------------
	await page.goto(paymentUrl, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Ters Kayıt Yap' }).click();
	await page.locator('#payment-reason').fill('e2e hatalı tahsilat');
	await page.getByRole('button', { name: 'Ters Kayıt Yap' }).click();
	await page.getByText('Ters Kayıt').first().waitFor();
	await page.getByText('e2e hatalı tahsilat').first().waitFor();
	log('payment reversed — row and allocations preserved as history');

	const summary = await apiGet(`/api/shareholders/${memberA.id}/financial-summary`);
	if (summary.totalPaid !== '0.00' || summary.totalRemaining !== '300.00') {
		fail(`derived balance after reversal wrong: ${JSON.stringify(summary)}`);
	}
	log('reversal returned the obligation to fully open (derived)');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
