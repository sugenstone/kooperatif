// STEP-014 focused E2E: Governance, Decisions, Voting & Approval
// foundation through the real stack (PostgreSQL + Rust API + SvelteKit
// dev server).
//
//   admin logs in → two shareholders supply canonical Persons → UI
//   creates a governance body (organ identity, ZERO money) → UI adds a
//   member by resolving the Person; API adds + ends a second member
//   (temporal history preserved) → UI creates a draft decision → UI
//   opens voting (material content freezes; eligible = ACTIVE
//   memberships) → UI records an approve vote; API records an abstain
//   → votes by ended/unknown persons and duplicate votes rejected →
//   finalize records the operator-declared outcome plus a frozen tally
//   snapshot → finalized evidence is immutable (update/vote/refinalize
//   rejected) → a second draft is cancelled and preserved → NO
//   financial account, movement, payment or credit exists anywhere.
//
// Boundaries asserted end-to-end:
//   - governance = EVIDENCE, never money;
//   - RBAC (governance.manage operator) ≠ membership (Person seat);
//   - eligibility is server-side temporal, not UI-suggested;
//   - outcome is RECORDED (Model B) — the system never computes
//     quorum/majority;
//   - finalized decisions are immutable evidence; drafts may be
//     cancelled but never deleted.
// Run: node scripts/e2e-step014.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_URL = 'postgres://kooperatif:kooperatif_dev_password@localhost:5494/kooperatif_e2e_014';

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
		'DROP DATABASE IF EXISTS kooperatif_e2e_014 WITH (FORCE);',
		'-c',
		'CREATE DATABASE kooperatif_e2e_014;'
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

	// --- 3. Fixture: canonical Persons via shareholders -------------------
	const mkShareholder = async (first, last, seq) => {
		const r = await apiPost('/api/shareholders', {
			person: { mode: 'new', firstName: first, lastName: last },
			family: { mode: 'new', sequenceNumber: seq }
		});
		if (!r.ok()) fail(`shareholder fixture: ${await r.text()}`);
		return r.json();
	};
	const shA = await mkShareholder('E2E Ayşe', 'Üye', 1);
	const shB = await mkShareholder('E2E Mehmet', 'Üye', 2);
	const personA = shA.person?.id ?? shA.personId;
	const personB = shB.person?.id ?? shB.personId;
	const resolved = await apiGet('/api/governance/persons?search=E2E');
	if (!resolved.find((p) => p.id === personA) || !resolved.find((p) => p.id === personB)) {
		fail(`person lookup: ${JSON.stringify(resolved)}`);
	}
	log('fixture: 2 ortak (canonical Person) hazır');

	// Browser session.
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');
	{
		// The browser session is authoritative now — read its CSRF token
		// instead of opening a second login (which rotates the cookie).
		const response = await page.request.get(`${API}/api/auth/me`, {
			headers: { origin: WEB }
		});
		csrf = (await response.json()).csrfToken;
	}

	// --- 4. UI: create the governance body — identity only ---------------
	await page.goto(`${WEB}/yonetim/kurullar/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#body-name').fill('E2E Yönetim Kurulu');
	await page.locator('#body-type').fill('Yönetim Kurulu');
	const [createBodyResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/api/governance/bodies') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Kaydet' }).click()
	]);
	if (!createBodyResponse.ok()) {
		fail(`create body → ${createBodyResponse.status()}: ${await createBodyResponse.text()}`);
	}
	const body = await createBodyResponse.json();
	await page.waitForURL(`**/yonetim/kurullar/${body.id}`);
	await page.getByText('Aktif').first().waitFor();
	log(`kurul #${body.bodyNumber} oluşturuldu — UI`);

	// Governance never moves money: no financial account exists at all.
	const accounts = await apiGet('/api/financial-accounts?pageSize=50');
	if (accounts.totalCount !== 0) {
		fail(`governance created money state: ${accounts.totalCount} accounts`);
	}
	log('kurul kimliği SIFIR para etkisi (hiç finansal hesap yok)');

	// --- 5. UI: membership via canonical Person resolution ---------------
	await page.getByRole('button', { name: 'Üye Ekle' }).click();
	await page.locator('#member-search').fill('E2E Ayşe');
	const pickA = page.getByRole('button', { name: /E2E Ayşe Üye/ });
	await pickA.waitFor();
	await pickA.click();
	const [memberResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/memberships') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Kaydet' }).last().click()
	]);
	if (!memberResponse.ok()) {
		fail(`membership → ${memberResponse.status()}: ${await memberResponse.text()}`);
	}
	await page.getByText('E2E Ayşe Üye').first().waitFor();
	log('üyelik 1 UI ile kaydedildi (Person arama → seçim)');

	// Second member via API, then ended — temporal history must persist.
	let response = await apiPost(`/api/governance/bodies/${body.id}/memberships`, {
		personId: personB,
		startedAt: new Date(Date.now() - 86400000).toISOString(),
		idempotencyKey: 'e2e-014-mem-b'
	});
	if (!response.ok()) fail(`membership B: ${await response.text()}`);
	const memberB = await response.json();
	response = await apiPost(`/api/governance/memberships/${memberB.membershipId}/end`, {
		endedAt: new Date().toISOString(),
		reason: 'E2E görev süresi doldu'
	});
	if (!response.ok()) fail(`end membership B: ${await response.text()}`);
	let bodyDetail = await apiGet(`/api/governance/bodies/${body.id}`);
	if (bodyDetail.memberships.length !== 2) fail(`membership history: ${bodyDetail.memberships.length}`);
	const ended = bodyDetail.memberships.find((m) => m.personId === personB);
	if (!ended || ended.active !== false || ended.endReason !== 'E2E görev süresi doldu') {
		fail(`ended membership evidence: ${JSON.stringify(ended)}`);
	}
	log('üyelik 2 API ile açıldı + sonlandırıldı — tarihçe korunuyor (2 satır)');

	// --- 6. UI: draft decision -------------------------------------------
	await page.goto(`${WEB}/yonetim/kararlar/yeni`, { waitUntil: 'networkidle' });
	await page.locator('#dec-body').selectOption(body.id);
	await page.locator('#dec-title').fill('E2E Bütçe Kararı');
	await page.locator('#dec-text').fill('2026 bütçesi onaylansın.');
	await page.locator('#dec-date').fill('2026-01-12');
	await page.locator('#dec-effective').fill('2026-02-01');
	const [createDecisionResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().endsWith('/api/governance/decisions') && r.request().method() === 'POST'
		),
		page.getByRole('button', { name: 'Kaydet' }).click()
	]);
	if (!createDecisionResponse.ok()) {
		fail(
			`create decision → ${createDecisionResponse.status()}: ${await createDecisionResponse.text()}`
		);
	}
	const decision = await createDecisionResponse.json();
	await page.waitForURL(`**/yonetim/kararlar/${decision.id}`);
	await page.getByText('Taslak').first().waitFor();
	log(`karar #${decision.decisionNumber} taslak — UI`);

	// --- 7. UI: open voting — material content freezes -------------------
	const [openResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().endsWith('/open') && r.request().method() === 'POST'),
		(async () => {
			await page.getByRole('button', { name: 'Oylamayı Aç' }).click();
			await page.getByRole('button', { name: 'Oylamayı Aç' }).last().click();
		})()
	]);
	if (!openResponse.ok()) fail(`open → ${openResponse.status()}: ${await openResponse.text()}`);
	let detail = await apiGet(`/api/governance/decisions/${decision.id}`);
	if (detail.status !== 'open' || detail.eligibleMembers.length !== 1) {
		// Only member A is ACTIVE — B ended before opening.
		fail(`electorate at open: ${JSON.stringify(detail.eligibleMembers?.map((m) => m.personId))}`);
	}
	log('oylama açık; seçmen = SADECE aktif üyelik (1 kişi — sonlanan üye dışarıda)');

	// Material content must be frozen now.
	response = await apiPost(`/api/governance/decisions/${decision.id}/update`, {
		title: 'Değiştirilmiş konu',
		decisionText: 'değiştirildi',
		decisionOn: '2026-01-12'
	});
	if (response.status() !== 400) {
		fail(`frozen decision update accepted: ${response.status()}`);
	}
	log('açık kararda içerik değişikliği REDDEDİLDİ (immutability sınırı)');

	// --- 8. UI: record a vote for the eligible member ---------------------
	await page.getByRole('button', { name: 'Oy Kaydet' }).click();
	const voterSelect = page.locator('#vote-person');
	// Exactly one eligible member is offered (B ended).
	if ((await voterSelect.locator('option').count()) !== 2) {
		fail(`voter options: ${await voterSelect.locator('option').allTextContents()}`);
	}
	await voterSelect.selectOption(personA);
	await page.locator('#vote-choice').selectOption('approve');
	const [voteResponse] = await Promise.all([
		page.waitForResponse((r) => r.url().endsWith('/votes') && r.request().method() === 'POST'),
		page.getByRole('button', { name: 'Oyu Kaydet' }).click()
	]);
	if (!voteResponse.ok()) {
		fail(`vote → ${voteResponse.status()}: ${await voteResponse.text()}`);
	}
	log('oy kaydedildi — UI (approve, üye A)');

	// Ended member can never vote — eligibility is temporal server-side.
	response = await apiPost(`/api/governance/decisions/${decision.id}/votes`, {
		personId: personB,
		choice: 'reject',
		idempotencyKey: 'e2e-014-vote-b'
	});
	if (response.status() !== 403) {
		fail(`ended-member vote accepted: ${response.status()} ${await response.text()}`);
	}
	// Duplicate vote rejected.
	response = await apiPost(`/api/governance/decisions/${decision.id}/votes`, {
		personId: personA,
		choice: 'reject',
		idempotencyKey: 'e2e-014-vote-a2'
	});
	if (response.status() !== 409) {
		fail(`duplicate vote accepted: ${response.status()}`);
	}
	log('sonlanmış üye oyu REDDEDİLDİ (403); mükerrer oy REDDEDİLDİ (409)');

	// --- 9. Finalize — operator-recorded outcome + frozen snapshot --------
	response = await apiPost(`/api/governance/decisions/${decision.id}/finalize`, {
		outcome: 'approved'
	});
	if (!response.ok()) fail(`finalize: ${await response.text()}`);
	detail = await apiGet(`/api/governance/decisions/${decision.id}`);
	if (
		detail.status !== 'approved' ||
		detail.eligibleCount !== 1 ||
		detail.approveCount !== 1 ||
		detail.rejectCount !== 0 ||
		detail.abstainCount !== 0
	) {
		fail(`final snapshot: ${JSON.stringify(detail)}`);
	}
	// Immutable evidence: update, vote and re-finalize all rejected.
	response = await apiPost(`/api/governance/decisions/${decision.id}/update`, {
		title: 'x',
		decisionText: 'x',
		decisionOn: '2026-01-12'
	});
	if (response.status() !== 400) fail(`finalized update: ${response.status()}`);
	response = await apiPost(`/api/governance/decisions/${decision.id}/votes`, {
		personId: personA,
		choice: 'reject',
		idempotencyKey: 'e2e-014-vote-late'
	});
	if (response.status() !== 400 && response.status() !== 409) {
		fail(`vote on finalized: ${response.status()}`);
	}
	response = await apiPost(`/api/governance/decisions/${decision.id}/finalize`, {
		outcome: 'rejected'
	});
	// State-transition replay → InvalidState → 400 (project convention).
	if (response.status() !== 400) fail(`re-finalize: ${response.status()}`);
	log('sonuç KAYDEDİLDİ (approved) + donmuş anıt (1/1/0/0); her mutasyon kilitli');

	// --- 10. Cancelled draft preserved as evidence ------------------------
	response = await apiPost('/api/governance/decisions', {
		bodyId: body.id,
		title: 'E2E Vazgeçilen Karar',
		decisionText: 'İptal edilecek taslak.',
		decisionOn: '2026-01-15',
		idempotencyKey: 'e2e-014-dec-2'
	});
	if (!response.ok()) fail(`draft 2: ${await response.text()}`);
	const draft2 = await response.json();
	response = await apiPost(`/api/governance/decisions/${draft2.id}/cancel`, {
		reason: 'E2E gündemden düştü'
	});
	if (!response.ok()) fail(`cancel: ${await response.text()}`);
	detail = await apiGet(`/api/governance/decisions/${draft2.id}`);
	if (detail.status !== 'cancelled' || detail.cancellationReason !== 'E2E gündemden düştü') {
		fail(`cancelled evidence: ${JSON.stringify(detail)}`);
	}
	const decisions = await apiGet(`/api/governance/decisions?bodyId=${body.id}`);
	if (decisions.totalCount !== 2) fail(`decision register: ${decisions.totalCount}`);
	log('iptal edilen taslak SİLİNMEDİ — cancelled + reason kanıt olarak duruyor');

	// --- 11. Body close + zero-money proof --------------------------------
	response = await apiPost(`/api/governance/bodies/${body.id}/close`, {});
	if (response.status() !== 409) {
		fail(`close with active member accepted: ${response.status()}`);
	}
	const allAccounts = await apiGet('/api/financial-accounts?pageSize=50');
	if (allAccounts.totalCount !== 0) {
		fail(`governance leaked money: ${allAccounts.totalCount} accounts`);
	}
	log('aktif üyeli kurul kapanışı REDDEDİLDİ; tüm akış sonunda SIFIR finansal hesap');

	// --- 12. UI register: statuses visible --------------------------------
	await page.goto(`${WEB}/yonetim`, { waitUntil: 'networkidle' });
	await page.getByText('E2E Bütçe Kararı').first().waitFor();
	await page.locator('table').getByText('Onaylandı').first().waitFor();
	await page.locator('table').getByText('İptal Edildi').first().waitFor();
	await page.goto(`${WEB}/yonetim/kurullar/${body.id}`, { waitUntil: 'networkidle' });
	await page.getByText('E2E Mehmet Üye').first().waitFor();
	await page.locator('table').getByText('Geçmiş').first().waitFor();
	log('UI register: Onaylandı + İptal Edildi + Geçmiş üyelik görünür');

	await browser.close();
} finally {
	await shutdown();
}

if (process.exitCode) {
	console.error('[e2e] STEP-014 FAILED');
} else {
	console.log('[e2e] STEP-014 PASS — governance evidence foundation verified end-to-end');
}
