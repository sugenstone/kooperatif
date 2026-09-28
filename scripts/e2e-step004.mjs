// STEP-004 focused E2E (§79): a deterministic browser workflow through
// the real stack (PostgreSQL + Rust API + SvelteKit dev server).
//
//   admin logs in → opens Hissedarlar → creates Family+Guardian+Shareholder
//   → sees shareholder in list → opens detail → navigates to Family
//   → sees the shareholder as a family member
//
// Run: node scripts/e2e-step004.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '\$1:').replace(/\/$/, '');
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
spawnSync('docker', [
	'compose', '-f', `${ROOT}/docker/compose.yaml`, 'up', '-d', 'postgres'
], { stdio: 'pipe' });
await waitFor('http://localhost:5494/nope').catch(() => {}); // port open check
for (let i = 0; i < 30; i++) {
	const ready = spawnSync('docker', [
		'compose', '-f', `${ROOT}/docker/compose.yaml`, 'exec', '-T', 'postgres',
		'pg_isready', '-U', 'kooperatif'
	], { stdio: 'pipe' });
	if (ready.status === 0) break;
	await sleep(1000);
}
spawnSync('docker', [
	'compose', '-f', `${ROOT}/docker/compose.yaml`, 'exec', '-T', 'postgres',
	'psql', '-U', 'kooperatif', '-d', 'postgres', '-c',
	'DROP DATABASE IF EXISTS kooperatif_e2e WITH (FORCE);', '-c', 'CREATE DATABASE kooperatif_e2e;'
], { stdio: 'pipe' });

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
	spawnSync('cargo', ['run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--', ...args], {
		env: serverEnv,
		stdio: 'pipe',
		encoding: 'utf8'
	});
let result = cargo(['migrate']);
if (result.status !== 0) fail(`migrate: ${result.stderr}`);
{
	const { execFileSync } = await import('node:child_process');
	try {
		execFileSync('cargo', [
			'run', '--manifest-path', `${ROOT}/apps/server/Cargo.toml`, '--quiet', '--',
			'create-user', '--username', 'e2eadmin', '--display-name', 'E2E Yoneticisi', '--password-stdin'
		], { env: serverEnv, input: 'e2e-parola-cok-gizli-1', stdio: ['pipe', 'pipe', 'pipe'] });
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
process.on('SIGINT', async () => { await shutdown(); process.exit(1); });

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	// --- 3. The browser workflow ----------------------------------------
	const browser = await chromium.launch({ headless: true });
	const page = await browser.newPage({ locale: 'tr-TR' });
	page.on('console', (msg) => {
		if (process.env.E2E_VERBOSE && msg.type() === 'error') {
			console.error(`[browser] ${msg.text()}`);
		}
	});
	page.on('requestfailed', (request) => {
		if (process.env.E2E_VERBOSE) {
			console.error(`[browser] request failed: ${request.url()} ${request.failure()?.errorText}`);
		}
	});

	// Login. Clicking before Svelte attaches the submit handler performs a
	// native GET submit, and early `fill` can precede `bind:value` wiring —
	// so retry the whole flow until the URL leaves /login (bounded).
	let loggedIn = false;
	for (let attempt = 1; attempt <= 6 && !loggedIn; attempt++) {
		await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
		await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
		await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
		await page.getByRole('button', { name: 'Giriş Yap' }).click();
		try {
			await page.waitForURL((url) => !url.pathname.startsWith('/login'), {
				timeout: 8000
			});
			loggedIn = true;
		} catch {
			log(`login attempt ${attempt} did not navigate (url=${page.url()})`);
		}
	}
	if (!loggedIn) throw new Error('login never completed');

	// Open Hissedarlar
	await page.getByRole('link', { name: 'Hissedarlar' }).click();
	await page.waitForURL('**/hissedarlar');

	// Create shareholder: new person + new guardian + new family
	const seq = Math.floor(2000 + Math.random() * 7000);
	await page.getByRole('link', { name: 'Yeni Hissedar' }).click();
	await page.getByLabel('Ad', { exact: true }).fill('Mehmet');
	await page.getByLabel('Soyad', { exact: true }).fill('Yılmaz');
	await page.getByRole('button', { name: 'Yeni vasi' }).click();
	await page.locator('#guardian-first').fill('Hasan');
	await page.locator('#guardian-last').fill('Yılmaz');
	await page.locator('#family-seq').fill(String(seq));
	await page.getByRole('button', { name: 'Hissedarı Oluştur' }).click();
	await page.waitForURL('**/hissedarlar');

	// Search and open the detail
	await page.getByRole('searchbox').fill('mehmet yılmaz');
	await page.getByRole('button', { name: 'Ara' }).click();
	await page.getByRole('link', { name: 'Mehmet Yılmaz' }).first().click();
	await page.waitForURL('**/hissedarlar/**');
	const detailLabel = await page.locator('h1').first().textContent();
	if (!detailLabel?.includes('Vasi: Hasan Yılmaz') || !detailLabel.includes(`Aile No ${seq}`)) {
		fail(`detail label missing context: ${detailLabel}`);
	} else {
		log(`detail shows canonical identity: ${detailLabel.trim()}`);
	}

	// Navigate to the family and see the member
	await page.getByRole('link', { name: `Aile No ${seq}` }).click();
	await page.waitForURL('**/aileler/**');
	await page.getByRole('link', { name: 'Mehmet Yılmaz' }).waitFor();
	log('family detail lists the shareholder as a member');

	// Same-name distinction context is visible in family members table
	const guardianCell = await page.getByText('Hasan Yılmaz').first().textContent();
	if (!guardianCell) fail('guardian context missing in family detail');

	await browser.close();
	if (!process.exitCode) log('E2E PASSED');
} catch (error) {
	fail(error.message);
} finally {
	await shutdown();
}
