// STEP-015 focused E2E: Reporting, financial visibility & traceable
// analytics through the real stack (PostgreSQL + Rust API + SvelteKit
// dev server).
//
//   admin logs in → a cross-domain fixture is built through REAL API
//   commands (accounts, shareholders, period+assessments, payments
//   with a payer≠debtor Person, credit origin/application, operational
//   income+expense, internal transfer, share return + settlement,
//   investment funding/valuation/income, social-aid fund+donation+
//   disbursement, governance body+decision, one reversed income) →
//   the /api/reports/* endpoints return hand-computed metrics → the
//   /raporlar UI renders the same numbers in tr-TR with NULL shown as
//   'Belirlenmedi' (never 0,00 ₺) → a payments.read-only user is 403,
//   a reports.read-only user reads everything but mutates nothing →
//   table row counts are byte-identical before/after report browsing.
//
// Boundaries asserted end-to-end:
//   - every metric reuses the canonical domain formula;
//   - Payment ≠ operational income; credit ≠ cash; valuation ≠ cash;
//     aid money ≠ cooperative income/expense; transfer ≠ income/expense;
//   - reversed rows are excluded from totals yet stay traceable;
//   - reports are read-only (zero row-count changes).
// Run: node scripts/e2e-step015.mjs   (requires Docker postgres up)
import { chromium } from '../apps/web/node_modules/playwright/index.mjs';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/(.):/, '$1:').replace(/\/$/, '');
const API = 'http://localhost:8099';
const WEB = 'http://localhost:5199';
const DB_NAME = 'kooperatif_e2e_015';
const DB_URL = `postgres://kooperatif:kooperatif_dev_password@localhost:5494/${DB_NAME}`;

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

const psql = (sql, db = 'postgres') =>
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
			db,
			'-Atc',
			sql
		],
		{ stdio: 'pipe', encoding: 'utf8' }
	).stdout.trim();

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
		`DROP DATABASE IF EXISTS ${DB_NAME} WITH (FORCE);`,
		'-c',
		`CREATE DATABASE ${DB_NAME};`
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

const eq = (name, got, want) => {
	if (String(got) !== String(want)) {
		fail(`${name}: expected ${want}, got ${got}`);
	}
};

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
	const csrf = (await loginResponse.json()).csrfToken;

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

	// --- 3. Cross-domain fixture via REAL API commands --------------------
	const kasa = (
		await (
			await apiPost('/api/financial-accounts', { name: 'E2E Kasa', accountType: 'cash' })
		).json()
	).id;
	const banka = (
		await (
			await apiPost('/api/financial-accounts', { name: 'E2E Banka', accountType: 'bank' })
		).json()
	).id;

	const mkShareholder = async (first, last, seq) => {
		const r = await apiPost('/api/shareholders', {
			person: { mode: 'new', firstName: first, lastName: last },
			family: { mode: 'new', sequenceNumber: seq },
			idempotencyKey: `e2e-015-sh-${seq}`
		});
		if (!r.ok()) fail(`shareholder: ${await r.text()}`);
		return r.json();
	};
	const shA = await mkShareholder('E2E Ayşe', 'Üye', 601);
	const shB = await mkShareholder('E2E Mehmet', 'Üye', 602);
	const shC = await mkShareholder('E2E Can', 'Üye', 603);

	const period = await (
		await apiPost('/api/periods', {
			name: 'E2E Ocak',
			collectionStartDate: '2026-01-01',
			dueDate: '2026-01-31',
			ruleType: 'per_shareholder',
			baseAmount: '1000.00',
			assessmentEffectiveDate: '2026-01-01',
			idempotencyKey: 'e2e-015-period'
		})
	).json();
	const gen = await apiPost(`/api/periods/${period.id}/generate-assessments`, {
		idempotencyKey: 'e2e-015-gen'
	});
	if (!gen.ok()) fail(`generate: ${await gen.text()}`);
	const assessments = (await apiGet(`/api/periods/${period.id}/assessments?pageSize=100`)).items;
	const bySh = (id) => assessments.find((a) => a.shareholder.shareholderId === id);

	const p1 = await apiPost('/api/payments', {
		payerFirstName: 'Veli',
		payerLastName: 'Ödeyen',
		amount: '1200.00',
		method: 'cash',
		destinationAccountId: kasa,
		allocations: [
			{ assessmentId: bySh(shA.id).id, amount: '800.00' },
			{ assessmentId: bySh(shB.id).id, amount: '400.00' }
		],
		idempotencyKey: 'e2e-015-pay1'
	});
	if (!p1.ok()) fail(`payment1: ${await p1.text()}`);
	const p2 = await apiPost('/api/payments', {
		payerFirstName: 'Veli',
		payerLastName: 'Ödeyen',
		amount: '900.00',
		method: 'bank_transfer',
		destinationAccountId: banka,
		allocations: [{ assessmentId: bySh(shC.id).id, amount: '600.00' }],
		idempotencyKey: 'e2e-015-pay2'
	});
	if (!p2.ok()) fail(`payment2: ${await p2.text()}`);
	const p2id = (await p2.json()).payment.id;
	// STEP-009 canonical behavior: assigning the credit triggers the
	// auto-offset sweep — it consumes ALL open assessments of the
	// beneficiary (oldest due date first). shA's assessment has 200.00
	// remaining → auto-applies 200.00, leaving 100.00 available.
	const credit = await apiPost(`/api/payments/${p2id}/credits`, {
		shareholderId: shA.id,
		amount: '300.00',
		idempotencyKey: 'e2e-015-credit'
	});
	if (!credit.ok()) fail(`credit: ${await credit.text()}`);

	const catIn = (
		await (
			await apiPost('/api/financial-categories', {
				name: 'E2E Aidat Dışı',
				categoryType: 'income'
			})
		).json()
	).id;
	const catEx = (
		await (
			await apiPost('/api/financial-categories', {
				name: 'E2E Kırtasiye',
				categoryType: 'expense'
			})
		).json()
	).id;
	for (const [kind, cat, amount, key] of [
		['income', catIn, '250.00', 'e2e-015-inc'],
		['expense', catEx, '75.50', 'e2e-015-exp']
	]) {
		const r = await apiPost(`/api/${kind}s`, {
			financialAccountId: kasa,
			categoryId: cat,
			amount,
			description: 'e2e',
			idempotencyKey: key
		});
		if (!r.ok()) fail(`${kind}: ${await r.text()}`);
	}
	// A posted income that is then REVERSED — excluded from totals but
	// remains traceable.
	const incRev = await apiPost('/api/incomes', {
		financialAccountId: kasa,
		categoryId: catIn,
		amount: '30.00',
		description: 'e2e iptal',
		idempotencyKey: 'e2e-015-inc-rev'
	});
	const incRevId = (await incRev.json()).id;
	const revRes = await apiPost(`/api/incomes/${incRevId}/reverse`, { reason: 'E2E yanlış kayıt' });
	if (!revRes.ok()) fail(`income reverse: ${await revRes.text()}`);

	const transfer = await apiPost('/api/account-transfers', {
		sourceAccountId: kasa,
		destinationAccountId: banka,
		amount: '400.00',
		idempotencyKey: 'e2e-015-transfer'
	});
	if (!transfer.ok()) fail(`transfer: ${await transfer.text()}`);

	// Share return for shareholder R (created AFTER period generation).
	const shR = await mkShareholder('E2E İade', 'Eden', 604);
	const share = await (
		await apiPost('/api/shares', {
			shareholderId: shR.id,
			acquisitionType: 'founder',
			effectiveAt: '2025-01-01T00:00:00Z'
		})
	).json();
	const ret = await apiPost('/api/share-returns', {
		shareId: share.id,
		effectiveReturnDate: '2026-01-15',
		reason: 'e2e ayrılma',
		idempotencyKey: 'e2e-015-return'
	});
	if (!ret.ok()) fail(`return: ${await ret.text()}`);
	const retJson = await ret.json();
	const fin = await apiPost(`/api/share-returns/${retJson.id}/finalize`, {
		entitlements: [
			{ entitlementType: 'principal', amount: '500.00', dueDate: '2026-03-01', policyReference: 'YK-1' },
			{ entitlementType: 'profit' }
		],
		expectedUpdatedAt: retJson.updatedAt
	});
	if (!fin.ok()) fail(`finalize: ${await fin.text()}`);
	const principalId = (await fin.json()).entitlements.find(
		(e) => e.entitlementType === 'principal'
	).id;
	const settle = await apiPost(`/api/share-return-entitlements/${principalId}/settlements`, {
		financialAccountId: kasa,
		amount: '200.00',
		idempotencyKey: 'e2e-015-settle'
	});
	if (!settle.ok()) fail(`settle: ${await settle.text()}`);

	const inv = await (
		await apiPost('/api/investments', {
			name: 'E2E Arsa',
			investmentType: 'real_estate',
			acquiredAt: '2026-01-05',
			idempotencyKey: 'e2e-015-inv'
		})
	).json();
	for (const [kind, amount, key] of [
		['fundings', '1000.00', 'e2e-015-ifund'],
		['incomes', '120.00', 'e2e-015-iinc']
	]) {
		const r = await apiPost(`/api/investments/${inv.id}/${kind}`, {
			financialAccountId: banka,
			amount,
			occurredAt: '2026-01-12T10:00:00Z',
			description: 'e2e',
			idempotencyKey: key
		});
		if (!r.ok()) fail(`${kind}: ${await r.text()}`);
	}
	const val = await apiPost(`/api/investments/${inv.id}/valuations`, {
		valuationDate: '2026-01-14',
		amount: '1500.00',
		method: 'Ekspertiz',
		idempotencyKey: 'e2e-015-ival'
	});
	if (!val.ok()) fail(`valuation: ${await val.text()}`);

	const fund = await (
		await apiPost('/api/social-aid/funds', {
			name: 'E2E Eğitim Fonu',
			idempotencyKey: 'e2e-015-fund'
		})
	).json();
	for (const [kind, amount, key, extra] of [
		['donations', '800.00', 'e2e-015-don', { donorDisplayName: 'E2E Hayırsever' }],
		[
			'disbursements',
			'200.00',
			'e2e-015-dis',
			{ beneficiaryDisplayName: 'E2E İhtiyaç Sahibi', reason: 'burs' }
		]
	]) {
		const r = await apiPost(`/api/social-aid/${kind}`, {
			fundId: fund.id,
			financialAccountId: banka,
			amount,
			occurredAt: '2026-01-12T10:00:00Z',
			idempotencyKey: key,
			...extra
		});
		if (!r.ok()) fail(`${kind}: ${await r.text()}`);
	}

	const gbody = await (
		await apiPost('/api/governance/bodies', {
			name: 'E2E Rapor Kurulu',
			bodyType: 'YK',
			idempotencyKey: 'e2e-015-body'
		})
	).json();
	const decision = await (
		await apiPost('/api/governance/decisions', {
			bodyId: gbody.id,
			title: 'E2E Rapor Kararı',
			decisionText: 'x',
			decisionOn: '2026-01-14',
			idempotencyKey: 'e2e-015-dec'
		})
	).json();
	await apiPost(`/api/governance/decisions/${decision.id}/open`, {});
	const fin2 = await apiPost(`/api/governance/decisions/${decision.id}/finalize`, {
		outcome: 'approved'
	});
	if (!fin2.ok()) fail(`finalize decision: ${await fin2.text()}`);
	log('fixture hazır: 7 alan — hesap, dönem, ödeme, kredi, gelir/gider, iade, yatırım, sosyal yardım, yönetim');

	// --- 4. Hand-computed expectations ------------------------------------
	const REPORT_PATHS = [
		'/api/reports/overview',
		'/api/reports/financial-accounts',
		'/api/reports/movements',
		'/api/reports/assessments',
		'/api/reports/assessments/summary',
		'/api/reports/periods',
		'/api/reports/shareholders',
		'/api/reports/families',
		'/api/reports/payments',
		'/api/reports/payments/summary',
		'/api/reports/credits',
		'/api/reports/share-returns',
		'/api/reports/investments',
		'/api/reports/social-aid',
		'/api/reports/governance',
		'/api/reports/income-expense-trend'
	];

	// Read-only proof: row counts before browsing all reports.
	const TABLES = [
		'account_movements',
		'payments',
		'payment_allocations',
		'shareholder_credits',
		'credit_applications',
		'income_entries',
		'expense_entries',
		'share_return_settlements',
		'investment_fundings',
		'social_aid_donations',
		'social_aid_disbursements',
		'governance_votes',
		'assessments'
	];
	const before = {};
	for (const table of TABLES) before[table] = psql(`SELECT count(*) FROM ${table}`, DB_NAME);

	const overview = await apiGet('/api/reports/overview');
	// Kasa: 1200+250+30−30−75.50−400−200 = 774.50 · Banka: 900+400−1000
	//   +120+800−200 = 1020.00 → 1794.50 total.
	eq('overview.financialAccountsBalance', overview.financialAccountsBalance, '1794.50');
	eq('overview.outstandingAssessmentDebt', overview.outstandingAssessmentDebt, '1000.00');
	eq('overview.assessmentsTotal', overview.assessmentsTotal, '3000.00');
	eq('overview.availableShareholderCredit', overview.availableShareholderCredit, '100.00');
	eq('overview.operationalIncomeTotal', overview.operationalIncomeTotal, '250.00');
	eq('overview.operationalExpenseTotal', overview.operationalExpenseTotal, '75.50');
	eq('overview.operationalNet', overview.operationalNet, '174.50');
	eq('overview.postedPaymentsTotal', overview.postedPaymentsTotal, '2100.00');
	eq('overview.outstandingReturnEntitlementDetermined', overview.outstandingReturnEntitlementDetermined, '300.00');
	eq('overview.undeterminedEntitlementCount', overview.undeterminedEntitlementCount, '1');
	eq('overview.investmentTotalFunded', overview.investmentTotalFunded, '1000.00');
	eq('overview.investmentLatestValuationTotal', overview.investmentLatestValuationTotal, '1500.00');
	eq('overview.investmentIncomeTotal', overview.investmentIncomeTotal, '120.00');
	eq('overview.socialAidRestrictedAvailable', overview.socialAidRestrictedAvailable, '600.00');
	eq('overview.activeShareholderCount', overview.activeShareholderCount, '4');
	// R's only share transitioned return_pending → closed on finalize.
	eq('overview.activeShareCount', overview.activeShareCount, '0');
	eq('overview.decisionsApproved', overview.decisionsApproved, '1');
	if ('netWorth' in overview || 'profit' in overview || 'nav' in overview) {
		fail('invented metric leaked into overview');
	}
	log('genel bakış metrikleri gerçek API üzerinden doğrulandı');

	const movements = await apiGet('/api/reports/movements?pageSize=100');
	// external inflow: 1200+900 payments + 250 income + 120 inv income
	//   + 800 donation = 3270.00 (reversed 30.00 income excluded);
	// external outflow: 75.50 + 200 settle + 1000 funding + 200 aid
	//   = 1475.50; internal transfer volume = 400.00.
	eq('movements.externalInflow', movements.summary.externalInflow, '3270.00');
	eq('movements.externalOutflow', movements.summary.externalOutflow, '1475.50');
	eq('movements.internalTransferVolume', movements.summary.internalTransferVolume, '400.00');
	const reversedRows = movements.items.filter((m) => m.status === 'reversed');
	if (!reversedRows.find((m) => m.amount === '30.00' && m.reversalReason)) {
		fail('reversed movement not traceable in report list');
	}
	// Filtered view: sourceType=transfer lists both legs; summary shows
	// internal volume only.
	const tr = await apiGet('/api/reports/movements?sourceType=transfer');
	eq('transfer filter count', tr.items.length, '2');
	eq('transfer filter internal', tr.summary.internalTransferVolume, '400.00');
	log('hareket raporu: kaynak izi + dış/iç ayrımı + iptal izlenebilirliği doğrulandı');

	const asmSummary = await apiGet('/api/reports/assessments/summary');
	eq('assessments.totalAssessed', asmSummary.totalAssessed, '3000.00');
	eq('assessments.paymentAllocated', asmSummary.totalPaymentAllocated, '1800.00');
	eq('assessments.creditApplied', asmSummary.totalCreditApplied, '200.00');
	eq('assessments.outstanding', asmSummary.totalOutstanding, '1000.00');

	const payments = await apiGet('/api/reports/payments');
	eq('payments.posted', payments.summary.postedAmount, '2100.00');
	eq('payments.allocated', payments.summary.allocatedAmount, '1800.00');
	eq('payments.credited', payments.summary.creditedAmount, '300.00');
	const p1row = payments.items.find((p) => p.paymentNumber === 1);
	eq('payer≠debtor preserved', p1row.payerName, 'Veli Ödeyen');
	if (!p1row.debtorShareholderNames.includes('E2E Ayşe')) {
		fail(`debtor names: ${p1row.debtorShareholderNames}`);
	}

	const credits = await apiGet('/api/reports/credits');
	eq('credits.available', credits.summary.totalAvailable, '100.00');

	const ents = await apiGet('/api/reports/share-returns');
	const profit = ents.items.find((e) => e.entitlementType === 'profit');
	if (profit.amount !== null || profit.remainingAmount !== null) {
		fail(`undetermined entitlement rendered as ${JSON.stringify(profit.amount)} — NULL violated`);
	}
	eq('entitlements.determined', ents.summary.determinedOutstanding, '300.00');
	eq('entitlements.undetermined', ents.summary.undeterminedCount, '1');

	const investments = await apiGet('/api/reports/investments');
	const invRow = investments.items[0];
	eq('investments.funded', invRow.totalFunded, '1000.00');
	eq('investments.valuation', invRow.latestValuation, '1500.00');
	eq('investments.income', invRow.incomeTotal, '120.00');
	if ('gain' in invRow || 'profit' in invRow) fail('invented gain metric');

	const aid = await apiGet('/api/reports/social-aid');
	eq('aid.restricted', aid.funds[0].restrictedAvailable, '600.00');
	eq('aid.pairs', aid.pairs.length, '1');
	log('API rapor katmanı: sınır ve formül ayrımları doğrulandı');

	// Browse EVERY report endpoint, then prove zero mutation.
	for (const path of REPORT_PATHS) await apiGet(path);
	for (const table of TABLES) {
		eq(`read-only ${table}`, psql(`SELECT count(*) FROM ${table}`, DB_NAME), before[table]);
	}
	log('salt-okunur kanıtı: 16 rapor endpoint + 13 tablo satır sayısı değişmedi');

	// Permission isolation: payments.read alone ≠ report access;
	// reports.read alone reads reports but mutates nothing.
	const mkUser = async (username) => {
		const { execFileSync } = await import('node:child_process');
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
				username,
				'--display-name',
				username,
				'--password-stdin'
			],
			{ env: serverEnv, input: 'e2e-parola-cok-gizli-1', stdio: ['pipe', 'pipe', 'pipe'] }
		);
	};
	await mkUser('e2epayer');
	await mkUser('e2ereader');
	psql(
		`INSERT INTO roles (id, name) VALUES ('e2e2e2e2-0000-4000-8000-000000000015','E2E Odeme Okur') ON CONFLICT DO NOTHING;
		 INSERT INTO role_permissions (role_id, permission_id) SELECT 'e2e2e2e2-0000-4000-8000-000000000015', id FROM permissions WHERE key='payments.read' ON CONFLICT DO NOTHING;
		 INSERT INTO user_role_assignments (user_id, role_id) SELECT id, 'e2e2e2e2-0000-4000-8000-000000000015' FROM users WHERE username='e2epayer' ON CONFLICT DO NOTHING;
		 INSERT INTO roles (id, name) VALUES ('e2e2e2e2-0000-4000-8000-000000000016','E2E Rapor Okur') ON CONFLICT DO NOTHING;
		 INSERT INTO role_permissions (role_id, permission_id) SELECT 'e2e2e2e2-0000-4000-8000-000000000016', id FROM permissions WHERE key='reports.read' ON CONFLICT DO NOTHING;
		 INSERT INTO user_role_assignments (user_id, role_id) SELECT id, 'e2e2e2e2-0000-4000-8000-000000000016' FROM users WHERE username='e2ereader' ON CONFLICT DO NOTHING;`,
		DB_NAME
	);
	const loginAs = async (username) => {
		const r = await page.request.post(`${API}/api/auth/login`, {
			headers: { 'content-type': 'application/json', origin: WEB },
			data: { username, password: 'e2e-parola-cok-gizli-1' }
		});
		if (!r.ok()) fail(`login ${username}: ${r.status()}`);
		return r;
	};
	await loginAs('e2epayer');
	const denied = await page.request.get(`${API}/api/reports/overview`, { headers: { origin: WEB } });
	eq('payments.read → reports 403', denied.status(), 403);
	await page.request.post(`${API}/api/auth/logout`, {
		headers: { 'content-type': 'application/json', origin: WEB }
	});
	await loginAs('e2ereader');
	const allowed = await page.request.get(`${API}/api/reports/overview`, { headers: { origin: WEB } });
	eq('reports.read → overview 200', allowed.status(), 200);
	// Read-only even for report users: POST attempts are rejected.
	const readerCsrf = (await (await page.request.get(`${API}/api/auth/me`, { headers: { origin: WEB } })).json()).csrfToken;
	const mut = await page.request.post(`${API}/api/financial-accounts`, {
		headers: {
			'content-type': 'application/json',
			origin: WEB,
			'x-csrf-token': readerCsrf
		},
		data: { name: 'Yetkisiz', accountType: 'cash' }
	});
	if (mut.ok()) fail('reports.read user created an account!');
	log('izin ayrışması: payments.read→403, reports.read→200, mutasyon reddedildi');

	// --- 5. UI: /raporlar --------------------------------------------------
	await page.request.post(`${API}/api/auth/logout`, {
		headers: { 'content-type': 'application/json', origin: WEB }
	});
	await page.goto(`${WEB}/login`, { waitUntil: 'networkidle' });
	await page.getByLabel('Kullanıcı adı').fill('e2eadmin');
	await page.getByLabel('Parola').fill('e2e-parola-cok-gizli-1');
	await page.getByRole('button', { name: 'Giriş Yap' }).click();
	await page.waitForURL('**/');

	await page.getByRole('link', { name: 'Raporlar' }).click();
	await page.waitForURL('**/raporlar');
	await page.getByText('1.794,50 ₺').first().waitFor();
	await page.getByText('Açık Tahakkuk Borcu').first().waitFor();
	await page.getByText('1.000,00 ₺').first().waitFor();
	await page.getByText('Sosyal Yardım Tahsisli Bakiye').first().waitFor();
	await page.getByText('600,00 ₺').first().waitFor();
	log('UI genel bakış: kartlar tr-TR, exact decimal gösterim');

	// Switch to the movement report — provenance columns + summary.
	await page.selectOption('#report-type', 'movements');
	await page.getByText('Kaynak Türü').first().waitFor();
	await page.getByText('3.270,00 ₺').first().waitFor();
	await page.getByText('İç transfer hacmi').first().waitFor();
	log('UI hareket raporu: kaynak türü + dış/iç ayrımı görünür');

	// Entitlement report — NULL renders as 'Belirlenmedi', never 0,00 ₺.
	await page.selectOption('#report-type', 'shareReturns');
	await page.getByText('Belirlenmedi').first().waitFor();
	// The undetermined profit right must show 'Belirlenmedi' for BOTH
	// amount and remaining (settledAmount 0,00 ₺ is legitimate).
	const undeterminedRow = page.locator('tr', { hasText: 'Belirlenmedi' }).first();
	const undeterminedCells = await undeterminedRow.getByText('Belirlenmedi').count();
	if (undeterminedCells < 2) {
		fail(`undetermined row shows ${undeterminedCells} 'Belirlenmedi' cells — NULL violated`);
	}
	log('UI hisse iadesi: NULL → Belirlenmedi korunuyor');

	// Assessment report — canonical settled columns visible.
	await page.selectOption('#report-type', 'assessments');
	await page.getByText('Kredi Uygulaması').first().waitFor();
	await page.getByText('1.000,00 ₺').first().waitFor();
	log('UI tahakkuk raporu: ödeme dağıtımı + kredi uygulaması ayrı sütunlar');

	await browser.close();
} finally {
	await shutdown();
}

if (process.exitCode) {
	console.error('[e2e] STEP-015 FAILED');
} else {
	console.log('[e2e] STEP-015 PASS — reporting & traceable analytics verified end-to-end');
}
