// backup:drill — full recovery drill on real PostgreSQL (ADR-013:
// "untested backup is not a verified backup"; docs/23 restore testing).
//
//   pnpm backup:drill            # run the drill, clean up on success
//   pnpm backup:drill -- --keep  # preserve drill DBs + artifacts
//
// Flow (isolated resources only — never touches dev/verify databases):
//   compose postgres → fresh kooperatif_backup_drill_source → migrate →
//   deterministic fixtures → backup:create (real operator command) →
//   backup:verify → fresh kooperatif_backup_drill_restore →
//   backup:restore → schema/data/constraint verification battery →
//   server `migrate` smoke on the restored DB → cleanup.
//
// Exit code is non-zero if any stage fails.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { BackupError, COMPOSE_FILE, ROOT } from './lib.mjs';

const KEEP = process.argv.includes('--keep');
const log = (msg) => console.log(`[drill] ${msg}`);
const results = [];
const stage = (name, ok) => {
	results.push({ name, ok });
	console.log(`  ${name.padEnd(28)} ${ok ? 'PASS' : 'FAIL'}`);
};

const PGUSER = process.env.POSTGRES_USER || 'kooperatif';
const PGPASS = process.env.POSTGRES_PASSWORD || 'kooperatif_dev_password';
const PGPORT = process.env.POSTGRES_PORT || '5494';
const SOURCE_DB = 'kooperatif_backup_drill_source';
const RESTORE_DB = 'kooperatif_backup_drill_restore';
const SOURCE_URL = `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/${SOURCE_DB}`;

// Synchronous sleep without shelling out (cross-platform).
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, ms);

const compose = (args, { input, capture = false, allowFail = false } = {}) => {
	// stdio 'inherit' would silently discard `input`; pipe stdin whenever
	// SQL is streamed in, and inherit stdout/stderr for live progress
	// unless output capture is requested.
	const stdio =
		capture || input !== undefined
			? ['pipe', capture ? 'pipe' : 'inherit', capture ? 'pipe' : 'inherit']
			: 'inherit';
	const res = spawnSync(
		'docker',
		['compose', '-f', COMPOSE_FILE, ...args],
		{ input, encoding: 'utf8', stdio }
	);
	if (res.error) throw new BackupError(`docker: ${res.error.message}`);
	if (res.status !== 0 && !allowFail) {
		throw new BackupError(`docker compose ${args.join(' ')} → ${res.status}: ${res.stderr ?? ''}`);
	}
	return res;
};

const adminSql = (sql, db = 'postgres') =>
	compose(
		['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', db, '-t', '-A', '-v', 'ON_ERROR_STOP=1', '-c', sql],
		{ capture: true }
	).stdout.trim();

const node = (script, args, env = {}) => {
	const res = spawnSync('node', [path.join(ROOT, 'scripts', 'backup', script), ...args], {
		env: { ...process.env, ...env },
		encoding: 'utf8',
		stdio: 'pipe'
	});
	return res;
};

async function main() {
	const drillDir = mkdtempSync(path.join(tmpdir(), 'kooperatif-drill-'));
	const started = Date.now();
	log(`scratch dir: ${drillDir}`);

	try {
		// --- Postgres service -------------------------------------------------
		compose(['up', '-d', 'postgres']);
		const cid = compose(['ps', '-q', 'postgres'], { capture: true }).stdout.trim();
		let healthy = false;
		for (let i = 0; i < 60; i += 1) {
			const st = spawnSync('docker', ['inspect', '--format', '{{if .State.Health}}{{.State.Health.Status}}{{else}}unknown{{end}}', cid], {
				encoding: 'utf8'
			}).stdout.trim();
			if (st === 'healthy') {
				healthy = true;
				break;
			}
			sleep(1000);
		}
		stage('Source PostgreSQL', healthy);
		if (!healthy) throw new BackupError('postgres service not healthy');

		// --- Source DB: fresh + migrations + fixtures -------------------------
		adminSql(`DROP DATABASE IF EXISTS ${SOURCE_DB} WITH (FORCE)`);
		adminSql(`CREATE DATABASE ${SOURCE_DB}`);
		const mig = spawnSync('cargo', ['run', '--manifest-path', 'apps/server/Cargo.toml', '--quiet', '--', 'migrate'], {
			env: { ...process.env, KOOPERATIF_DATABASE_URL: SOURCE_URL },
			encoding: 'utf8',
			stdio: 'pipe',
			cwd: ROOT
		});
		stage('Migrations on source', mig.status === 0);
		if (mig.status !== 0) throw new BackupError(`migrate failed: ${mig.stderr}`);

		compose(['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', SOURCE_DB, '-v', 'ON_ERROR_STOP=1'], {
			input: readFileSync(path.join(ROOT, 'scripts', 'backup', 'fixtures.sql'))
		});
		stage('Fixture preparation', true);

		// --- Backup via the real operator command -----------------------------
		const env = {
			KOOPERATIF_BACKUP_DATABASE_URL: SOURCE_URL,
			KOOPERATIF_BACKUP_DIR: drillDir,
			KOOPERATIF_BACKUP_ENV: 'drill'
		};
		const createRes = node('create.mjs', [], env);
		stage('Backup creation', createRes.status === 0);
		if (createRes.status !== 0) throw new BackupError(`create failed: ${createRes.stdout}${createRes.stderr}`);
		const artifactName = readdirSync(drillDir).find((n) => n.endsWith('.dump'));
		if (!artifactName) throw new BackupError('no .dump artifact produced');

		// --- Level-1 verify ----------------------------------------------------
		const verifyRes = node('verify.mjs', [artifactName], env);
		stage('Artifact verification', verifyRes.status === 0);
		stage('Archive inspection', verifyRes.status === 0);
		if (verifyRes.status !== 0) throw new BackupError(`verify failed: ${verifyRes.stdout}${verifyRes.stderr}`);

		// --- Fresh target + restore -------------------------------------------
		adminSql(`DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE)`);
		const restoreRes = node(
			'restore.mjs',
			['--artifact', artifactName, '--target-db', RESTORE_DB, '--confirm', RESTORE_DB],
			env
		);
		stage('Fresh target + restore', restoreRes.status === 0);
		if (restoreRes.status !== 0) throw new BackupError(`restore failed: ${restoreRes.stdout}${restoreRes.stderr}`);

		// --- Verification battery ---------------------------------------------
		const expect = (name, sql, want) => {
			const got = adminSql(sql, RESTORE_DB);
			stage(name, got === want);
			if (got !== want) log(`    got: ${got}`);
		};

		const manifest = JSON.parse(readFileSync(path.join(drillDir, `${artifactName}.manifest.json`), 'utf8'));
		const srcMig = adminSql(
			"SELECT json_agg(version||':'||description ORDER BY version)::text FROM _sqlx_migrations WHERE success",
			SOURCE_DB
		);
		const dstMig = adminSql(
			"SELECT json_agg(version||':'||description ORDER BY version)::text FROM _sqlx_migrations WHERE success",
			RESTORE_DB
		);
		// json_agg text output uses ", " separators — compare normalized JSON.
		stage(
			'Migration ledger match',
			srcMig === dstMig &&
				JSON.stringify(JSON.parse(dstMig)) === JSON.stringify(manifest.migrations)
		);

		expect(
			'Extension verification',
			"SELECT json_agg(extname ORDER BY extname)::jsonb = '[\"btree_gist\",\"pgcrypto\"]'::jsonb FROM pg_extension WHERE extname <> 'plpgsql'",
			't'
		);

		const counts = [
			['users', 1],
			['user_role_assignments', 1],
			['user_sessions', 1],
			['permissions', 28],
			['persons', 4],
			['shareholders', 2],
			['families', 2],
			['shareholder_family_memberships', 3],
			['shares', 3],
			['share_ownerships', 4],
			['share_events', 6],
			['periods', 3],
			['assessment_rules', 3],
			['assessments', 4],
			['assessment_share_sources', 4],
			['payments', 3],
			['payment_allocations', 3],
			['financial_accounts', 2],
			['account_movements', 19],
			['account_transfers', 1],
			['shareholder_credits', 1],
			['credit_applications', 2],
			['financial_categories', 12],
			['income_entries', 1],
			['expense_entries', 2],
			['share_returns', 1],
			['share_return_entitlements', 2],
			['share_return_settlements', 2],
			['investments', 2],
			['investment_fundings', 2],
			['investment_valuations', 2],
			['investment_incomes', 2],
			['investment_disposals', 1],
			['investment_disposal_proceeds', 1],
			['social_aid_funds', 2],
			['social_aid_donations', 2],
			['social_aid_disbursements', 2]
		];
		let allCounts = true;
		for (const [table, want] of counts) {
			const got = adminSql(`SELECT count(*) FROM ${table}`, RESTORE_DB);
			if (got !== String(want)) {
				allCounts = false;
				log(`    ${table}: expected ${want}, got ${got}`);
			}
		}
		stage('Critical data (row counts)', allCounts);

		const relChecks = [
			// Shareholder → person + guardian + current family.
			[
				"SELECT p.first_name||' '||p.last_name||'|'||gp.first_name||' '||gp.last_name||'|'||f.sequence_number FROM shareholders s JOIN persons p ON p.id=s.person_id JOIN persons gp ON gp.id=s.guardian_person_id JOIN shareholder_family_memberships m ON m.shareholder_id=s.id AND m.ended_at IS NULL JOIN families f ON f.id=m.family_id WHERE s.id='44444444-0000-4000-8000-000000000001'",
				'Yedek Hissedarı|Vasi Test|900002'
			],
			// Share 9001 current owner = B via open interval.
			[
				"SELECT p.first_name||' '||p.last_name FROM share_ownerships o JOIN shareholders s ON s.id=o.shareholder_id JOIN persons p ON p.id=s.person_id JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NULL",
				'Yedek Devralan'
			],
			// Ownership interval → source event provenance.
			[
				"SELECT e.event_type FROM share_ownerships o JOIN share_events e ON e.id=o.source_event_id JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NULL",
				'transfer'
			],
			// Assessment → period + debtor shareholder.
			[
				"SELECT p.name FROM assessments a JOIN periods p ON p.id=a.period_id WHERE a.id='acacacac-0000-4000-8000-000000000004'",
				'Yedek Dönem C'
			],
			// Assessment share source → share + ownership interval.
			[
				"SELECT count(*) FROM assessment_share_sources src JOIN assessments a ON a.id=src.assessment_id JOIN shares sh ON sh.id=src.share_id JOIN share_ownerships o ON o.id=src.ownership_id WHERE a.id='acacacac-0000-4000-8000-000000000004'",
				'2'
			],
			// Payment 9001 → payer is the THIRD-PARTY person, never a debtor FK.
			[
				"SELECT p.first_name||' '||p.last_name FROM payments pay JOIN persons p ON p.id=pay.payer_person_id WHERE pay.payment_number=9001",
				'Ödeyen Üçüncü'
			],
			// Payment 9001 allocations reach TWO different debtors.
			[
				"SELECT count(DISTINCT a.shareholder_id) FROM payment_allocations al JOIN assessments a ON a.id=al.assessment_id WHERE al.payment_id='dddddddd-0000-4000-8000-000000000001'",
				'2'
			],
			// Reversed payment keeps actor + reason (docs/19 bookkeeping).
			[
				"SELECT status||'|'||reversal_reason FROM payments WHERE payment_number=9002",
				'reversed|drill: hatalı kayıt'
			],
			// Payment 9001 → destination financial account (STEP-008).
			[
				"SELECT a.name FROM payments p JOIN financial_accounts a ON a.id=p.destination_account_id WHERE p.payment_number=9001",
				'Yedek Kasa'
			],
			// Transfer 9001 links its two legs to the same source row.
			[
				"SELECT s.name||'→'||d.name||'|'||count(*) FROM account_transfers t JOIN financial_accounts s ON s.id=t.source_account_id JOIN financial_accounts d ON d.id=t.destination_account_id JOIN account_movements m ON m.source_type='transfer' AND m.source_id=t.id WHERE t.transfer_number=9001 GROUP BY s.name, d.name",
				'Yedek Kasa→Yedek Banka|2'
			],
			// Credit → explicit beneficiary + source payment (never payer).
			[
				"SELECT p.first_name||' '||p.last_name||'|'||pay.payment_number FROM shareholder_credits c JOIN shareholders s ON s.id=c.shareholder_id JOIN persons p ON p.id=s.person_id JOIN payments pay ON pay.id=c.source_payment_id WHERE c.credit_number=9001",
				'Yedek Hissedarı|9003'
			],
			// Application → credit origin + obligation target + mode.
			[
				"SELECT c.credit_number||'|'||p.name||'|'||ap.mode FROM credit_applications ap JOIN shareholder_credits c ON c.id=ap.credit_id JOIN assessments a ON a.id=ap.assessment_id JOIN periods p ON p.id=a.period_id WHERE ap.status='active'",
				'9001|Yedek Dönem A|automatic'
			],
			// Income 9001 → its ONE movement, provenance 'income' (STEP-010).
			[
				"SELECT m.direction||'|'||m.source_type||'|'||(m.source_id=i.id)::text FROM income_entries i JOIN account_movements m ON m.id=i.account_movement_id WHERE i.income_number=9001",
				'inflow|income|true'
			],
			// Expense 9001 → typed category + account.
			[
				"SELECT c.name||'|'||c.category_type||'|'||a.name FROM expense_entries e JOIN financial_categories c ON c.id=e.category_id JOIN financial_accounts a ON a.id=e.financial_account_id WHERE e.expense_number=9001",
				'Yedek Gider Türü|expense|Yedek Kasa'
			],
			// Reversed expense 9002 keeps entry AND movement reversal bookkeeping.
			[
				"SELECT e.status||'|'||m.status||'|'||e.reversal_reason FROM expense_entries e JOIN account_movements m ON m.id=e.account_movement_id WHERE e.expense_number=9002",
				'reversed|reversed|drill: hatalı gider'
			],
			// STEP-011: return 9001 → closed share + identity snapshot.
			[
				"SELECT sh.status||'|'||r.owner_display_name FROM share_returns r JOIN shares sh ON sh.id=r.share_id WHERE r.return_number=9001",
				'closed|Yedek Hissedarı · Vasi: Vasi Test · Aile No 900002'
			],
			// Entitlement 9001 → beneficiary + determined amount; 9002 stays
			// undetermined (NULL amount must survive as NULL, never 0).
			[
				"SELECT p.first_name||' '||p.last_name||'|'||e.amount::text||'|'||e.entitlement_type FROM share_return_entitlements e JOIN shareholders s ON s.id=e.beneficiary_shareholder_id JOIN persons p ON p.id=s.person_id WHERE e.entitlement_number=9001",
				'Yedek Hissedarı|1000.00|principal'
			],
			[
				"SELECT (e.amount IS NULL)::text||'|'||(e.determined_at IS NULL)::text||'|'||e.status FROM share_return_entitlements e WHERE e.entitlement_number=9002",
				'true|true|open'
			],
			// Settlement 9001 → its ONE movement, provenance
			// 'share_return_settlement' — never income/expense/payment.
			[
				"SELECT m.direction||'|'||m.source_type||'|'||a.name FROM share_return_settlements st JOIN account_movements m ON m.id=st.account_movement_id JOIN financial_accounts a ON a.id=st.financial_account_id WHERE st.settlement_number=9001",
				'outflow|share_return_settlement|Yedek Kasa'
			],
			// Reversed settlement 9002 keeps entry AND movement reversal
			// bookkeeping after restore.
			[
				"SELECT st.status||'|'||m.status||'|'||st.reversal_reason FROM share_return_settlements st JOIN account_movements m ON m.id=st.account_movement_id WHERE st.settlement_number=9002",
				'reversed|reversed|drill: hatalı ödeme'
			],
			// STEP-012: investment 9001 disposed real estate + identity.
			[
				"SELECT investment_type||'|'||status||'|'||coalesce(location,'-') FROM investments WHERE investment_number=9001",
				'real_estate|disposed|Organize Sanayi Bölgesi'
			],
			// Funding 9001 → its ONE movement, provenance
			// 'investment_funding' — never expense/payment/transfer.
			[
				"SELECT m.direction||'|'||m.source_type||'|'||a.name FROM investment_fundings f JOIN account_movements m ON m.id=f.account_movement_id JOIN financial_accounts a ON a.id=f.financial_account_id WHERE f.funding_number=9001",
				'outflow|investment_funding|Yedek Kasa'
			],
			// Reversed funding 9002 keeps entry AND movement reversal.
			[
				"SELECT f.status||'|'||m.status||'|'||f.reversal_reason FROM investment_fundings f JOIN account_movements m ON m.id=f.account_movement_id WHERE f.funding_number=9002",
				'reversed|reversed|drill: hatalı finansman'
			],
			// Valuation 9001 preserves method + source; 9002 stays
			// cancelled with its reason (history, never deleted).
			[
				"SELECT v.amount::text||'|'||v.method||'|'||v.source FROM investment_valuations v WHERE v.valuation_number=9001",
				'300.00|Emsal karşılaştırma|Eksper raporu ER-9'
			],
			[
				"SELECT v.status||'|'||v.cancellation_reason FROM investment_valuations v WHERE v.valuation_number=9002",
				'cancelled|drill: hatalı değerleme'
			],
			// Investment income 9001 → its ONE 'investment_income'
			// inflow — NOT a STEP-010 incomes row.
			[
				"SELECT m.direction||'|'||m.source_type FROM investment_incomes i JOIN account_movements m ON m.id=i.account_movement_id WHERE i.income_number=9001",
				'inflow|investment_income'
			],
			// Disposal 9001 → agreed consideration is metadata; the ONE
			// proceeds leg is the real cash inflow on the BANK account.
			[
				"SELECT d.consideration_amount::text||'|'||d.counterparty_name||'|'||d.status FROM investment_disposals d WHERE d.disposal_number=9001",
				'350.00|Drill Alıcı AŞ|posted'
			],
			[
				"SELECT m.direction||'|'||m.source_type||'|'||a.name||'|'||p.amount::text FROM investment_disposal_proceeds p JOIN account_movements m ON m.id=p.account_movement_id JOIN financial_accounts a ON a.id=p.financial_account_id WHERE p.disposal_id='18181818-0000-4000-8000-000000000001'",
				'inflow|investment_disposal|Yedek Banka|320.00'
			],
			// STEP-013: fund 9001 restricted identity (never an account);
			// cancelled fund 9002 keeps actor + reason.
			[
				"SELECT name||'|'||status FROM social_aid_funds WHERE fund_number=9001",
				'Drill Eğitim Fonu|active'
			],
			[
				"SELECT status||'|'||cancellation_reason FROM social_aid_funds WHERE fund_number=9002",
				'cancelled|drill: program iptal'
			],
			// Donation 9001 → canonical Person donor (third-party payer —
			// donor ≠ shareholder) + its ONE 'social_aid_donation' inflow.
			[
				"SELECT p.first_name||' '||p.last_name||'|'||m.direction||'|'||m.source_type FROM social_aid_donations d JOIN persons p ON p.id=d.donor_person_id JOIN account_movements m ON m.id=d.account_movement_id WHERE d.donation_number=9001",
				'Ödeyen Üçüncü|inflow|social_aid_donation'
			],
			// Donation 9002 → external organization identity + entry and
			// movement reversal bookkeeping both survive.
			[
				"SELECT d.donor_display_name||'|'||d.status||'|'||m.status||'|'||d.reversal_reason FROM social_aid_donations d JOIN account_movements m ON m.id=d.account_movement_id WHERE d.donation_number=9002",
				'Drill Hayırsever A.Ş.|reversed|reversed|drill: hatalı bağış'
			],
			// Disbursement 9001 → beneficiary Person + required reason +
			// its ONE 'social_aid_disbursement' outflow — never
			// expense/payment/transfer.
			[
				"SELECT p.first_name||' '||p.last_name||'|'||d.reason||'|'||m.direction||'|'||m.source_type FROM social_aid_disbursements d JOIN persons p ON p.id=d.beneficiary_person_id JOIN account_movements m ON m.id=d.account_movement_id WHERE d.disbursement_number=9001",
				'Yedek Devralan|drill: burs ödemesi|outflow|social_aid_disbursement'
			],
			[
				"SELECT d.status||'|'||m.status||'|'||d.reversal_reason FROM social_aid_disbursements d JOIN account_movements m ON m.id=d.account_movement_id WHERE d.disbursement_number=9002",
				'reversed|reversed|drill: hatalı yardım'
			]
		];
		let allRel = true;
		for (const [sql, want] of relChecks) {
			const got = adminSql(sql, RESTORE_DB);
			if (got !== want) {
				allRel = false;
				log(`    relationship check failed: ${want} != ${got}`);
			}
		}
		stage('Relationships', allRel);

		// Temporal history: closed + open intervals survived.
		expect(
			'Temporal family history',
			"SELECT count(*) FROM shareholder_family_memberships WHERE shareholder_id='44444444-0000-4000-8000-000000000001' AND ended_at IS NOT NULL",
			'1'
		);
		expect(
			'Temporal ownership history',
			"SELECT count(*) FROM share_ownerships o JOIN shares sh ON sh.id=o.share_id WHERE sh.share_number=9001 AND o.ended_at IS NOT NULL",
			'1'
		);

		// Exact money: text-exact comparison, never float.
		expect(
			'Assessment precision (0.01/10000.00/1234.56)',
			"SELECT json_agg(amount::text ORDER BY amount)::jsonb = '[\"0.02\",\"2469.12\",\"10000.00\",\"10000.00\"]'::jsonb FROM assessments",
			't'
		);
		expect(
			'PER_SHARE provenance sum',
			"SELECT sum(amount_component)::text FROM assessment_share_sources WHERE assessment_id='acacacac-0000-4000-8000-000000000004'",
			'2469.12'
		);
		// Payment precision + derived paid total for assessment ...0001:
		// 100.00 active + 30.00 reversed → only the ACTIVE row counts.
		expect(
			'Payment precision (150.00/30.00/80.00)',
			"SELECT json_agg(amount::text ORDER BY amount)::jsonb = '[\"30.00\",\"80.00\",\"150.00\"]'::jsonb FROM payments",
			't'
		);
		expect(
			'Active allocations for assessment ...0001',
			"SELECT coalesce(sum(amount),0)::text FROM payment_allocations WHERE assessment_id='acacacac-0000-4000-8000-000000000001' AND status='active'",
			'100.00'
		);
		// Derived balances on the RESTORED copy — cash: +150 -40 +80
		// +500 (income) −60 (expense) −300 (settlement) −200 (funding)
		// +25 (investment income) +300 (donation) −120 (aid) = 335; the
		// reversed 25.00 expense, 100.00 settlement, 50.00 funding,
		// 10.00 investment income, 40.00 donation and 50.00 aid are
		// excluded. Bank: +30 reversed (excluded) +40 +320 (disposal
		// proceeds) = 360. Balances are never stored; the restored
		// movement history must derive them.
		expect(
			'Derived cash balance (…+300−120=335.00)',
			"SELECT coalesce(sum(CASE direction WHEN 'inflow' THEN amount ELSE -amount END),0)::text FROM account_movements WHERE account_id='f1f1f1f1-0000-4000-8000-000000000001' AND status='active'",
			'335.00'
		);
		expect(
			'Derived bank balance (40+320=360.00)',
			"SELECT coalesce(sum(CASE direction WHEN 'inflow' THEN amount ELSE -amount END),0)::text FROM account_movements WHERE account_id='f1f1f1f1-0000-4000-8000-000000000002' AND status='active'",
			'360.00'
		);
		expect(
			'Reversed movement history preserved',
			"SELECT count(*) FROM account_movements WHERE status='reversed' AND reversed_by IS NOT NULL AND reversal_reason IS NOT NULL",
			'7'
		);
		// STEP-011 derived truth: remaining principal = determined amount
		// − POSTED settlements (1000 − 300 = 700; the reversed 100 is
		// excluded). Never stored — always derived on the copy.
		expect(
			'Derived entitlement remaining (1000-300=700.00)',
			"SELECT (e.amount - coalesce((SELECT sum(s.amount) FROM share_return_settlements s WHERE s.entitlement_id=e.id AND s.status='posted'),0))::text FROM share_return_entitlements e WHERE e.entitlement_number=9001",
			'700.00'
		);
		// STEP-010: entry exactness + operational totals on the restored
		// copy — posted income 500.00, posted expense 60.00 (the reversed
		// 25.00 counts in neither totals nor balance).
		expect(
			'Income/expense precision (500.00/60.00/25.00)',
			"SELECT json_agg(amount::text ORDER BY amount)::jsonb = '[\"25.00\",\"60.00\"]'::jsonb FROM expense_entries",
			't'
		);
		expect(
			'Operational totals (posted only: 500/60/440)',
			"SELECT (SELECT coalesce(sum(amount),0)::text FROM income_entries WHERE status='posted')||'|'||(SELECT coalesce(sum(amount),0)::text FROM expense_entries WHERE status='posted')||'|'||((SELECT coalesce(sum(amount),0) FROM income_entries WHERE status='posted')-(SELECT coalesce(sum(amount),0) FROM expense_entries WHERE status='posted'))::text",
			'500.00|60.00|440.00'
		);

		// STEP-009 derived truths on the restored copy:
		// available credit = active credits − active applications
		// (80.00 − 30.00 = 50.00; the reversed 25.00 is excluded).
		expect(
			'Derived credit available (80-30=50.00)',
			"SELECT (coalesce((SELECT sum(amount) FROM shareholder_credits WHERE status='active'),0) - coalesce((SELECT sum(amount) FROM credit_applications WHERE status='active'),0))::text",
			'50.00'
		);
		// Assessment ...0001 settled = 100.00 allocations + 30.00 credit.
		expect(
			'Settled incl. credit application (100+30=130.00)',
			"SELECT (coalesce((SELECT sum(amount) FROM payment_allocations WHERE assessment_id='acacacac-0000-4000-8000-000000000001' AND status='active'),0) + coalesce((SELECT sum(amount) FROM credit_applications WHERE assessment_id='acacacac-0000-4000-8000-000000000001' AND status='active'),0))::text",
			'130.00'
		);
		expect(
			'Credit created NO account movement',
			"SELECT count(*) FROM account_movements WHERE source_type IN ('credit','credit_application') OR source_id IN ('a9a9a9a9-0000-4000-8000-000000000001','b1b1b1b1-0000-4000-8000-000000000001','b1b1b1b1-0000-4000-8000-000000000002')",
			'0'
		);

		// STEP-012 derived truths on the restored copy:
		//   cost = POSTED fundings only (200; reversed 50 excluded),
		//   value = LATEST recorded valuation (300; cancelled 310 is
		//   never "latest"), cash result = posted incomes + proceeds
		//   (25 + 320). Consideration 350 is metadata — no gain math.
		expect(
			'Derived investment cost (posted fundings = 200.00)',
			"SELECT coalesce(sum(amount),0)::text FROM investment_fundings WHERE investment_id='10101010-0000-4000-8000-000000000001' AND status='posted'",
			'200.00'
		);
		expect(
			'Latest recorded valuation (300.00, cancelled excluded)',
			"SELECT amount::text FROM investment_valuations WHERE investment_id='10101010-0000-4000-8000-000000000001' AND status='recorded' ORDER BY valuation_date DESC, valuation_number DESC LIMIT 1",
			'300.00'
		);
		expect(
			'Valuation created NO account movement',
			"SELECT count(*) FROM account_movements WHERE source_id IN ('14141414-0000-4000-8000-000000000001','14141414-0000-4000-8000-000000000002')",
			'0'
		);
		expect(
			'Posted investment income (25.00; no incomes row)',
			"SELECT coalesce(sum(amount),0)::text FROM investment_incomes WHERE status='posted'",
			'25.00'
		);
		expect(
			'Disposal proceeds = actual cash (320.00 ≠ agreed 350.00)',
			"SELECT coalesce(sum(amount),0)::text FROM investment_disposal_proceeds WHERE disposal_id='18181818-0000-4000-8000-000000000001'",
			'320.00'
		);

		// STEP-013 derived truths on the restored copy — restricted
		// availability is NEVER a stored column: posted donations
		// (300.00; reversed 40.00 excluded) minus posted disbursements
		// (120.00; reversed 50.00 excluded) = 180.00, per
		// (fund, financial_account).
		expect(
			'Derived restricted availability (300-120=180.00)',
			"SELECT (coalesce((SELECT sum(d.amount) FROM social_aid_donations d WHERE d.fund_id='30303030-0000-4000-8000-000000000001' AND d.financial_account_id='f1f1f1f1-0000-4000-8000-000000000001' AND d.status='posted'),0) - coalesce((SELECT sum(y.amount) FROM social_aid_disbursements y WHERE y.fund_id='30303030-0000-4000-8000-000000000001' AND y.financial_account_id='f1f1f1f1-0000-4000-8000-000000000001' AND y.status='posted'),0))::text",
			'180.00'
		);
		// Social Aid is a separate financial context: its movements are
		// NOT income/expense/payment/transfer legs and no donation or
		// disbursement created a row in the cooperative-finance tables.
		expect(
			'Social aid ≠ income/expense/payment/transfer',
			"SELECT count(*) FROM account_movements WHERE source_type IN ('social_aid_donation','social_aid_disbursement') AND (source_id IN (SELECT id FROM payments) OR source_id IN (SELECT id FROM income_entries) OR source_id IN (SELECT id FROM expense_entries) OR source_id IN (SELECT id FROM account_transfers))",
			'0'
		);

		// Constraints must still BEHAVE after restore — not just exist.
		const constraintFails = [
			[
				'assessment uniqueness',
				`INSERT INTO assessments (period_id, shareholder_id, rule_type, base_amount, amount, assessment_effective_date, generated_by)
				 VALUES ('99999999-0000-4000-8000-000000000001','44444444-0000-4000-8000-000000000001','per_shareholder',1,1,'2026-01-15','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'family membership overlap exclusion',
				`INSERT INTO shareholder_family_memberships (shareholder_id, family_id, started_at, ended_at)
				 VALUES ('44444444-0000-4000-8000-000000000001','33333333-0000-4000-8000-000000000001','2025-05-01','2025-05-31')`
			],
			[
				'share ownership overlap exclusion',
				`INSERT INTO share_ownerships (share_id, shareholder_id, started_at, ended_at, acquisition_type, source_event_id, created_by)
				 VALUES ('66666666-0000-4000-8000-000000000001','44444444-0000-4000-8000-000000000001','2025-07-01',NULL,'transfer','77777777-0000-4000-8000-000000000002','11111111-0000-4000-8000-0000000000aa')`
			],
			['family sequence unique', `INSERT INTO families (sequence_number) VALUES (900001)`],
			['period dates check', `INSERT INTO periods (name, collection_start_date, due_date) VALUES ('x','2026-05-02','2026-05-01')`],
			[
				'payment positive amount check',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',0,'cash',now(),'drill-x','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'payment idempotency key unique',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',1,'cash',now(),'drill-idem-0001','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'payment reversal consistency check',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',1,'cash',now(),'reversed','drill-y','drill-y','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'allocation (payment,assessment) unique',
				`INSERT INTO payment_allocations (payment_id, assessment_id, amount, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000001','acacacac-0000-4000-8000-000000000001',1,'11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'account currency check',
				`INSERT INTO financial_accounts (name, account_type, currency, created_by)
				 VALUES ('drill usd','cash','USD','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'movement leg uniqueness',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','inflow',1,'payment','dddddddd-0000-4000-8000-000000000001',now(),'11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'movement reversal consistency check',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, status, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','inflow',1,'transfer','f3f3f3f3-0000-4000-8000-000000000099',now(),'reversed','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'transfer same-account check',
				`INSERT INTO account_transfers (source_account_id, destination_account_id, amount, occurred_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'drill-tx','drill-tx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'transfer idempotency key unique',
				`INSERT INTO account_transfers (source_account_id, destination_account_id, amount, occurred_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000002',1,now(),'drill-tidem-0001','drill-ty','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'payment destination FK',
				`INSERT INTO payments (payer_person_id, amount, method, received_at, destination_account_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('22222222-0000-4000-8000-000000000004',1,'cash',now(),'f1f1f1f1-0000-4000-8000-00000000ffff','drill-z','drill-z','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit positive amount check',
				`INSERT INTO shareholder_credits (source_payment_id, shareholder_id, amount, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000003','44444444-0000-4000-8000-000000000001',0,'drill-cx','drill-cx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit idempotency key unique',
				`INSERT INTO shareholder_credits (source_payment_id, shareholder_id, amount, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000003','44444444-0000-4000-8000-000000000001',1,'drill-cidem-0001','drill-cy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit reversal consistency check',
				`INSERT INTO shareholder_credits (source_payment_id, shareholder_id, amount, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000003','44444444-0000-4000-8000-000000000001',1,'reversed','drill-cz','drill-cz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit beneficiary shareholder FK',
				`INSERT INTO shareholder_credits (source_payment_id, shareholder_id, amount, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('dddddddd-0000-4000-8000-000000000003','44444444-0000-4000-8000-00000000ffff',1,'drill-cw','drill-cw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit application mode check',
				`INSERT INTO credit_applications (credit_id, assessment_id, amount, mode, created_by)
				 VALUES ('a9a9a9a9-0000-4000-8000-000000000001','acacacac-0000-4000-8000-000000000001',1,'legacy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'credit application idempotency key unique',
				`INSERT INTO credit_applications (credit_id, assessment_id, amount, mode, idempotency_key, created_by)
				 VALUES ('a9a9a9a9-0000-4000-8000-000000000001','acacacac-0000-4000-8000-000000000001',1,'manual','drill-capp-0001','11111111-0000-4000-8000-0000000000aa')`
			],
			// STEP-010 invariants must still BEHAVE after restore.
			[
				'category (type,name) unique',
				`INSERT INTO financial_categories (category_type, name, created_by)
				 VALUES ('income','Yedek Gelir Türü  ','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'income positive amount check',
				`INSERT INTO income_entries (financial_account_id, category_id, amount, occurred_at, description, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','c0c0c0c0-0000-4000-8000-000000000001',0,now(),'drill','f2f2f2f2-0000-4000-8000-0000000099aa','drill-ix','drill-ix','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'income idempotency key unique',
				`INSERT INTO income_entries (financial_account_id, category_id, amount, occurred_at, description, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','c0c0c0c0-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-0000000099bb','drill-iidem-0001','drill-iy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'income reversal consistency check',
				`INSERT INTO income_entries (financial_account_id, category_id, amount, occurred_at, description, account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','c0c0c0c0-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-0000000099cc','reversed','drill-iz','drill-iz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'income/expense category-type composite FK',
				`INSERT INTO expense_entries (financial_account_id, category_id, amount, occurred_at, description, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','c0c0c0c0-0000-4000-8000-000000000001',1,now(),'drill: income kategorisi giderde','f2f2f2f2-0000-4000-8000-0000000099dd','drill-ex','drill-ex','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'entry→movement 1:1 unique',
				`INSERT INTO income_entries (financial_account_id, category_id, amount, occurred_at, description, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','c0c0c0c0-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-000000000006','drill-iw','drill-iw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'income movement leg uniqueness',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','inflow',1,'income','e1e1e1e1-0000-4000-8000-000000000001',now(),'11111111-0000-4000-8000-0000000000aa')`
			],
			// STEP-011 invariants must still BEHAVE after restore.
			[
				'share status enum',
				`UPDATE shares SET status='archived' WHERE share_number=9001`
			],
			[
				'return idempotency key unique',
				`INSERT INTO share_returns (share_id, shareholder_id, owner_display_name, share_number, ownership_started_at, requested_at, effective_return_date, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('66666666-0000-4000-8000-000000000002','44444444-0000-4000-8000-000000000002','x',9002,now(),now(),'2026-02-10','drill-ridem-0001','drill-rx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'return finalized consistency check',
				`INSERT INTO share_returns (share_id, shareholder_id, owner_display_name, share_number, ownership_started_at, requested_at, effective_return_date, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('66666666-0000-4000-8000-000000000002','44444444-0000-4000-8000-000000000002','x',9002,now(),now(),'2026-02-10','finalized','drill-ry','drill-ry','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'one open right per (return,type)',
				`INSERT INTO share_return_entitlements (share_return_id, entitlement_type, beneficiary_shareholder_id, amount, recognized_at, determined_at, created_by)
				 VALUES ('d5d5d5d5-0000-4000-8000-000000000001','principal','44444444-0000-4000-8000-000000000001',1,now(),now(),'11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'entitlement determined consistency check',
				`INSERT INTO share_return_entitlements (share_return_id, entitlement_type, beneficiary_shareholder_id, amount, recognized_at, status, cancelled_at, cancelled_by, cancellation_reason, created_by)
				 VALUES ('d5d5d5d5-0000-4000-8000-000000000001','profit','44444444-0000-4000-8000-000000000001',1,now(),'cancelled',now(),'11111111-0000-4000-8000-0000000000aa','x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'settlement positive amount check',
				`INSERT INTO share_return_settlements (entitlement_id, financial_account_id, amount, settled_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('e7e7e7e7-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',0,now(),'f2f2f2f2-0000-4000-8000-0000000099ee','drill-sx','drill-sx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'settlement idempotency key unique',
				`INSERT INTO share_return_settlements (entitlement_id, financial_account_id, amount, settled_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('e7e7e7e7-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-0000000099ff','drill-sidem-0001','drill-sy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'settlement reversal consistency check',
				`INSERT INTO share_return_settlements (entitlement_id, financial_account_id, amount, settled_at, account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('e7e7e7e7-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000a0a0','reversed','drill-sz','drill-sz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'settlement→movement 1:1 unique',
				`INSERT INTO share_return_settlements (entitlement_id, financial_account_id, amount, settled_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('e7e7e7e7-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-000000000009','drill-sw','drill-sw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'settlement movement leg uniqueness',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','outflow',1,'share_return_settlement','f4f4f4f4-0000-4000-8000-000000000001',now(),'11111111-0000-4000-8000-0000000000aa')`
			],
			// STEP-012 invariants must still BEHAVE after restore.
			[
				'investment type enum',
				`INSERT INTO investments (name, investment_type, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','vehicle','drill-iex','drill-iex','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'investment idempotency key unique',
				`INSERT INTO investments (name, investment_type, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','business','drill-iidem-0001','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'investment disposed consistency check',
				`INSERT INTO investments (name, investment_type, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','business','disposed','drill-iy','drill-iy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'funding positive amount check',
				`INSERT INTO investment_fundings (investment_id, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',0,now(),'f2f2f2f2-0000-4000-8000-00000000b0b0','drill-fx','drill-fx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'funding idempotency key unique',
				`INSERT INTO investment_fundings (investment_id, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000b1b1','drill-fidem-0001','drill-fy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'funding reversal consistency check',
				`INSERT INTO investment_fundings (investment_id, financial_account_id, amount, occurred_at, account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000b2b2','reversed','drill-fz','drill-fz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'funding→movement 1:1 unique',
				`INSERT INTO investment_fundings (investment_id, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-000000000011','drill-fw','drill-fw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'valuation positive amount check',
				`INSERT INTO investment_valuations (investment_id, valuation_date, amount, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','2026-03-01',0,'drill-vx','drill-vx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'valuation cancelled consistency check',
				`INSERT INTO investment_valuations (investment_id, valuation_date, amount, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','2026-03-01',1,'cancelled','drill-vy','drill-vy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'investment income positive amount check',
				`INSERT INTO investment_incomes (investment_id, financial_account_id, amount, occurred_at, description, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',0,now(),'drill','f2f2f2f2-0000-4000-8000-00000000b3b3','drill-gx','drill-gx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'investment income reversal consistency check',
				`INSERT INTO investment_incomes (investment_id, financial_account_id, amount, occurred_at, description, account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000002','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-00000000b4b4','reversed','drill-gz','drill-gz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'one disposal per investment',
				`INSERT INTO investment_disposals (investment_id, disposed_at, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('10101010-0000-4000-8000-000000000001','2026-05-01','drill-dx','drill-dx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'proceeds→movement 1:1 unique',
				`INSERT INTO investment_disposal_proceeds (disposal_id, financial_account_id, amount, occurred_at, account_movement_id)
				 VALUES ('18181818-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-000000000015')`
			],
			[
				'investment movement leg uniqueness',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','outflow',1,'investment_funding','12121212-0000-4000-8000-000000000001',now(),'11111111-0000-4000-8000-0000000000aa')`
			],
			// STEP-013 invariants must still BEHAVE after restore.
			[
				'fund status enum',
				`UPDATE social_aid_funds SET status='archived' WHERE fund_number=9001`
			],
			[
				'fund idempotency key unique',
				`INSERT INTO social_aid_funds (name, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','drill-aidem-0001','drill-x','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'fund closed consistency check',
				`INSERT INTO social_aid_funds (name, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','closed','drill-ay','drill-ay','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'fund window check (ends before starts)',
				`INSERT INTO social_aid_funds (name, starts_on, ends_on, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('drill','2026-05-02','2026-05-01','drill-aw','drill-aw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'donation identity required',
				`INSERT INTO social_aid_donations (fund_id, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000c0c0','drill-dx','drill-dx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'donation positive amount check',
				`INSERT INTO social_aid_donations (fund_id, donor_display_name, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',0,now(),'f2f2f2f2-0000-4000-8000-00000000c1c1','drill-dy','drill-dy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'donation idempotency key unique',
				`INSERT INTO social_aid_donations (fund_id, donor_display_name, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000c2c2','drill-didem-0001','drill-dz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'donation reversal consistency check',
				`INSERT INTO social_aid_donations (fund_id, donor_display_name, financial_account_id, amount, occurred_at, account_movement_id, status, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-00000000c3c3','reversed','drill-dw','drill-dw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'donation→movement 1:1 unique',
				`INSERT INTO social_aid_donations (fund_id, donor_display_name, financial_account_id, amount, occurred_at, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'f2f2f2f2-0000-4000-8000-000000000016','drill-dv','drill-dv','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'disbursement identity required',
				`INSERT INTO social_aid_disbursements (fund_id, financial_account_id, amount, occurred_at, reason, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-00000000c4c4','drill-yx','drill-yx','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'disbursement reason required',
				`INSERT INTO social_aid_disbursements (fund_id, beneficiary_display_name, financial_account_id, amount, occurred_at, reason, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),NULL,'f2f2f2f2-0000-4000-8000-00000000c5c5','drill-yy','drill-yy','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'disbursement idempotency key unique',
				`INSERT INTO social_aid_disbursements (fund_id, beneficiary_display_name, financial_account_id, amount, occurred_at, reason, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-00000000c6c6','drill-yidem-0001','drill-yz','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'disbursement→movement 1:1 unique',
				`INSERT INTO social_aid_disbursements (fund_id, beneficiary_display_name, financial_account_id, amount, occurred_at, reason, account_movement_id, idempotency_key, idempotency_fingerprint, created_by)
				 VALUES ('30303030-0000-4000-8000-000000000001','drill','f1f1f1f1-0000-4000-8000-000000000001',1,now(),'drill','f2f2f2f2-0000-4000-8000-000000000018','drill-yw','drill-yw','11111111-0000-4000-8000-0000000000aa')`
			],
			[
				'social aid movement leg uniqueness',
				`INSERT INTO account_movements (account_id, direction, amount, source_type, source_id, occurred_at, created_by)
				 VALUES ('f1f1f1f1-0000-4000-8000-000000000001','inflow',1,'social_aid_donation','31313131-0000-4000-8000-000000000001',now(),'11111111-0000-4000-8000-0000000000aa')`
			]
		];
		let allConstraints = true;
		for (const [name, sql] of constraintFails) {
			const res = compose(
				['exec', '-T', 'postgres', 'psql', '-U', PGUSER, '-d', RESTORE_DB, '-v', 'ON_ERROR_STOP=1', '-c', sql],
				{ capture: true, allowFail: true }
			);
			if (res.status === 0) {
				allConstraints = false;
				log(`    constraint did NOT fire: ${name}`);
			}
		}
		stage('Constraint verification', allConstraints);

		// Application smoke: the server binary accepts the restored schema
		// (migrate is a no-op) — proves app-version/schema compatibility.
		const smoke = spawnSync(
			'cargo',
			['run', '--manifest-path', 'apps/server/Cargo.toml', '--quiet', '--', 'migrate'],
			{
				env: { ...process.env, KOOPERATIF_DATABASE_URL: `postgres://${PGUSER}:${PGPASS}@localhost:${PGPORT}/${RESTORE_DB}` },
				encoding: 'utf8',
				stdio: 'pipe',
				cwd: ROOT
			}
		);
		stage('Application migrate smoke', smoke.status === 0);

		const failed = results.filter((r) => !r.ok);
		const seconds = ((Date.now() - started) / 1000).toFixed(1);
		console.log('\nBACKUP DRILL');
		for (const r of results) console.log(`  ${r.name.padEnd(34)} ${r.ok ? 'PASS' : 'FAIL'}`);
		if (failed.length === 0) {
			console.log(`\nRECOVERY DRILL PASSED in ${seconds}s`);
			if (!KEEP) {
				adminSql(`DROP DATABASE IF EXISTS ${SOURCE_DB} WITH (FORCE)`);
				adminSql(`DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE)`);
				rmSync(drillDir, { recursive: true, force: true });
			} else {
				log(`kept: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
			}
		} else {
			console.log(`\nRECOVERY DRILL FAILED (${failed.length} stage(s))`);
			log(`preserved for diagnostics: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
			process.exitCode = 1;
		}
	} catch (error) {
		console.error(`\n[drill] FAILED: ${error.message}`);
		log(`preserved for diagnostics: ${SOURCE_DB}, ${RESTORE_DB}, ${drillDir}`);
		process.exitCode = 1;
	}
}

main();
