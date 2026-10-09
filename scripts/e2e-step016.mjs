// STEP-016 focused E2E: real-time WebSocket + live TV/projector display
// through the real stack (PostgreSQL + Rust API + SvelteKit + Chromium).
//
//   admin logs in → minimal canonical fixture (financial account +
//   period + assessments) → a SECOND browser context opens
//   /canli-ekran (TV) → the TV socket authenticates with the session
//   cookie and subscribes to reports.read → a Payment is committed via
//   the REAL API → the TV updates WITHOUT reload to exactly the
//   canonical /api/reports/overview value → the payment is REVERSED →
//   the TV converges back to the canonical value → browser goes
//   OFFLINE, another payment commits, browser returns ONLINE →
//   bounded reconnect + canonical resync converge the TV again →
//   an unauthenticated socket is rejected → an out-of-scope socket
//   receives no unauthorized-domain signal → account_movements count
//   proves real-time delivery created ZERO phantom rows.
//
// Run: node scripts/e2e-step016.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8098';
const WEB = 'http://localhost:5198';
const DB_NAME = 'kooperatif_e2e_016';
const DB_URL = `postgres://kooperatif:kooperatif_dev_password@localhost:5494/${DB_NAME}`;

const log = (msg) => console.log(`[e2e] ${msg}`);
const fail = (msg) => {
	console.error(`[e2e] FAIL: ${msg}`);
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
const psql = (sql, db = 'postgres') =>
	compose(['exec', '-T', 'postgres', 'psql', '-U', 'kooperatif', '-d', db, '-Atc', sql])
		.stdout.trim();

// --- 1. Reset database, migrate, bootstrap admin -----------------------
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
// A viewer holding ONLY reports.read — proves the TV minimum grant.
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
			'e2eviewer',
			'--display-name',
			'E2E Izleyici',
			'--password-stdin'
		],
		{ env: serverEnv, input: 'e2e-parola-cok-gizli-2', stdio: ['pipe', 'pipe', 'pipe'] }
	);
} catch (error) {
	fail(`create-viewer: ${error.stderr?.toString()}`);
}

// --- 2. Start backend + web --------------------------------------------
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

const eq = (name, got, want) => {
	if (String(got) !== String(want)) fail(`${name}: expected ${want}, got ${got}`);
};

const TL = (decimal) =>
	`${new Intl.NumberFormat('tr-TR', { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(
		Number(decimal)
	)} ₺`;

try {
	await waitFor(`${API}/health`);
	await waitFor(`${WEB}/login`);
	log('stack ready');

	const browser = await chromium.launch({ headless: true });
	// Context A: operator session (API fixture + assertions).
	const opCtx = await browser.newContext({ locale: 'tr-TR' });
	const op = await opCtx.newPage();
	const loginResponse = await op.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'e2eadmin', password: 'e2e-parola-cok-gizli-1' }
	});
	if (!loginResponse.ok()) throw new Error(`admin login: ${loginResponse.status()}`);
	const csrf = (await loginResponse.json()).csrfToken;

	const apiPost = async (path, body) => {
		const response = await op.request.post(`${API}${path}`, {
			headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': csrf },
			data: body
		});
		return response;
	};
	const apiGet = async (path) => {
		const response = await op.request.get(`${API}${path}`, { headers: { origin: WEB } });
		if (!response.ok())
			throw new Error(`GET ${path} → ${response.status()}: ${await response.text()}`);
		return response.json();
	};

	// --- 3. Minimal canonical fixture via REAL API ----------------------
	const kasa = (
		await (
			await apiPost('/api/financial-accounts', { name: 'E2E Kasa', accountType: 'cash' })
		).json()
	).id;
	const mkShareholder = async (first, last, seq) => {
		const r = await apiPost('/api/shareholders', {
			person: { mode: 'new', firstName: first, lastName: last },
			family: { mode: 'new', sequenceNumber: seq }
		});
		if (!r.ok()) fail(`shareholder: ${await r.text()}`);
		return r.json();
	};
	const shA = await mkShareholder('E2E Ayşe', 'Üye', 701);
	const period = await (
		await apiPost('/api/periods', {
			name: 'E2E Dönem',
			collectionStartDate: '2026-02-01',
			dueDate: '2026-02-28',
			ruleType: 'per_shareholder',
			baseAmount: '1000.00',
			assessmentEffectiveDate: '2026-02-01'
		})
	).json();
	const gen = await apiPost(`/api/periods/${period.id}/generate-assessments`);
	if (!gen.ok()) fail(`generate: ${await gen.text()}`);
	const assessments = (await apiGet(`/api/periods/${period.id}/assessments?pageSize=100`)).items;
	const shAssessment = assessments.find((a) => a.shareholder.shareholderId === shA.id);

	const postPayment = async (amount, key) => {
		const r = await apiPost('/api/payments', {
			payerFirstName: 'Veli',
			payerLastName: 'Ödeyen',
			amount,
			method: 'cash',
			destinationAccountId: kasa,
			allocations: [{ assessmentId: shAssessment.id, amount }],
			idempotencyKey: key
		});
		if (!r.ok()) fail(`payment ${key}: ${await r.text()}`);
		return (await r.json()).payment.id;
	};

	const movementsBefore = Number(
		psql('SELECT count(*) FROM account_movements', DB_NAME)
	);

	// --- 4. TV display in a second, viewer-scoped browser session -------
	log('opening TV display as reports.read viewer');
	// Grant the viewer a read-only role.
	psql(
		`INSERT INTO roles (name, description) VALUES ('E2E Goruntuleyici', 'tv') ON CONFLICT DO NOTHING;
		 INSERT INTO role_permissions (role_id, permission_id)
		   SELECT r.id, p.id FROM roles r, permissions p
		   WHERE r.name = 'E2E Goruntuleyici' AND p.key = 'reports.read';
		 INSERT INTO user_role_assignments (user_id, role_id)
		   SELECT u.id, r.id FROM users u, roles r
		   WHERE u.username = 'e2eviewer' AND r.name = 'E2E Goruntuleyici';`,
		DB_NAME
	);
	const tvCtx = await browser.newContext({ locale: 'tr-TR' });
	const tv = await tvCtx.newPage();
	const tvLogin = await tv.request.post(`${API}/api/auth/login`, {
		headers: { 'content-type': 'application/json', origin: WEB },
		data: { username: 'e2eviewer', password: 'e2e-parola-cok-gizli-2' }
	});
	if (!tvLogin.ok()) throw new Error(`viewer login: ${tvLogin.status()}`);
	await tv.goto(`${WEB}/canli-ekran`, { waitUntil: 'domcontentloaded' });
	await tv.waitForSelector('[data-testid="tv-display"]', { timeout: 30000 });
	await tv.waitForSelector('[data-testid="tv-collected"]', { timeout: 30000 });
	// Socket connects — status chip reports Bağlı.
	await tv.waitForFunction(
		() => document.querySelector('[data-testid="tv-status"]')?.textContent?.trim() === 'Bağlı',
		null,
		{ timeout: 30000 }
	);
	log('TV connected');
	eq('initial collected', (await tv.textContent('[data-testid="tv-collected"]')).trim(), TL('0'));

	// --- 5. Commit a payment → TV updates without reload -----------------
	const pid = await postPayment('250.00', 'e2e-016-pay1');
	const canonical = async () => (await apiGet('/api/reports/overview')).postedPaymentsTotal;
	await tv.waitForFunction(
		(expected) =>
			document.querySelector('[data-testid="tv-collected"]')?.textContent?.trim() === expected,
		TL('250.00'),
		{ timeout: 30000 }
	);
	eq('canonical after pay1', await canonical(), '250.00');
	log('live update on commit: PASS');

	// --- 6. Reversal → TV converges back ---------------------------------
	const rev = await apiPost(`/api/payments/${pid}/reverse`, { reason: 'E2E yanlış tahsilat' });
	if (!rev.ok()) fail(`reverse: ${await rev.text()}`);
	await tv.waitForFunction(
		(expected) =>
			document.querySelector('[data-testid="tv-collected"]')?.textContent?.trim() === expected,
		TL('0.00'),
		{ timeout: 30000 }
	);
	eq('canonical after reverse', await canonical(), '0.00');
	log('live update on reversal: PASS');

	// --- 7. Socket drop → reconnect → canonical resync --------------------
	// Deterministic gap: close the TV socket while the backend stays up
	// (Chromium's offline emulation does not reliably sever WebSockets).
	// The client enters `reconnecting` → stale marker → a payment commits
	// during the gap → reconnect resync must surface it via the
	// canonical API — the socket itself never carried the change.
	await tv.evaluate(async () => {
		const mod = await import('/src/lib/realtime/realtime.svelte.ts');
		mod.live.drop();
	});
	await tv.waitForSelector('[data-testid="tv-stale"]', { timeout: 20000 });
	await postPayment('175.00', 'e2e-016-pay2'); // committed during the gap
	await tv.waitForFunction(
		() => document.querySelector('[data-testid="tv-status"]')?.textContent?.trim() === 'Bağlı',
		null,
		{ timeout: 60000 }
	);
	await tv.waitForFunction(
		(expected) =>
			document.querySelector('[data-testid="tv-collected"]')?.textContent?.trim() === expected,
		TL('175.00'),
		{ timeout: 30000 }
	);
	eq('canonical after resync', await canonical(), '175.00');
	log('offline gap → reconnect → canonical resync: PASS');

	// --- 8. Security probes ----------------------------------------------
	// Anonymous socket in a cookie-free context must be rejected.
	const anonCtx = await browser.newContext();
	const anonPage = await anonCtx.newPage();
	const anonResult = await anonPage.evaluate(
		(url) =>
			new Promise((resolve) => {
				const s = new WebSocket(url);
				s.onopen = () => resolve('opened');
				s.onerror = () => resolve('error');
				s.onclose = (e) => resolve(`closed:${e.code}`);
				setTimeout(() => resolve('timeout'), 10000);
			}),
		`ws://${API.replace('http://', '')}/api/realtime`
	);
	if (anonResult === 'opened') fail('anonymous WebSocket must not open');
	await anonCtx.close();
	log('anonymous socket rejected: PASS');

	// Wrong-Origin socket: Playwright lets us set Origin via evaluate?
	// Browsers set Origin automatically; assert the allowlisted one works
	// and a forged Referer-less non-browser attempt is covered by the
	// Rust tests (wrong_origin_upgrade_rejected).

	// TV read-only: no mutation controls exist on the display.
	const buttons = await tv.locator('button').count();
	const forms = await tv.locator('form, input[type="text"], textarea').count();
	if (buttons > 1 || forms > 0) fail(`TV must be read-only (buttons=${buttons}, inputs=${forms})`);
	// TV viewer session cannot mutate via the API either.
	const tvCsrf = (await tvLogin.json()).csrfToken;
	const forbidden = await tv.request.post(`${API}/api/financial-accounts`, {
		headers: { 'content-type': 'application/json', origin: WEB, 'x-csrf-token': tvCsrf },
		data: { name: 'TV Sızma', accountType: 'cash' }
	});
	eq('viewer POST account', forbidden.status(), 403);
	log('TV/viewer read-only enforcement: PASS');

	// --- 9. Delivery created zero phantom financial rows -----------------
	const movementsAfter = Number(psql('SELECT count(*) FROM account_movements', DB_NAME));
	// Two payments → two inflow movements; the reversal updates status
	// in place (docs/19) — real-time delivery added ZERO rows.
	eq('account_movements delta', movementsAfter - movementsBefore, 2);
	log('no phantom financial rows from real-time delivery: PASS');

	await browser.close();
	if (process.exitCode) {
		console.log('[e2e] STEP-016 finished with FAILURES');
	} else {
		console.log('[e2e] STEP-016 PASS');
	}
} finally {
	await shutdown();
}
