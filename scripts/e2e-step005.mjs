// STEP-005 focused E2E (§78): a deterministic browser workflow through
// the real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → creates a Share via the UI for an existing
//   shareholder → sees the canonical owner identity on /hisseler →
//   opens share detail → sells it to a second shareholder with an
//   agreed amount → sees the new owner + append-only event history →
//   verifies the shareholder detail "Hisseler" sections
//
// Run: node scripts/e2e-step005.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8097';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e;'
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
const web = spawn(`pnpm --dir "${ROOT}/apps/web" dev --port 5199 --strictPort`, {
	env: { ...process.env, PUBLIC_API_BASE_URL: API },
	stdio: 'pipe',
	shell: true
});
web.stderr.on('data', (d) => process.env.E2E_VERBOSE && process.stderr.write(d));

const killTree = (child) => {
	if (!child?.pid) return;
	if (process.platform === 'win32') {
		// shell:true spawns cmd.exe which orphans the real cargo/vite
		// child on a bare kill(); taskkill /T removes the whole tree.
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

	// --- 3. API-side setup: two shareholders (UI-agnostic fixtures) -------
	const browser = await chromium.launch({ headless: true });
	const page = await browser.newPage({ locale: 'tr-TR' });
	page.on('console', (msg) => {
		if (process.env.E2E_VERBOSE && msg.type() === 'error') {
			console.error(`[browser] ${msg.text()}`);
		}
	});
	// confirm() dialogs (sale confirmation) are accepted automatically.
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
	const seqB = seqA + 1;
	const holderA = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ali', lastName: 'Veli' },
		guardian: { mode: 'new', firstName: 'Hasan', lastName: 'Veli' },
		family: { mode: 'new', sequenceNumber: seqA }
	});
	const holderB = await apiPost('/api/shareholders', {
		person: { mode: 'new', firstName: 'Ayşe', lastName: 'Demir' },
		family: { mode: 'new', sequenceNumber: seqB }
	});
	log(`fixtures: Ali Veli (aile ${seqA}), Ayşe Demir (aile ${seqB})`);

	// --- 4. UI workflow: create a share ------------------------------------
	await page.goto(`${WEB}/hisseler`, { waitUntil: 'networkidle' });
	await page.getByRole('link', { name: 'Yeni Hisse' }).click();
	await page.waitForURL('**/hisseler/yeni');

	await page.getByRole('button', { name: new RegExp(`Ali Veli.*Aile No ${seqA}`) }).click();
	await page.getByLabel('Edinim Bedeli').fill('50.000,00');
	await page.getByRole('button', { name: 'Hisseyi Oluştur' }).click();
	await page.waitForURL('**/hisseler');

	const rowLabel = `Ali Veli · Vasi: Hasan Veli · Aile No ${seqA}`;
	await page.getByText(rowLabel).waitFor();
	log('share row shows canonical owner identity');

	await page.getByRole('link', { name: '1' }).click();
	await page.waitForURL('**/hisseler/*');
	await page.getByRole('link', { name: rowLabel }).waitFor();
	await page.getByText('Kurucu Tahsisi').waitFor();
	await page.getByText('50.000,00 ₺').waitFor();
	log('share detail: canonical owner + initial acquisition + exact amount');

	// --- 5. UI workflow: sell the share ------------------------------------
	await page.getByRole('button', { name: 'Satış Yap' }).click();
	await page.getByRole('button', { name: new RegExp(`Ayşe Demir.*Aile No ${seqB}`) }).click();
	await page.getByLabel('Satış Bedeli').fill('150.000,50');
	await page.getByRole('button', { name: 'Satış Yap' }).last().click();

	const buyerLabel = `Ayşe Demir · Vasi: Belirtilmemiş · Aile No ${seqB}`;
	await page.getByRole('link', { name: buyerLabel }).waitFor();
	await page.getByText('150.000,50 ₺').waitFor();
	log(`sale recorded; new owner: ${buyerLabel}`);

	// --- 6. Shareholder detail "Hisseler" sections --------------------------
	await page.getByRole('link', { name: buyerLabel }).click();
	await page.waitForURL('**/hissedarlar/*');
	await page.getByRole('link', { name: '1' }).waitFor();
	log('buyer detail lists the share under Hisseler');

	// Seller holds nothing now.
	const sellerResp = await page.request.get(`${API}/api/shareholders/${holderA.id}/shares`);
	const sellerShares = await sellerResp.json();
	if (sellerShares.length !== 0) fail(`seller still holds ${sellerShares.length} share(s)`);
	log('seller holds no open share interval');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
