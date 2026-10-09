// STEP-017B focused E2E: responsive layout, navigation shell and
// critical-workflow usability through the real stack
// (PostgreSQL + Rust API + SvelteKit + Chromium).
//
//   fresh DB → migrate → admin login through the REAL login form →
//   canonical fixture (account + family + shareholder + period +
//   assessments + payment) → key routes exercised at
//   360x800 / 390x844 / 768x1024 / 1440x900 → asserts no horizontal
//   page scroll, mobile menu behaviour, sidebar presence, money inputs
//   exposing inputmode=decimal, and screenshots for the report.
//
// Run: node scripts/e2e-step017-uiux.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const SHOTS = `${ROOT}/artifacts/step017`;
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_NAME = 'kooperatif_e2e_017';
const DB_URL = `postgres://kooperatif:kooperatif_dev_password@localhost:5494/${DB_NAME}`;

const log = (msg) => console.log(`[e2e-17b] ${msg}`);
const fail = (msg) => {
	console.error(`[e2e-17b] FAIL: ${msg}`);
	process.exitCode = 1;
};

async function waitFor(url, timeoutMs = 90000) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		try {
			const response = await fetch(url);
			if (response.ok || response.status === 401 || response.status === 404) return true;
		} catch {}
		await sleep(500);
	}
	throw new Error(`timeout waiting for ${url}`);
}

const compose = (args) =>
	spawnSync('docker', ['compose', '-f', `${ROOT}/docker/compose.yaml`, ...args], {
		stdio: 'pipe',
		encoding: 'utf8'
	});

log('preparing database');
compose(['up', '-d', 'postgres']);
for (let i = 0; i < 30; i++) {
	if (compose(['exec', '-T', 'postgres', 'pg_isready', '-U', 'kooperatif']).status === 0) break;
	await sleep(1000);
}
compose([
	'exec',
	'-T',
	'postgres',
	'psql',
	'-U',
	'kooperatif',
	'-d',
	'postgres',
	'-c',
	`DROP DATABASE IF EXISTS ${DB_NAME} WITH (FORCE);`,
	'-c',
	`CREATE DATABASE ${DB_NAME};`
]);

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
		{ env: serverEnv, stdio: 'pipe', encoding: 'utf8' }
	);
let result = cargo(['migrate']);
if (result.status !== 0) fail(`migrate: ${result.stderr}`);
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
	fail(`create-user: ${error.stderr?.toString()}`);
}
result = cargo(['grant-role', '--username', 'e2eadmin', '--role', 'Sistem Yöneticisi']);
if (result.status !== 0) fail(`grant-role: ${result.stderr}`);

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

mkdirSync(SHOTS, { recursive: true });

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	const browser = await chromium.launch({ headless: true });
	const seedCtx = await browser.newContext({ locale: 'tr-TR' });
	const seed = await seedCtx.newPage();
	const loginResponse = await seed.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
	});
	if (!loginResponse.ok()) throw new Error(`admin login: ${loginResponse.status()}`);
	const csrf = (await loginResponse.json()).csrfToken;
	const apiPost = async (path, body) =>
		seed.request.post(`${API}${path}`, {
			headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
			data: body
		});

	// Fixture: enough rows that list screens render real content.
	const kasa = (await (await apiPost('/api/financial-accounts', { name: 'E2E Kasa', accountType: 'cash' })).json()).id;
	for (let i = 1; i <= 8; i++) {
		const r = await apiPost('/api/shareholders', {
			person: { mode: 'new', firstName: `E2E Üye${i}`, lastName: 'Soyadı' },
			family: { mode: 'new', sequenceNumber: 800 + i },
			idempotencyKey: `e2e-017-sh-${i}`
		});
		if (!r.ok()) fail(`shareholder ${i}: ${await r.text()}`);
	}
	const period = await (
		await apiPost('/api/periods', {
			name: 'E2E-017 Dönem',
			collectionStartDate: '2026-03-01',
			dueDate: '2026-03-31',
			ruleType: 'per_shareholder',
			baseAmount: '250.00',
			assessmentEffectiveDate: '2026-03-01',
			idempotencyKey: 'e2e-017-period'
		})
	).json();
	const gen = await apiPost(`/api/periods/${period.id}/generate-assessments`, {
		idempotencyKey: 'e2e-017-gen'
	});
	if (!gen.ok()) fail(`generate: ${await gen.text()}`);
	const assessments = (
		await (
			await seed.request.get(`${API}/api/periods/${period.id}/assessments?pageSize=100`, {
				headers: { origin: WEB }
			})
		).json()
	).items;
	const pay = await apiPost('/api/payments', {
		payerFirstName: 'Veli',
		payerLastName: 'Ödeyen',
		amount: '250.00',
		method: 'cash',
		destinationAccountId: kasa,
		allocations: [{ assessmentId: assessments[0].id, amount: '250.00' }],
		idempotencyKey: 'e2e-017-pay-1'
	});
	if (!pay.ok()) fail(`payment: ${await pay.text()}`);
	log('fixture ready');

	const routes = [
		['home', '/'],
		['hissedarlar', '/hissedarlar'],
		['aileler', '/aileler'],
		['hisseler', '/hisseler'],
		['donemler', '/donemler'],
		['tahsilatlar', '/tahsilatlar'],
		['tahsilat-yeni', '/tahsilatlar/yeni'],
		['finansal-hesaplar', '/finansal-hesaplar'],
		['raporlar', '/raporlar']
	];
	const viewports = [
		[360, 800],
		[390, 844],
		[768, 1024],
		[1440, 900]
	];

	for (const [vw, vh] of viewports) {
		log(`viewport ${vw}x${vh}`);
		const ctx = await browser.newContext({
			locale: 'tr-TR',
			viewport: { width: vw, height: vh }
		});
		const page = await ctx.newPage();

		// Real login through the form.
		await page.goto(`${WEB}/login`, { waitUntil: 'domcontentloaded' });
		await page.waitForLoadState('networkidle');
		await page.fill('#username', 'e2eadmin');
		await page.fill('#password', 'e2e-parola-cok-gizli-1');
		await page.getByRole('button', { name: 'Giriş Yap' }).click();
		await page.waitForURL(`${WEB}/`, { timeout: 30000 });

		for (const [name, route] of routes) {
			await page.goto(`${WEB}${route}`, { waitUntil: 'domcontentloaded' });
			await page.waitForSelector('#main-content', { timeout: 15000 });
			await page.waitForTimeout(400);
			const overflow = await page.evaluate(
				() =>
					document.documentElement.scrollWidth -
					document.documentElement.clientWidth
			);
			if (overflow > 1)
				fail(`${route} @${vw}px: horizontal page overflow ${overflow}px`);
			await page.screenshot({ path: `${SHOTS}/${vw}x${vh}-${name}.png` });
		}

		// Navigation behaviour per breakpoint.
		const menuButton = page.getByRole('button', { name: 'Menü' });
		if (vw < 768) {
			if (!(await menuButton.isVisible())) fail(`${vw}px: mobile menu button missing`);
			await menuButton.click();
			// shadcn sidebar renders an off-canvas dialog on mobile.
			const nav = page.getByRole('dialog');
			if (!(await nav.isVisible())) fail(`${vw}px: mobile menu did not open`);
			// Navigate via mobile menu.
			await nav.getByRole('link', { name: 'Dönemler' }).click();
			await page.waitForURL(`${WEB}/donemler`, { timeout: 15000 });
		} else {
			// The trigger also collapses the desktop sidebar to icons —
			// presence is expected; the sidebar itself must stay visible.
			const aside = page.locator('[data-slot="sidebar-inner"]');
			if (!(await aside.isVisible())) fail(`${vw}px: sidebar navigation missing`);
		}

		await ctx.close();
	}

	// Money inputs must offer a decimal keypad on touch devices.
	const checkCtx = await browser.newContext({ locale: 'tr-TR', viewport: { width: 390, height: 844 } });
	const cp = await checkCtx.newPage();
	await cp.goto(`${WEB}/login`, { waitUntil: 'domcontentloaded' });
	await cp.waitForLoadState('networkidle');
	await cp.fill('#username', 'e2eadmin');
	await cp.fill('#password', 'e2e-parola-cok-gizli-1');
	await cp.getByRole('button', { name: 'Giriş Yap' }).click();
	await cp.waitForURL(`${WEB}/`, { timeout: 30000 });
	for (const [route, selector] of [
		['/tahsilatlar/yeni', '#pay-amount'],
		['/gelirler/yeni', '#inc-amount'],
		['/giderler/yeni', '#exp-amount'],
		['/transferler/yeni', '#tr-amount']
	]) {
		await cp.goto(`${WEB}${route}`, { waitUntil: 'domcontentloaded' });
		const mode = await cp.getAttribute(selector, 'inputmode');
		if (mode !== 'decimal') fail(`${route} ${selector}: inputmode=${mode}, expected decimal`);
	}
	await checkCtx.close();

	await browser.close();
	await shutdown();
	if (process.exitCode) console.log('[e2e-17b] FAILURES — see above');
	else console.log('[e2e-17b] PASS');
} catch (error) {
	await shutdown();
	console.error('[e2e-17b] ERROR', error);
	process.exit(1);
}
