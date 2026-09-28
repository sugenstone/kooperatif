// STEP-006 focused E2E: a deterministic browser workflow through the
// real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → fixture shareholders + a Share (API) → UI creates a
//   Period with a per_share rule → runs the read-only Önizleme →
//   finalizes via Tahakkukları Oluştur (confirm) → sees the durable
//   Tahakkuk list → opens the obligation detail (per-Share provenance)
//   → sees the shareholder's Tahakkuklar section → closes the Period
//
// Financial boundary asserted: obligations exist; no payment surface.
// Run: node scripts/e2e-step006.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8098';
const WEB = 'http://localhost:5198';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_006';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_006 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_006;'
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

	// --- 3. API-side fixtures: shareholder + share -------------------------
	const browser = await chromium.launch({ headless: true });
	const page = await browser.newPage({ locale: 'tr-TR' });
	page.on('console', (msg) => {
		if (process.env.E2E_VERBOSE && msg.type() === 'error') {
			console.error(`[browser] ${msg.text()}`);
		}
	});
	// confirm() dialogs (finalize / close) are accepted automatically.
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

	const seqA = Math.floor(2000 + Math.random() * 7000);
	const holderA = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ali', lastName: 'Veli' },
		guardian: { mode: 'new', firstName: 'Hasan', lastName: 'Veli' },
		family: { mode: 'new', sequenceNumber: seqA }
	});
	await apiPost('/api/shares', {
		shareholderId: holderA.id,
		acquisitionType: 'founder'
	});
	log(`fixture: Ali Veli (aile ${seqA}) holds share 1`);

	// UI needs a session cookie in the browser context.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	// --- 4. UI workflow: create a Period -----------------------------------
	await page.goto(`${WEB}/donemler`, { waitUntil: 'networkidle' });
	await page.getByRole('link', { name: 'Yeni Dönem' }).click();
	await page.waitForURL('**/donemler/yeni');

	await page.getByLabel('Dönem Adı').fill('2026 Ekim Dönemi');
	await page.getByLabel('Tahsilat Başlangıcı').fill('2026-10-01');
	await page.getByLabel('Son Ödeme Tarihi').fill('2026-10-15');
	await page.getByRole('button', { name: 'Hisse Başına' }).click();
	await page.getByLabel('Tahakkuk Tutarı').fill('250,00');
	await page.getByLabel('Tahakkuk Geçerlilik Tarihi').fill('2026-10-01');
	await page.getByRole('button', { name: 'Dönemi Oluştur' }).click();
	await page.waitForURL('**/donemler/*');
	await page.getByText('Taslak').waitFor();
	log('period created as draft');

	// --- 5. Read-only preview -----------------------------------------------
	await page.getByRole('button', { name: 'Önizlemeyi Çalıştır' }).click();
	const rowLabel = `Ali Veli · Vasi: Hasan Veli · Aile No ${seqA}`;
	await page.getByText(rowLabel).waitFor();
	await page.getByText('250,00 ₺').first().waitFor();
	log('preview shows canonical identity + expected amount, persists nothing');

	// --- 6. Finalize ---------------------------------------------------------
	await page.getByRole('button', { name: 'Tahakkukları Oluştur' }).click();
	await page.getByText('Açık').waitFor();
	await page.getByRole('link', { name: rowLabel }).waitFor();
	log('finalization produced durable obligations (period open)');

	// Double-finalization is impossible: the command surface is gone.
	if (await page.getByRole('button', { name: 'Tahakkukları Oluştur' }).count()) {
		fail('generate command still visible after finalization');
	}

	// --- 7. Obligation detail: per-Share provenance --------------------------
	await page.getByRole('link', { name: rowLabel }).click();
	await page.waitForURL('**/tahakkuklar/*');
	await page.getByText('Kaynak Hisseler').waitFor();
	await page.getByRole('link', { name: '1', exact: true }).waitFor();
	log('assessment detail pins source share 1');

	// --- 8. Shareholder Tahakkuklar section ----------------------------------
	await page.goto(`${WEB}/hissedarlar/${holderA.id}`, { waitUntil: 'networkidle' });
	await page.getByText('Tahakkuklar').first().waitFor();
	await page.getByText('250,00 ₺').waitFor();
	log('shareholder detail shows the obligation');

	// --- 9. Close the Period ---------------------------------------------------
	await page.goto(`${WEB}/donemler`, { waitUntil: 'networkidle' });
	await page.getByRole('link', { name: '1', exact: true }).click();
	await page.waitForURL('**/donemler/*');
	await page.getByRole('button', { name: 'Dönemi Kapat' }).click();
	await page.getByText('Kapalı').waitFor();
	log('period closed; obligations remain readable');
	await page.getByRole('link', { name: rowLabel }).waitFor();

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
