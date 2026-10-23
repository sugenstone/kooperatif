// M1-P7-B — real-browser E2E for the platform cooperative-management UI.
//
//   Disposable DB kooperatif_e2e_p7b (created only if absent — never
//   dropped) → migrate → users → bootstrap cooperative → capability
//   split fixture:
//     p7badmin    = seeded NULL-scope `platform-lifecycle` role ONLY
//                   (cooperatives.manage, never users.manage) + plain
//                   coop A membership — the least-privilege operator;
//     p7busradmin = NULL-scope `Sistem Yöneticisi` (users.manage,
//                   never cooperatives.manage);
//     p7bcoopadmin= cooperative-scope authority only (forged
//                   cooperatives.manage included) — never platform.
//   Journeys: capability flags on /me, platform list/detail/create/
//   activate/suspend/reactivate/archive, unauthorized + mismatched-
//   capability denial surfaces, session-scope stability during platform
//   operations, P7-A recovery regression while scoped to a suspended
//   cooperative, mobile layout.
//
// Run: node scripts/e2e-m1p7-b.mjs   (requires Docker postgres up)
import { chromium } from "../apps/web/node_modules/playwright/index.mjs";
import { spawn, spawnSync, execFileSync } from "node:child_process";
import { spawnService, killService } from "./lib/proc-group.mjs";
import { setTimeout as sleep } from "node:timers/promises";
import { randomUUID } from "node:crypto";
import { createWriteStream, mkdirSync } from "node:fs";
import { join } from "node:path";

const ROOT = new URL("..", import.meta.url).pathname
  .replace(/^\/(.):/, "$1:")
  .replace(/\/$/, "");
// M1-P8-A: every machine-specific value is env-overridable; the
// defaults below preserve the documented local development layout.
const API = process.env.E2E_API_URL ?? "http://localhost:8099";
const WEB = process.env.E2E_WEB_URL ?? "http://localhost:5199";
const API_PORT = new URL(API).port || "80";
const WEB_PORT = new URL(WEB).port || "80";
const DB_NAME = process.env.E2E_DB_NAME ?? "kooperatif_e2e_p7b";
const PG_HOST = process.env.E2E_DB_HOST ?? "localhost";
const PG_PORT = process.env.E2E_DB_PORT ?? "5494";
const PG_USER = process.env.E2E_PG_USER ?? "kooperatif";
const PG_PASSWORD = process.env.E2E_PG_PASSWORD ?? "kooperatif_dev_password";
const APP_PASSWORD =
  process.env.E2E_APP_PASSWORD ?? "kooperatif_app_dev_password";
const DB_URL = `postgres://${PG_USER}:${PG_PASSWORD}@${PG_HOST}:${PG_PORT}/${DB_NAME}`;
const APP_DB_URL = `postgres://kooperatif_app:${APP_PASSWORD}@${PG_HOST}:${PG_PORT}/${DB_NAME}`;

// Disposable-fixture guards: the fixture name must keep the
// kooperatif_e2e_* convention and both connection targets must stay on
// loopback, so a mistyped override can never point this script at a
// shared or protected database.
if (!/^kooperatif_e2e_[a-z0-9_]+$/.test(DB_NAME)) {
  console.error(`refusing non-e2e fixture database name '${DB_NAME}'`);
  process.exit(1);
}
for (const [label, url] of [
  ["database", DB_URL],
  ["app", APP_DB_URL],
]) {
  const host = new URL(url).hostname;
  if (!["localhost", "127.0.0.1", "::1"].includes(host)) {
    console.error(`refusing non-local database target '${host}' (${label} url)`);
    process.exit(1);
  }
}

const log = (msg) => console.log(`[p7b-e2e] ${msg}`);
const fail = (msg) => {
  console.error(`[p7b-e2e] FAIL: ${msg}`);
  process.exitCode = 1;
};
const check = (name, cond) => {
  if (!cond) fail(name);
  else log(`ok: ${name}`);
};

// Diagnostics + secret hygiene (M1-P8-A): when E2E_LOG_DIR is set the
// child-process output is mirrored to files the CI uploads on failure.
// Every byte leaving this process — file or verbose stderr — passes
// through redact(), so generated CI credentials and the dev fallbacks
// can never be persisted or printed.
const LOG_DIR = process.env.E2E_LOG_DIR;
if (LOG_DIR) mkdirSync(LOG_DIR, { recursive: true });
const logStream = (name) =>
  LOG_DIR ? createWriteStream(join(LOG_DIR, `${DB_NAME}-${name}.log`)) : null;
const secrets = [
  PG_PASSWORD,
  APP_PASSWORD,
  process.env.E2E_APP_ROLE_PASSWORD,
].filter(Boolean);
const redact = (s) =>
  secrets.reduce((t, pw) => t.replaceAll(pw, "***"), String(s ?? ""));
const die = (msg) => {
  console.error(`FATAL: ${redact(msg)}`);
  process.exit(1);
};

async function waitFor(url, timeoutMs = 120000, label = url) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok || response.status === 401 || response.status === 404)
        return true;
    } catch {}
    await sleep(500);
  }
  throw new Error(`timeout waiting for ${label} (${url})`);
}

// A realtime-driven hard navigation can destroy the evaluate context
// mid-call; these evaluations are pure reads, so waiting for the load
// event and retrying is safe and never masks a failed assertion.
const evalResilient = async (pg, fn, arg, attempts = 3) => {
  for (let i = 0; i < attempts; i++) {
    try {
      return await pg.evaluate(fn, arg);
    } catch (error) {
      if (
        i === attempts - 1 ||
        !/Execution context was destroyed|Cannot find object with id/.test(
          String(error),
        )
      )
        throw error;
      await pg.waitForLoadState("load").catch(() => {});
    }
  }
};


const compose = (args) =>
  spawnSync(
    "docker",
    ["compose", "-f", `${ROOT}/docker/compose.yaml`, ...args],
    { stdio: "pipe", encoding: "utf8" },
  );
const psql = (sql, db = "postgres") =>
  compose([
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    PG_USER,
    "-d",
    db,
    "-Atc",
    sql,
  ]).stdout.trim();

// --- 1. Disposable fixture database -----------------------------------
log("preparing database");
const up = compose(["up", "-d", "postgres"]);
if (up.status !== 0) die(`postgres startup failed: ${up.stderr}`);
for (let i = 0; i < 60; i++) {
  if (
    compose(["exec", "-T", "postgres", "pg_isready", "-U", PG_USER])
      .status === 0
  )
    break;
  await sleep(1000);
}

// Ephemeral-instance proof (M1-P8-A Batch 3): the fixture-name and
// loopback guards alone cannot prove WHICH postgres owns the TCP port
// the backend will dial — a same-named database could exist in a stray
// local dev cluster. The compose service's published-port binding is
// exclusive by kernel semantics: if it reports PG_PORT, no other
// process can hold that port, so host TCP on PG_PORT necessarily lands
// in this container's cluster. Fail closed at every step.
const psLines = compose(["ps", "postgres", "--format", "json"]).stdout;
const containerRunning = psLines
  .split("\n")
  .map((line) => {
    try {
      return JSON.parse(line);
    } catch {
      return null;
    }
  })
  .some((o) => o?.Service === "postgres" && o?.State === "running");
if (!containerRunning)
  die("postgres container is not running — target instance unidentified");
const published = compose(["port", "postgres", "5432"]).stdout;
if (!published.split("\n").some((m) => m.trim().endsWith(`:${PG_PORT}`))) {
  die(
    `postgres is not published on ${PG_PORT} (got '${published.trim()}') — refusing an unidentified instance`,
  );
}
// CI-only (E2E_EPHEMERAL=1): stamp an unforgeable per-run marker into
// the cluster and read it back. The exec path and the published port
// both belong to this container, so host TCP provably reaches THIS
// run's ephemeral instance. Local runs never stamp the shared cluster.
if (process.env.E2E_EPHEMERAL === "1") {
  const runMarker = `kooperatif-e2e-run:${randomUUID()}`;
  psql(`COMMENT ON DATABASE postgres IS '${runMarker}'`);
  const markerBack = psql(
    `SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = 'postgres'`,
  );
  if (markerBack !== runMarker)
    die("ephemeral run-marker mismatch — target is not this run's cluster");
  log(`ephemeral instance verified (${runMarker})`);
}

const exists = psql(
  `SELECT count(*) FROM pg_database WHERE datname = '${DB_NAME}'`,
);
let fixtureReady = false;
if (exists === "1") {
  const migrated = psql(
    `SELECT count(*) FROM _sqlx_migrations WHERE version = 72 AND success`,
    DB_NAME,
  );
  const users = Number(
    psql(
      `SELECT count(*) FROM users WHERE username IN ('p7badmin','p7bcoopadmin','p7bplain','p7busradmin')`,
      DB_NAME,
    ),
  );
  const platformRole = psql(
    `SELECT count(*) FROM roles WHERE lower(name) = 'platform-lifecycle' AND cooperative_id IS NULL`,
    DB_NAME,
  );
  if (migrated === "1" && users >= 3 && platformRole === "1") {
    log("reusing own fixture database (signature verified)");
    fixtureReady = true;
  } else {
    fail(`${DB_NAME} exists with an unrecognized shape — refusing to touch it`);
    process.exit(1);
  }
}
if (!fixtureReady) {
  const created = compose([
    "exec",
    "-T",
    "postgres",
    "psql",
    "-U",
    PG_USER,
    "-d",
    "postgres",
    "-c",
    `CREATE DATABASE ${DB_NAME};`,
  ]);
  if (created.status !== 0) fail(`create database: ${created.stderr}`);
}

const serverEnv = {
  ...process.env,
  KOOPERATIF_ENV: "development",
  KOOPERATIF_HOST: "127.0.0.1",
  KOOPERATIF_PORT: API_PORT,
  KOOPERATIF_DATABASE_URL: DB_URL,
  KOOPERATIF_APP_DATABASE_URL: APP_DB_URL,
  KOOPERATIF_CORS_ORIGINS: WEB,
  KOOPERATIF_ALLOWED_ORIGINS: WEB,
  KOOPERATIF_ARGON2_M_COST: "8192",
  KOOPERATIF_ARGON2_T_COST: "1",
  KOOPERATIF_ARGON2_P_COST: "1",
};
const cargo = (args, input) =>
  spawnSync(
    "cargo",
    [
      "run",
      "--manifest-path",
      `${ROOT}/apps/server/Cargo.toml`,
      "--quiet",
      "--",
      ...args,
    ],
    { env: serverEnv, input, stdio: "pipe", encoding: "utf8" },
  );
let result = cargo(["migrate"]);
if (result.status !== 0) die(`migrate failed: ${result.stderr}`);

// M1-P8-A: on CI the fixture database is fresh, so the migrate step
// above just created `kooperatif_app` WITHOUT a password (migration
// 0032 never sets one). When the caller supplies
// E2E_APP_ROLE_PASSWORD, provision it now — via stdin so the value
// never reaches argv or logs. Local runs leave it unset and the local
// role untouched.
if (process.env.E2E_APP_ROLE_PASSWORD) {
  const pw = process.env.E2E_APP_ROLE_PASSWORD.replaceAll("'", "''");
  const provision = spawnSync(
    "docker",
    [
      "compose",
      "-f",
      `${ROOT}/docker/compose.yaml`,
      "exec",
      "-T",
      "postgres",
      "psql",
      "-U",
      PG_USER,
      "-d",
      "postgres",
    ],
    {
      input: `ALTER ROLE kooperatif_app PASSWORD '${pw}';
`,
      stdio: ["pipe", "pipe", "pipe"],
      encoding: "utf8",
    },
  );
  if (provision.status !== 0)
    fail(`app role provisioning failed: ${provision.stderr}`);
  log("provisioned ci app-role password");
}


const mkuser = (username, displayName, password) => {
  try {
    execFileSync(
      "cargo",
      [
        "run",
        "--manifest-path",
        `${ROOT}/apps/server/Cargo.toml`,
        "--quiet",
        "--",
        "create-user",
        "--username",
        username,
        "--display-name",
        displayName,
        "--password-stdin",
      ],
      { env: serverEnv, input: password, stdio: ["pipe", "pipe", "pipe"] },
    );
  } catch (error) {
    fail(`create-user ${username}: ${error.stderr?.toString()}`);
  }
};

if (!fixtureReady) {
  mkuser("p7badmin", "P7B Platform Admin", "p7b-parola-gizli-1");
  mkuser("p7bcoopadmin", "P7B Koop Yöneticisi", "p7b-parola-gizli-2");
  mkuser("p7bplain", "P7B Düz Üye", "p7b-parola-gizli-3");
  mkuser("p7busradmin", "P7B Kullanıcı Yöneticisi", "p7b-parola-gizli-4");
  result = cargo(["bootstrap-cooperative", "--name", "P7B Ana"]);
  if (result.status !== 0) fail(`bootstrap-cooperative: ${result.stderr}`);
  result = cargo([
    "add-member",
    "--username",
    "p7bplain",
    "--cooperative",
    "P7B Ana",
  ]);
  if (result.status !== 0) fail(`add-member plain: ${result.stderr}`);
  result = cargo([
    "add-member",
    "--username",
    "p7bcoopadmin",
    "--cooperative",
    "P7B Ana",
  ]);
  if (result.status !== 0) fail(`add-member coop admin: ${result.stderr}`);
  result = cargo([
    "add-member",
    "--username",
    "p7badmin",
    "--cooperative",
    "P7B Ana",
  ]);
  if (result.status !== 0) fail(`add-member admin: ${result.stderr}`);
}

if (
  psql(`SELECT count(*) FROM users WHERE username = 'p7busradmin'`, DB_NAME) !==
  "1"
) {
  mkuser("p7busradmin", "P7B Kullanıcı Yöneticisi", "p7b-parola-gizli-4");
}

// Adversarial fixture (idempotent reconcile, disposable DB only):
// p7bcoopadmin must hold cooperative-scope authority ONLY — a scoped
// clone of Sistem Yöneticisi inside coop A carrying the same keys,
// and never a NULL-scope assignment. The platform lane must reject it.
const coopAdminId = psql(
  `SELECT id FROM users WHERE username = 'p7bcoopadmin'`,
  DB_NAME,
);
const coopAId = psql(
  `SELECT id FROM cooperatives WHERE name = 'P7B Ana'`,
  DB_NAME,
);
psql(
  `DELETE FROM user_role_assignments \
    WHERE user_id = '${coopAdminId}' AND cooperative_id IS NULL`,
  DB_NAME,
);
psql(
  `INSERT INTO roles (name, cooperative_id) \
   SELECT 'Sistem Yöneticisi', '${coopAId}' \
    WHERE NOT EXISTS (SELECT 1 FROM roles \
       WHERE name = 'Sistem Yöneticisi' AND cooperative_id = '${coopAId}')`,
  DB_NAME,
);
const coopRoleId = psql(
  `SELECT id FROM roles \
    WHERE name = 'Sistem Yöneticisi' AND cooperative_id = '${coopAId}'`,
  DB_NAME,
);
psql(
  `INSERT INTO role_permissions (role_id, permission_id) \
   SELECT '${coopRoleId}', permission_id FROM role_permissions \
    WHERE role_id = (SELECT id FROM roles \
       WHERE name = 'Sistem Yöneticisi' AND cooperative_id IS NULL LIMIT 1) \
   ON CONFLICT DO NOTHING`,
  DB_NAME,
);
// Ensure the coop-scope clone carries the FORGED cooperatives.manage —
// the 0069 boundary rejects platform keys in definers, so this owner-lane
// INSERT models an out-of-band grant the lane must still ignore.
psql(
  `INSERT INTO role_permissions (role_id, permission_id) \
   SELECT '${coopRoleId}', id FROM permissions WHERE key = 'cooperatives.manage' \
   ON CONFLICT DO NOTHING`,
  DB_NAME,
);
psql(
  `INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
   VALUES ('${coopAdminId}', '${coopRoleId}', '${coopAId}') \
   ON CONFLICT DO NOTHING`,
  DB_NAME,
);

// Capability-split fixture (idempotent reconcile, disposable DB only):
// - p7badmin holds ONLY the seeded NULL-scope `platform-lifecycle` role
//   (NULL-scope cooperatives.manage, NO users.manage) plus a plain coop A
//   membership — the intended least-privilege lifecycle operator.
// - p7busradmin holds ONLY NULL-scope users.manage through the legacy
//   NULL-scope `Sistem Yöneticisi` seed — which must NOT carry
//   cooperatives.manage (an earlier fixture revision forged it there).
const adminId = psql(
  `SELECT id FROM users WHERE username = 'p7badmin'`,
  DB_NAME,
);
const usrAdminId = psql(
  `SELECT id FROM users WHERE username = 'p7busradmin'`,
  DB_NAME,
);
psql(`DELETE FROM user_role_assignments WHERE user_id = '${adminId}'`, DB_NAME);
psql(
  `INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
   SELECT '${adminId}', id, NULL FROM roles \
    WHERE cooperative_id IS NULL AND lower(name) = 'platform-lifecycle'`,
  DB_NAME,
);
psql(
  `INSERT INTO cooperative_memberships (cooperative_id, user_id, status) \
   VALUES ('${coopAId}', '${adminId}', 'active') \
   ON CONFLICT (cooperative_id, user_id) DO UPDATE \
     SET status = 'active', ended_at = NULL`,
  DB_NAME,
);
psql(
  `DELETE FROM user_role_assignments WHERE user_id = '${usrAdminId}'`,
  DB_NAME,
);
psql(
  `INSERT INTO user_role_assignments (user_id, role_id, cooperative_id) \
   SELECT '${usrAdminId}', id, NULL FROM roles \
    WHERE cooperative_id IS NULL AND name = 'Sistem Yöneticisi'`,
  DB_NAME,
);
// Un-forge the NULL-scope grant so the users.manage-only shape is honest;
// the coop-scope clone above keeps its forged copy.
psql(
  `DELETE FROM role_permissions rp USING roles r, permissions p \
    WHERE rp.role_id = r.id AND rp.permission_id = p.id \
      AND r.cooperative_id IS NULL AND r.name = 'Sistem Yöneticisi' \
      AND p.key = 'cooperatives.manage'`,
  DB_NAME,
);

// --- 2. Start stack ----------------------------------------------------
log("starting backend + web");
const server = spawnService(
  `cargo run --manifest-path "${ROOT}/apps/server/Cargo.toml" --quiet`,
  { env: serverEnv, stdio: "pipe" },
);
const web = spawnService(
  `pnpm --dir "${ROOT}/apps/web" dev --port ${WEB_PORT} --strictPort`,
  {
    env: { ...process.env, PUBLIC_API_BASE_URL: API },
    stdio: "pipe",
  },
);
const serverLog = logStream("server");
const webLog = logStream("web");
for (const [child, stream, tag] of [
  [server, serverLog, "backend"],
  [web, webLog, "frontend"],
]) {
  child.stdout?.on("data", (d) => stream?.write(redact(d)));
  child.stderr?.on("data", (d) => {
    const line = redact(d);
    stream?.write(line);
    if (process.env.E2E_VERBOSE) process.stderr.write(line);
  });
  child.on("error", async (e) => {
    await shutdown();
    die(`${tag} spawn failed: ${e.message}`);
  });
}
// M1-P8-B follow-up: `browser` is hoisted so the finally path can close
// it on ANY exit — success, assertion failure or throw. Without this a
// failed journey left the Playwright driver alive, which kept the Node
// event loop busy and hung the suite until the CI job timeout killed it
// (observed as a 30-minute "cancelled" instead of a fast FAIL).
let browser;
/** Bounded browser close — cleanup must never hang nor mask the original failure. */
const closeBrowser = async () => {
  if (!browser) return;
  const b = browser;
  browser = null;
  await Promise.race([b.close().catch(() => {}), sleep(15000)]);
};
const shutdown = async () => {
  await closeBrowser();
  await killService(server);
  await killService(web);
};
process.on("SIGINT", async () => {
  await shutdown();
  process.exit(1);
});

async function login(page, username, password) {
  const response = await page.context().request.post(`${API}/api/auth/login`, {
    headers: { "content-type": "application/json", origin: WEB },
    data: { username, password },
  });
  if (!response.ok()) fail(`login ${username}: ${response.status()}`);
  await page.goto(`${WEB}/`);
  await page.waitForLoadState("networkidle");
}

const headerHasCoop = async (page, name, timeout = 20000) =>
  page.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    name,
    { timeout },
  );

const plainId = psql(
  `SELECT id FROM users WHERE username = 'p7bplain'`,
  DB_NAME,
);
const COOP_A = "P7B Ana";
// Unique per run: the lifecycle journey drives this coop through every
// status, so a fixed name would collide with a previous run's coop.
const COOP_NEW = `P7B Yeni ${Date.now().toString(36)}`;

try {
  await waitFor(`${API}/health`, 120000, "backend /health");
  await waitFor(`${WEB}/login`, 120000, "web /login");
  log("stack ready");

  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.E2E_CHROMIUM_EXECUTABLE || undefined,
  });
  const ctx = await browser.newContext({ locale: "tr-TR" });
  const page = await ctx.newPage();
  page.on("dialog", (dialog) => dialog.accept());

  // --- J1: least-privilege platform-lifecycle operator sees the lane --
  // p7badmin holds ONLY NULL-scope cooperatives.manage (seeded
  // `platform-lifecycle` role) — never NULL-scope users.manage.
  await login(page, "p7badmin", "p7b-parola-gizli-1");
  const meFlags = await evalResilient(page, async (api) => {
    const res = await fetch(`${api}/api/auth/me`, { credentials: "include" });
    const body = await res.json();
    return {
      status: res.status,
      platformAdmin: body.platformAdmin,
      platformCooperativesManage: body.platformCooperativesManage,
    };
  }, API);
  check(
    "lifecycle operator: cooperatives.manage flag on",
    meFlags.platformCooperativesManage === true,
  );
  check(
    "lifecycle operator: users.manage flag off",
    meFlags.platformAdmin === false,
  );
  await page.goto(`${WEB}/platform/kooperatifler`);
  await page.waitForLoadState("networkidle");
  await page.locator("#main-content").getByText(COOP_A).first().waitFor();
  check("platform list renders cooperatives", true);
  check(
    "platform nav group is visible to platform admin",
    await page
      .getByRole("link", { name: "Kooperatifler", exact: true })
      .first()
      .isVisible(),
  );

  // --- J2: create → provisioning --------------------------------------
  await page.locator("#coop-name").fill(COOP_NEW);
  await page.locator("#coop-legal").fill("P7B Yeni Ltd.");
  await page.getByRole("button", { name: "Kooperatifi Oluştur" }).click();
  await page
    .locator("#main-content")
    .getByText(COOP_NEW)
    .first()
    .waitFor({ timeout: 15000 });
  check("created cooperative appears in the list", true);

  // --- J3: detail → activate with initial admin ------------------------
  await page
    .getByRole("row", { name: new RegExp(COOP_NEW) })
    .getByRole("link", { name: "Detay" })
    .click();
  await page.locator("#main-content").getByText(COOP_NEW).first().waitFor();
  // Invalid id stays client-side.
  await page.getByRole("button", { name: "Etkinleştir" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Etkinleştir" })
    .click();
  await page.getByText(/Geçerli bir kullanıcı kimliği/).waitFor();
  check("activation rejects a missing UUID client-side", true);
  await page.locator("#initial-admin-id").fill(plainId);
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Etkinleştir" })
    .click();
  await page
    .locator("#main-content")
    .getByText("Aktif", { exact: true })
    .waitFor({ timeout: 15000 });
  check("activation lands on active status", true);

  // --- J4: suspend → reactivate → archive → restore --------------------
  await page.getByRole("button", { name: "Askıya Al" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Askıya Al" })
    .click();
  await page
    .locator("#main-content")
    .getByText("Askıda", { exact: true })
    .waitFor({ timeout: 15000 });
  check("suspend lands on suspended", true);

  await page.getByRole("button", { name: "Etkinleştir" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Etkinleştir" })
    .click();
  await page
    .locator("#main-content")
    .getByText("Aktif", { exact: true })
    .waitFor({ timeout: 15000 });
  check("reactivate lands on active", true);

  await page.getByRole("button", { name: "Arşivle" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Arşivle" })
    .click();
  await page
    .locator("#main-content")
    .getByText("Arşivli", { exact: true })
    .waitFor({ timeout: 15000 });
  check("archive lands on archived", true);

  await page.getByRole("button", { name: "Etkinleştir" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Etkinleştir" })
    .click();
  await page
    .locator("#main-content")
    .getByText("Aktif", { exact: true })
    .waitFor({ timeout: 15000 });
  check("restore from archive lands on active", true);

  // --- J5: bootstrap cooperative can never be archived ------------------
  await page.goto(`${WEB}/platform/kooperatifler`);
  await page
    .getByRole("row", { name: new RegExp(COOP_A) })
    .getByRole("link", { name: "Detay" })
    .click();
  await page.locator("#main-content").getByText(COOP_A).first().waitFor();
  check(
    "bootstrap cooperative never offers archive",
    (await page.getByRole("button", { name: "Arşivle" }).count()) === 0,
  );

  // --- J6: session scope untouched by platform operations --------------
  await headerHasCoop(page, COOP_A);
  check("platform ops never changed the session's cooperative", true);

  // --- J7: unauthorized surfaces ---------------------------------------
  const ctxPlain = await browser.newContext({ locale: "tr-TR" });
  const plainPage = await ctxPlain.newPage();
  await login(plainPage, "p7bplain", "p7b-parola-gizli-3");
  await plainPage.goto(`${WEB}/platform/kooperatifler`);
  await plainPage
    .getByText(/platform düzeyinde kooperatif yönetimi yetkisi/)
    .waitFor({ timeout: 15000 });
  check("plain member sees the localized denial", true);
  check(
    "platform nav is hidden from plain member",
    (await plainPage
      .getByRole("link", { name: "Kooperatifler", exact: true })
      .count()) === 0,
  );
  const plainProbe = await evalResilient(plainPage, async (api) => {
    const res = await fetch(`${api}/api/platform/cooperatives`, {
      credentials: "include",
    });
    return res.status;
  }, API);
  check("backend rejects the plain member's platform read", plainProbe === 403);

  // NULL-scope users.manage without cooperatives.manage: the P7-B
  // mismatch-B shape. platformAdmin stays true (users lane intact) but
  // the cooperative lane must be hidden and the API must still 403.
  const ctxUsr = await browser.newContext({ locale: "tr-TR" });
  const usrPage = await ctxUsr.newPage();
  await login(usrPage, "p7busradmin", "p7b-parola-gizli-4");
  const usrFlags = await evalResilient(usrPage, async (api) => {
    const res = await fetch(`${api}/api/auth/me`, { credentials: "include" });
    const body = await res.json();
    return {
      platformAdmin: body.platformAdmin,
      platformCooperativesManage: body.platformCooperativesManage,
    };
  }, API);
  check(
    "users.manage-only: platformAdmin flag on",
    usrFlags.platformAdmin === true,
  );
  check(
    "users.manage-only: cooperatives.manage flag off",
    usrFlags.platformCooperativesManage === false,
  );
  await usrPage.goto(`${WEB}/platform/kooperatifler`);
  await usrPage
    .getByText(/platform düzeyinde kooperatif yönetimi yetkisi/)
    .waitFor({ timeout: 15000 });
  check("users.manage-only operator sees the localized denial", true);
  check(
    "platform nav is hidden from users.manage-only operator",
    (await usrPage
      .getByRole("link", { name: "Kooperatifler", exact: true })
      .count()) === 0,
  );
  const usrProbe = await evalResilient(usrPage, async (api) => {
    const res = await fetch(`${api}/api/platform/cooperatives`, {
      credentials: "include",
    });
    return res.status;
  }, API);
  check(
    "backend rejects the users.manage-only platform read",
    usrProbe === 403,
  );

  // Cooperative-scoped cooperatives.manage must NOT open the lane —
  // the adversarial fixture grants the key inside coop A only.
  const ctxCoop = await browser.newContext({ locale: "tr-TR" });
  const coopPage = await ctxCoop.newPage();
  await login(coopPage, "p7bcoopadmin", "p7b-parola-gizli-2");
  await coopPage.goto(`${WEB}/platform/kooperatifler`);
  await coopPage
    .getByText(/platform düzeyinde kooperatif yönetimi yetkisi/)
    .waitFor({ timeout: 15000 });
  check("coop admin sees the localized denial", true);
  const coopProbe = await evalResilient(coopPage, async (api) => {
    const res = await fetch(`${api}/api/platform/cooperatives`, {
      credentials: "include",
    });
    return res.status;
  }, API);
  check(
    "cooperative-scoped cooperatives.manage is rejected by the lane",
    coopProbe === 403,
  );

  // --- J8: P7-A regression — suspended own coop, platform lane keeps working
  const coopAId = psql(
    `SELECT id FROM cooperatives WHERE name = '${COOP_A}'`,
    DB_NAME,
  );
  // Admin's session scope is A; suspending A must trigger the recovery
  // panel on business routes but never block the platform lane.
  await page.goto(`${WEB}/platform/kooperatifler`);
  await page.locator("#main-content").getByText(COOP_A).first().waitFor();
  psql(`SELECT app_cooperative_set_status('${coopAId}', 'suspended')`, DB_NAME);
  const biz = await ctx.newPage();
  await biz.goto(`${WEB}/gelirler`);
  await biz
    .getByText(/Kooperatif erişimi kaldırıldı|Kullanılabilir kooperatif yok/)
    .waitFor({ timeout: 15000 });
  check("suspended active coop masks business page", true);
  // Platform lane remains reachable without selecting a cooperative.
  await page.reload();
  await page
    .locator("#main-content")
    .getByText(COOP_NEW)
    .first()
    .waitFor({ timeout: 15000 });
  check("platform lane survives the revoked own-scope", true);
  psql(`SELECT app_cooperative_set_status('${coopAId}', 'active')`, DB_NAME);

  // --- J9: mobile layout ------------------------------------------------
  const mobile = await browser.newContext({
    locale: "tr-TR",
    viewport: { width: 375, height: 812 },
    hasTouch: true,
  });
  const mobPage = await mobile.newPage();
  await login(mobPage, "p7badmin", "p7b-parola-gizli-1");
  await mobPage.goto(`${WEB}/platform/kooperatifler`);
  await mobPage
    .locator("#main-content")
    .getByText(COOP_A)
    .first()
    .waitFor({ timeout: 20000 });
  check("mobile platform list renders", true);
  await mobile.close();

  await ctxPlain.close();
  await ctxUsr.close();
  await ctxCoop.close();
  await closeBrowser();
  if (process.exitCode !== 1) log("ALL JOURNEYS PASSED");
} catch (error) {
  fail(String(error?.stack ?? error));
} finally {
  await shutdown();
}
