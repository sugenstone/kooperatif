// HOTFIX-001 focused E2E: Payment-detail allocation prefill on the real
// stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture cash account + family + shareholder + open
//   assessment (remaining 300,00) → a 500,00 Payment posts with NO
//   allocation → UI "Dağılım Ekle" → debtor selected → the editable
//   tr-TR amount field MUST show "300,00" (canonical "300.00" would
//   parse back as 30000.00) → submit WITHOUT touching the field →
//   the server stores exactly 300.00 and the assessment settles.
//
// Asserts the representation boundary: API canonical "300.00" must
// become editable "300,00" (canonicalToTryInput) and round-trip back
// to canonical "300.00" — never "30000.00".
// Run: node scripts/e2e-hotfix001.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8097';
const WEB = 'http://localhost:5197';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_h001';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_h001 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_h001;'
	],
	{ stdio: 'pipe' }
);

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
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port 5197 --strictPort`, {
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
	const apiGet = async (path) => {
		const response = await page.request.get(`${API}${path}`, {
			headers: { origin: WEB }
		});
		if (!response.ok()) {
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		}
		return response.json();
	};

	// --- 3. Fixtures: cash account, family, member, one open assessment --
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
	const period = await apiPost('/api/periods', {
		name: 'Tahsilat Dönemi',
		collectionStartDate: '2026-01-01',
		dueDate: '2026-01-31',
		ruleType: 'per_shareholder',
		baseAmount: '300.00',
		assessmentEffectiveDate: '2026-01-01'
	});
	await apiPost(`/api/periods/${period.id}/generate-assessments`, null);
	const open = await apiGet(`/api/shareholders/${member.id}/open-assessments`);
	if (open.length !== 1 || open[0].remainingAmount !== '300.00') {
		fail(`open assessments: ${JSON.stringify(open)}`);
	}
	const assessmentId = open[0].id;

	// A posted payment with the full amount still unallocated.
	const created = await apiPost('/api/payments', {
		payerFirstName: 'Ödeyen',
		payerLastName: 'Üçüncü Kişi',
		amount: '500.00',
		method: 'cash',
		destinationAccountId: cash.id,
		idempotencyKey: crypto.randomUUID(),
		allocations: []
	});
	const payment = created.payment;
	log(`fixture: payment ${payment.id} (500,00 unallocated), assessment ${assessmentId} (300,00 open)`);

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// --- 4. Payment detail → Dağılım Ekle → prefilled amount --------------
	await page.goto(`${WEB}/tahsilatlar/${payment.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Kayıtlı').first().waitFor();
	await page.getByRole('button', { name: 'Dağılım Ekle' }).click();
	await page.getByPlaceholder('Hissedar ara (ad, vasi, aile no)').fill('Ali');
	await page.getByRole('button', { name: 'Ara', exact: true }).click();
	await page.getByRole('button', { name: /Ali Aile/ }).click();

	const amountInput = page.locator('#alloc-amount');
	await amountInput.waitFor();
	const prefilled = await amountInput.inputValue();
	if (prefilled !== '300,00') {
		fail(`prefilled editable value: "${prefilled}" — expected "300,00"`);
	}
	log('prefill correct: canonical "300.00" → editable "300,00"');

	// Submit WITHOUT rewriting the amount; the captured request must
	// carry canonical "300.00" — if the raw canonical string had been
	// prefilled, tr-TR parsing would produce "30000.00".
	const [allocResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes('/allocations') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Dağılımı Kaydet' }).click()
	]);
	const postedBody = allocResponse.request().postDataJSON();
	if (postedBody?.allocations?.[0]?.amount !== '300.00') {
		fail(`request amount: ${JSON.stringify(postedBody)} — expected 300.00, NOT 30000.00`);
	}
	if (!allocResponse.ok()) {
		fail(`allocations POST → ${allocResponse.status()}: ${await allocResponse.text()}`);
	}
	log('request carried canonical "300.00" — never "30000.00"');

	// --- 5. Server truth: the assessment is now fully settled -------------
	const after = await apiGet(`/api/shareholders/${member.id}/open-assessments`);
	const settled = after.find((a) => a.id === assessmentId);
	if (!settled || settled.remainingAmount !== '0.00' || settled.paidAmount !== '300.00') {
		fail(`assessment not settled: ${JSON.stringify(after)}`);
	}
	const detail = await apiGet(`/api/payments/${payment.id}`);
	const line = detail.allocations.find((a) => a.assessmentId === assessmentId);
	if (!line || line.amount !== '300.00' || line.status !== 'active') {
		fail(`stored allocation: ${JSON.stringify(line)}`);
	}
	if (detail.unallocatedAmount !== '200.00') {
		fail(`unallocated after allocation: ${detail.unallocatedAmount}`);
	}
	log('server truth: allocation 300.00 active, assessment settled, remainder 200.00');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
