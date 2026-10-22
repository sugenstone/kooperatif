// M1-P7-A — real-browser E2E through the real stack
// (PostgreSQL + Rust API + SvelteKit + Chromium).
//
//   Disposable DB kooperatif_e2e_p7a (created only if absent — never
//   dropped) → migrate → users → bootstrap cooperative → a second
//   cooperative created+activated INSIDE the disposable DB via the
//   authorized owner lane → a third cooperative suspended → journeys:
//     T7.3 active-coop visibility on desktop/mobile/deep-link,
//     T7.2 dirty-form guards on switch, in-app navigation and
//     beforeunload, superseded-T7.1 cross-tab convergence with an open
//     dirty form, T7.4 revoked access with/without an eligible
//     cooperative (realtime-on and realtime-off), cooperative
//     suspension masking and stale-content prevention.
//
// Run: node scripts/e2e-m1p7-a.mjs   (requires Docker postgres up)
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
const DB_NAME = process.env.E2E_DB_NAME ?? "kooperatif_e2e_p7a";
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

const log = (msg) => console.log(`[p7a-e2e] ${msg}`);
const fail = (msg) => {
  console.error(`[p7a-e2e] FAIL: ${msg}`);
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
// Mutating requests are never retried: a destroyed execution context
// cannot prove whether the POST was dispatched, so a retry could
// double-submit. A lost context becomes a visible null (failed check),
// not a silent re-fire.
const evalOnce = async (pg, fn, arg) => {
  try {
    return await pg.evaluate(fn, arg);
  } catch (error) {
    if (
      !/Execution context was destroyed|Cannot find object with id/.test(
        String(error),
      )
    )
      throw error;
    await pg.waitForLoadState("load").catch(() => {});
    return null;
  }
};


const compose = (args) =>
  spawnSync(
    "docker",
    ["compose", "-f", `${ROOT}/docker/compose.yaml`, ...args],
    {
      stdio: "pipe",
      encoding: "utf8",
    },
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

// --- 1. Fresh disposable database (never dropped) ----------------------
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
  const coops = psql(
    `SELECT count(*) FROM cooperatives WHERE name IN ('P7A Ana','P7A İkinci','P7A Askılı')`,
    DB_NAME,
  );
  const users = psql(
    `SELECT count(*) FROM users WHERE username IN ('p7aadmin','p7adual','p7asolo')`,
    DB_NAME,
  );
  if (migrated === "1" && coops === "3" && users === "3") {
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


if (!fixtureReady) {
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
  mkuser("p7aadmin", "P7A Yöneticisi", "p7a-parola-gizli-1");
  mkuser("p7adual", "P7A Çift Üye", "p7a-parola-gizli-2");
  mkuser("p7asolo", "P7A Tek Üye", "p7a-parola-gizli-3");
  result = cargo(["bootstrap-cooperative", "--name", "P7A Ana"]);
  if (result.status !== 0) fail(`bootstrap-cooperative: ${result.stderr}`);
  result = cargo([
    "grant-role",
    "--username",
    "p7aadmin",
    "--role",
    "Sistem Yöneticisi",
  ]);
  if (result.status !== 0) fail(`grant-role: ${result.stderr}`);

  // Second cooperative inside the DISPOSABLE test database only.
  result = cargo(["create-cooperative", "--name", "P7A İkinci"]);
  if (result.status !== 0) fail(`create-cooperative: ${result.stderr}`);
  const coopB = psql(
    `SELECT id FROM cooperatives WHERE name = 'P7A İkinci'`,
    DB_NAME,
  );
  const adminId = psql(
    `SELECT id FROM users WHERE username = 'p7aadmin'`,
    DB_NAME,
  );
  const activated = psql(
    `SELECT app_cooperative_activate('${coopB}', '${adminId}')`,
    DB_NAME,
  );
  if (activated !== coopB) fail(`activate cooperative B: ${activated}`);
  // p7adual is a plain member of BOTH cooperatives (revoke recovery).
  result = cargo([
    "add-member",
    "--username",
    "p7adual",
    "--cooperative",
    "P7A İkinci",
  ]);
  if (result.status !== 0) fail(`add-member dual→B: ${result.stderr}`);

  // Third cooperative suspended — disabled-item evidence.
  result = cargo(["create-cooperative", "--name", "P7A Askılı"]);
  if (result.status !== 0) fail(`create-cooperative susp: ${result.stderr}`);
  const coopC = psql(
    `SELECT id FROM cooperatives WHERE name = 'P7A Askılı'`,
    DB_NAME,
  );
  psql(`SELECT app_cooperative_activate('${coopC}', '${adminId}')`, DB_NAME);
  psql(`SELECT app_cooperative_set_status('${coopC}', 'suspended')`, DB_NAME);
}

// Fixture reuse safety: a previously interrupted run may have left the
// fixture memberships/cooperatives suspended — restore the baseline.
const coopBId = psql(
  `SELECT id FROM cooperatives WHERE name = 'P7A İkinci'`,
  DB_NAME,
);
const dualId = psql(`SELECT id FROM users WHERE username = 'p7adual'`, DB_NAME);
const soloId = psql(`SELECT id FROM users WHERE username = 'p7asolo'`, DB_NAME);
psql(
  `UPDATE cooperative_memberships SET status='active' WHERE status='suspended' AND user_id IN ('${dualId}','${soloId}')`,
  DB_NAME,
);
psql(
  `UPDATE cooperatives SET status='active', business_enabled=true WHERE name='P7A İkinci' AND (status<>'active' OR NOT business_enabled)`,
  DB_NAME,
);

log("starting backend + web");
const server = spawnService(
  `cargo run --manifest-path "${ROOT}/apps/server/Cargo.toml" --quiet`,
  {
    env: serverEnv,
    stdio: "pipe",
  },
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

const shutdown = async () => {
  await killService(server);
  await killService(web);
};
process.on("SIGINT", async () => {
  await shutdown();
  process.exit(1);
});

const COOP_A = "P7A Ana";
const COOP_B = "P7A İkinci";
const switcher = (page) =>
  page.getByRole("button", { name: "Kooperatif değiştir" });
const headerHasCoop = async (page, name, timeout = 20000) =>
  page.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    name,
    { timeout },
  );

async function login(page, username, password) {
  const response = await page.context().request.post(`${API}/api/auth/login`, {
    headers: { "content-type": "application/json", origin: WEB },
    data: { username, password },
  });
  if (!response.ok()) fail(`login ${username}: ${response.status()}`);
  await page.goto(`${WEB}/`);
  await page.waitForLoadState("networkidle");
}

/** POST /api/auth/cooperative from inside a logged-in page (audited API). */
async function apiSwitch(page, coopId) {
  const status = await evalOnce(page,
    async ({ api, coopId }) => {
      const me = await fetch(`${api}/api/auth/me`, {
        credentials: "include",
      }).then((r) => r.json());
      const context = await fetch(`${api}/api/auth/cooperative-context`, {
        credentials: "include",
      }).then((r) => r.json());
      const response = await fetch(`${api}/api/auth/cooperative`, {
        method: "POST",
        credentials: "include",
        headers: {
          "content-type": "application/json",
          "x-csrf-token": me.csrfToken,
          "x-cooperative-id": context.cooperativeId,
        },
        body: JSON.stringify({ cooperativeId: coopId }),
      });
      return response.status;
    },
    { api: API, coopId },
  );
  return status;
}

/** POST /api/users/{id}/membership-suspend inside the admin's scope. */
async function apiSuspendMembership(page, userId) {
  const status = await evalOnce(page,
    async ({ api, userId }) => {
      const me = await fetch(`${api}/api/auth/me`, {
        credentials: "include",
      }).then((r) => r.json());
      const context = await fetch(`${api}/api/auth/cooperative-context`, {
        credentials: "include",
      }).then((r) => r.json());
      const response = await fetch(
        `${api}/api/users/${userId}/membership-suspend`,
        {
          method: "POST",
          credentials: "include",
          headers: {
            "content-type": "application/json",
            "x-csrf-token": me.csrfToken,
            "x-cooperative-id": context.cooperativeId,
          },
          body: JSON.stringify({}),
        },
      );
      return response.status;
    },
    { api: API, userId },
  );
  return status;
}

/** Any cooperative-scoped business GET — a 403 fires the access-loss seam. */
async function apiProbe(page) {
  return evalResilient(page, async (api) => {
    const ctxRes = await fetch(`${api}/api/auth/cooperative-context`, {
      credentials: "include",
    });
    const context = ctxRes.status === 200 ? await ctxRes.json() : {};
    const res = await fetch(`${api}/api/shareholders`, {
      credentials: "include",
      headers: context.cooperativeId
        ? { "x-cooperative-id": context.cooperativeId }
        : {},
    });
    return { ctx: ctxRes.status, probe: res.status };
  }, API);
}

try {
  await waitFor(`${API}/health`, 120000, "backend /health");
  await waitFor(`${WEB}/login`, 120000, "web /login");
  log("stack ready");

  const browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.E2E_CHROMIUM_EXECUTABLE || undefined,
  });
  const ctx = await browser.newContext({ locale: "tr-TR" });
  const page = await ctx.newPage();
  page.on("dialog", (dialog) => dialog.accept());

  // --- K1: deep-link — active cooperative visible on a direct URL ----
  await login(page, "p7aadmin", "p7a-parola-gizli-1");
  await page.goto(`${WEB}/gelirler/yeni`);
  await switcher(page).waitFor();
  await headerHasCoop(page, COOP_A);
  check("deep link shows authoritative cooperative name in header", true);

  // --- K2: dirty-form switch — Cancel keeps scope + values -----------
  // networkidle settles the page's load fetches — hydration is then
  // guaranteed done, so a fill cannot be reset to the pristine state.
  await page.waitForLoadState("networkidle");
  await page.locator("#inc-description").fill("p7a kirli form");
  let coopPosts = 0;
  await page.route("**/api/auth/cooperative", (route) => {
    coopPosts += 1;
    route.continue();
  });
  await switcher(page).click();
  await page.getByRole("menuitem", { name: new RegExp(COOP_B) }).click();
  await page.getByRole("alertdialog").waitFor();
  check("dirty switch asks for confirmation", true);
  check(
    "dialog explains data loss in Turkish",
    (await page.getByRole("alertdialog").textContent()).includes(
      "Kaydedilmemiş",
    ),
  );
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Vazgeç" })
    .click();
  await sleep(300);
  check(
    "cancel posts no switch",
    coopPosts === 0 &&
      (await page.locator("#inc-description").inputValue()) ===
        "p7a kirli form",
  );
  check(
    "cancel keeps cooperative scope",
    (await switcher(page).textContent()).includes(COOP_A),
  );

  // --- K3: dirty-form switch — Continue runs the authorized switch ---
  await switcher(page).click();
  await page.getByRole("menuitem", { name: new RegExp(COOP_B) }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Devam et" })
    .click();
  await headerHasCoop(page, COOP_B);
  check("confirmed dirty switch posts and lands on B", coopPosts === 1);
  await page.unroute("**/api/auth/cooperative");

  // --- K4: in-app navigation guard -------------------------------------
  await page.goto(`${WEB}/gelirler/yeni`);
  await page.waitForLoadState("networkidle");
  await page.locator("#inc-description").fill("nav guard");
  await page.getByRole("link", { name: "Hissedarlar" }).first().click();
  await page.getByRole("alertdialog").waitFor();
  check("in-app navigation guarded while dirty", true);
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Vazgeç" })
    .click();
  await sleep(300);
  check(
    "cancel keeps the page and form values",
    page.url().includes("/gelirler/yeni") &&
      (await page.locator("#inc-description").inputValue()) === "nav guard",
  );
  await page.getByRole("link", { name: "Hissedarlar" }).first().click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Devam et" })
    .click();
  await page.waitForURL("**/hissedarlar**");
  check("confirmed navigation leaves the form", true);

  // --- K5: browser-level protection (beforeunload) --------------------
  await page.goto(`${WEB}/gelirler/yeni`);
  await page.waitForLoadState("networkidle");
  await page.locator("#inc-description").fill("unload guard");
  const dirtyTab = await ctx.newPage();
  await dirtyTab.goto(`${WEB}/gelirler/yeni`);
  await dirtyTab.waitForLoadState("networkidle");
  await dirtyTab.locator("#inc-description").fill("unload guard");
  let dismissClose = true;
  dirtyTab.on("dialog", async (dialog) => {
    if (dialog.type() === "beforeunload") {
      if (dismissClose) await dialog.dismiss();
      else await dialog.accept();
    } else await dialog.accept();
  });
  await dirtyTab.close({ runBeforeUnload: true }).catch(() => {});
  await sleep(400);
  check("beforeunload cancel keeps the dirty tab open", !dirtyTab.isClosed());
  dismissClose = false;
  await dirtyTab.close({ runBeforeUnload: true }).catch(() => {});
  await sleep(400);
  check("beforeunload accept closes the tab", dirtyTab.isClosed());

  // --- K6: mobile viewport — context still visible --------------------
  const mobile = await browser.newContext({
    locale: "tr-TR",
    viewport: { width: 375, height: 812 },
    hasTouch: true,
  });
  const mobPage = await mobile.newPage();
  await login(mobPage, "p7aadmin", "p7a-parola-gizli-1");
  const mobTrigger = switcher(mobPage);
  await mobTrigger.waitFor();
  check("mobile header shows active cooperative", await mobTrigger.isVisible());
  await mobile.close();

  // --- K7: cross-tab convergence with an open dirty form (T7.1′) ------
  // Session-authoritative model: tab2 CANNOT veto the server-committed
  // switch — it must mask its dirty form and converge to the new scope.
  const tab2 = await ctx.newPage();
  await tab2.goto(`${WEB}/gelirler/yeni`);
  await tab2.waitForLoadState("networkidle");
  await tab2.locator("#inc-description").fill("tab2 kirli");
  // tab1 (admin page, currently scoped B) switches back to A.
  const switchStatus = await apiSwitch(
    page,
    psql(`SELECT id FROM cooperatives WHERE name='${COOP_A}'`, DB_NAME),
  );
  check("tab1 server-side switch to A succeeded", switchStatus === 204);
  // tab2's local switch broadcast only happens on UI switch — emulate the
  // sibling hint via the app's own broadcast channel message OR let the
  // authz realtime signal drive it; either way tab2 must converge.
  await headerHasCoop(tab2, COOP_A, 30000);
  check("tab2 converged to A despite an open dirty form", true);

  // --- K8: revoked access WITH another eligible coop (realtime OFF) ---
  const ctxDual = await browser.newContext({ locale: "tr-TR" });
  const dualPage = await ctxDual.newPage();
  dualPage.on("dialog", (d) => d.accept());
  await login(dualPage, "p7adual", "p7a-parola-gizli-2");
  await headerHasCoop(dualPage, COOP_A);
  // Realtime unavailable: only the 403 seam can reveal the revocation.
  await dualPage.route("**/api/realtime**", (route) => route.abort());
  // Admin suspends dual's A membership through the real audited API
  // (admin session is scoped to A after K7's switch).
  const suspStatus = await apiSuspendMembership(page, dualId);
  check("membership suspend API succeeded", suspStatus === 204);
  // The next scoped request fails closed → resync → recovery panel.
  const probe = await apiProbe(dualPage);
  check(
    `revoked scope fails closed (ctx=${probe.ctx}, probe=${probe.probe})`,
    probe.probe === 403,
  );
  // With realtime aborted, the access-loss seam only fires through the
  // app's own client (apiFetch). A raw fetch would bypass it — resolve
  // the app's actual module in dev mode and issue the next request.
  const seamFired = await evalResilient(dualPage, async () => {
    try {
      const { apiFetch } = await import("/src/lib/api-client.ts");
      try {
        await apiFetch("/api/shareholders");
      } catch {
        /* expected cooperative_access_denied */
      }
      return true;
    } catch {
      return false;
    }
  });
  if (!seamFired) await dualPage.reload(); // module import unavailable
  // dual's session field was never set (bootstrap resolution) — the
  // context fails closed and the panel shows the neutral selection
  // state with the surviving cooperative offered explicitly.
  await dualPage
    .getByText("Kooperatif seçimi gerekli")
    .waitFor({ timeout: 15000 });
  check("revocation masks stale content", true);
  // The seam resync adopts the revoked scope through a hard navigation —
  // wait for the post-reload panel before probing the recovery button.
  await dualPage
    .getByRole("button", { name: new RegExp(COOP_B) })
    .waitFor({ timeout: 15000 });
  check("recovery offers the surviving cooperative explicitly", true);
  await dualPage.getByRole("button", { name: new RegExp(COOP_B) }).click();
  await headerHasCoop(dualPage, COOP_B);
  check("explicit recovery selection lands on B", true);
  await dualPage.waitForLoadState("networkidle");
  const meDual = await evalResilient(dualPage, async (api) => {
    const me = await fetch(`${api}/api/auth/me`, {
      credentials: "include",
    }).then((r) => r.json());
    return me.activeCooperativeId;
  }, API);
  check("authoritative scope is now B", meDual === coopBId);
  // Restore the fixture for reuse.
  psql(
    `UPDATE cooperative_memberships SET status='active' WHERE user_id='${dualId}' AND cooperative_id=(SELECT id FROM cooperatives WHERE name='${COOP_A}')`,
    DB_NAME,
  );

  // --- K9: revoked access with NO eligible cooperative -----------------
  const ctxSolo = await browser.newContext({ locale: "tr-TR" });
  const soloPage = await ctxSolo.newPage();
  await login(soloPage, "p7asolo", "p7a-parola-gizli-3");
  await headerHasCoop(soloPage, COOP_A);
  const suspSolo = await apiSuspendMembership(page, soloId);
  check("solo membership suspend succeeded", suspSolo === 204);
  const probeSolo = await apiProbe(soloPage);
  check("solo revoked scope fails closed", probeSolo.probe === 403);
  // solo holds zero permission scopes — no realtime socket exists, so
  // the seam (next apiFetch 403) is the only in-page detector here.
  const soloSeam = await evalResilient(soloPage, async () => {
    try {
      const { apiFetch } = await import("/src/lib/api-client.ts");
      try {
        await apiFetch("/api/shareholders");
      } catch {
        /* expected cooperative_access_denied */
      }
      return true;
    } catch {
      return false;
    }
  });
  if (!soloSeam) await soloPage.reload();
  await soloPage
    .getByText("Kullanılabilir kooperatif yok")
    .waitFor({ timeout: 15000 });
  check(
    "no eligible cooperative → unavailable state + logout path",
    await soloPage
      .getByRole("button", { name: "Çıkış Yap" })
      .last()
      .isVisible(),
  );
  check(
    "no recovery cooperative is offered",
    (await soloPage
      .getByRole("button", { name: new RegExp(COOP_B) })
      .count()) === 0,
  );
  psql(
    `UPDATE cooperative_memberships SET status='active' WHERE user_id='${soloId}' AND cooperative_id=(SELECT id FROM cooperatives WHERE name='${COOP_A}')`,
    DB_NAME,
  );

  // --- K10: cooperative suspension masks + stale-data prevention ------
  // Admin session is scoped to A; switch it to B, suspend coop B
  // entirely, then reload — the recovery panel must replace every
  // cooperative view (stale rows must not render).
  const switchB = await apiSwitch(page, coopBId);
  check("admin switch to B", switchB === 204);
  // The switch's own authz event may still be driving a hard navigation
  // on this tab — let it settle before suspending cooperative B.
  await page.waitForLoadState("networkidle");
  psql(`SELECT app_cooperative_set_status('${coopBId}', 'suspended')`, DB_NAME);
  // Realtime path: the cooperatives authz event must mask B's content
  // in place (scope is still B — now inaccessible → revoked panel).
  await page
    .getByText("Kooperatif erişimi kaldırıldı")
    .waitFor({ timeout: 15000 });
  check("realtime authz masks suspended cooperative in place", true);
  // Reload under the revoked scope: revalidation must mask again — no
  // stale cooperative data may render while access is denied.
  await page.goto(`${WEB}/uyelikler`);
  await page
    .getByText("Kooperatif erişimi kaldırıldı")
    .waitFor({ timeout: 15000 });
  check("suspended cooperative masks content on load", true);
  check(
    "no stale business rows while revoked",
    (await page.getByRole("cell").count()) === 0,
  );
  check(
    "recovery offers cooperative A",
    await page.getByRole("button", { name: new RegExp(COOP_A) }).isVisible(),
  );
  // Restore coop B BEFORE choosing recovery so the fixture stays reusable.
  psql(`SELECT app_cooperative_set_status('${coopBId}', 'active')`, DB_NAME);
  await page.getByRole("button", { name: new RegExp(COOP_A) }).click();
  await headerHasCoop(page, COOP_A);
  check("recovery from suspended coop lands on A", true);

  await ctxDual.close();
  await ctxSolo.close();
  await browser.close();
  if (process.exitCode !== 1) log("ALL JOURNEYS PASSED");
} catch (error) {
  fail(String(error?.stack ?? error));
} finally {
  await shutdown();
}
