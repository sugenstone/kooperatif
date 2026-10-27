// M1-P6 B5 — real-browser E2E through the real stack
// (PostgreSQL + Rust API + SvelteKit + Chromium).
//
//   Disposable DB kooperatif_e2e_b5 (created only if absent — never
//   dropped) → migrate → users → bootstrap cooperative → second
//   cooperative created+activated INSIDE the disposable DB via the
//   authorized owner lane → a third cooperative suspended for
//   disabled-item evidence → real browser journeys:
//     selector on desktop + mobile, active marking, successful switch,
//     rejected switch (disabled item + direct API 403), memberships
//     page + permission-gated nav, lifecycle actions, unauthorized
//     access, two tabs sharing one session, switch with WebSocket
//     connected and unavailable, refresh restoration, stale-data
//     prevention, keyboard accessibility.
//
// Run: node scripts/e2e-m1p6-b5.mjs   (requires Docker postgres up)
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
const DB_NAME = process.env.E2E_DB_NAME ?? "kooperatif_e2e_b5";
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

const log = (msg) => console.log(`[b5-e2e] ${msg}`);
const fail = (msg) => {
  console.error(`[b5-e2e] FAIL: ${msg}`);
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
  // The stamp must be VERIFIED as written, not assumed: spawnSync
  // failures (container mid-restart, transient exec refusal) used to
  // silently yield an empty stdout and surface only as the mismatch
  // below — an opaque flake. Check the exec status and retry once.
  const stamp = (marker) =>
    compose([
      "exec",
      "-T",
      "postgres",
      "psql",
      "-U",
      PG_USER,
      "-d",
      "postgres",
      "-Atc",
      `COMMENT ON DATABASE postgres IS '${marker}'`,
    ]);
  let runMarker = "";
  let marked = false;
  for (let attempt = 0; attempt < 2 && !marked; attempt++) {
    runMarker = `kooperatif-e2e-run:${randomUUID()}`;
    const wrote = stamp(runMarker);
    if (wrote.status !== 0) continue;
    marked =
      psql(
        `SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = 'postgres'`,
      ) === runMarker;
  }
  if (!marked)
    die("ephemeral run-marker mismatch — target is not this run's cluster");
  log(`ephemeral instance verified (${runMarker})`);
}

const exists = psql(
  `SELECT count(*) FROM pg_database WHERE datname = '${DB_NAME}'`,
);
let fixtureReady = false;
if (exists === "1") {
  // Reuse is allowed ONLY when the database provably carries this
  // script's own fixture signature — anything else is preserved.
  const migrated = psql(
    `SELECT count(*) FROM _sqlx_migrations WHERE version = 72 AND success`,
    DB_NAME,
  );
  const coops = psql(
    `SELECT count(*) FROM cooperatives WHERE name IN ('Ana Kooperatif','İkinci Kooperatif','Askılı Kooperatif')`,
    DB_NAME,
  );
  const users = psql(
    `SELECT count(*) FROM users WHERE username IN ('b5admin','b5member','b5plain')`,
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


let coopB;
let coopC;
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
  mkuser("b5admin", "B5 Yöneticisi", "b5-parola-gizli-1");
  mkuser("b5member", "B5 Üyesi", "b5-parola-gizli-2");
  mkuser("b5plain", "B5 İzinsiz", "b5-parola-gizli-3");
  // Enroll every active user into the bootstrap cooperative, then grant.
  result = cargo(["bootstrap-cooperative", "--name", "Ana Kooperatif"]);
  if (result.status !== 0) fail(`bootstrap-cooperative: ${result.stderr}`);
  result = cargo([
    "grant-role",
    "--username",
    "b5admin",
    "--role",
    "Sistem Yöneticisi",
  ]);
  if (result.status !== 0) fail(`grant-role: ${result.stderr}`);

  // Second cooperative inside the DISPOSABLE test database only: create
  // (provisioning) then activate via the authorized owner lane.
  result = cargo(["create-cooperative", "--name", "İkinci Kooperatif"]);
  if (result.status !== 0) fail(`create-cooperative: ${result.stderr}`);
  coopB = psql(
    `SELECT id FROM cooperatives WHERE name = 'İkinci Kooperatif'`,
    DB_NAME,
  );
  const adminId = psql(
    `SELECT id FROM users WHERE username = 'b5admin'`,
    DB_NAME,
  );
  const activated = psql(
    `SELECT app_cooperative_activate('${coopB}', '${adminId}')`,
    DB_NAME,
  );
  if (activated !== coopB) fail(`activate cooperative B: ${activated}`);

  // Third cooperative suspended — selector must show it disabled.
  result = cargo(["create-cooperative", "--name", "Askılı Kooperatif"]);
  if (result.status !== 0) fail(`create-cooperative susp: ${result.stderr}`);
  coopC = psql(
    `SELECT id FROM cooperatives WHERE name = 'Askılı Kooperatif'`,
    DB_NAME,
  );
  psql(`SELECT app_cooperative_activate('${coopC}', '${adminId}')`, DB_NAME);
  psql(`SELECT app_cooperative_set_status('${coopC}', 'suspended')`, DB_NAME);
}

coopB = psql(
  `SELECT id FROM cooperatives WHERE name = 'İkinci Kooperatif'`,
  DB_NAME,
);
coopC = psql(
  `SELECT id FROM cooperatives WHERE name = 'Askılı Kooperatif'`,
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

const COOP_NAME_A = "Ana Kooperatif";
const COOP_NAME_B = "İkinci Kooperatif";
const COOP_NAME_C = "Askılı Kooperatif";

async function login(page, username, password) {
  // API-level login: the HttpOnly cookie lands on the browser context —
  // same result as the form without depending on hydration timing.
  const response = await page.context().request.post(`${API}/api/auth/login`, {
    headers: { "content-type": "application/json", origin: WEB },
    data: { username, password },
  });
  if (!response.ok()) fail(`login ${username}: ${response.status()}`);
  await page.goto(`${WEB}/`);
  await page.waitForLoadState("networkidle");
}

async function switchCooperative(page, name) {
  await page.getByRole("button", { name: "Kooperatif değiştir" }).click();
  await page.getByRole("menuitem", { name }).click();
  await page.waitForLoadState("load");
}

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

  // --- J1: login + selector on desktop, active identification ---------
  await login(page, "b5admin", "b5-parola-gizli-1");
  await page.getByRole("button", { name: "Kooperatif değiştir" }).click();
  const menu = page.getByRole("menu");
  await menu.getByRole("menuitem", { name: new RegExp(COOP_NAME_A) }).waitFor();
  check("selector lists Ana Kooperatif", true);
  check(
    "selector lists İkinci Kooperatif",
    (await menu
      .getByRole("menuitem", { name: new RegExp(COOP_NAME_B) })
      .count()) === 1,
  );
  const activeItem = menu.getByRole("menuitem", {
    name: new RegExp(COOP_NAME_A),
  });
  check(
    "active cooperative marked",
    (await activeItem.textContent()).includes("Aktif"),
  );
  const suspendedItem = menu.getByRole("menuitem", {
    name: new RegExp(COOP_NAME_C),
  });
  check(
    "suspended cooperative disabled with reason",
    (await suspendedItem.getAttribute("aria-disabled")) === "true" &&
      (await suspendedItem.textContent()).includes("Kooperatif askıda"),
  );
  await page.keyboard.press("Escape");

  // --- J2: keyboard accessibility -------------------------------------
  await page.keyboard.press("Tab");
  const trigger = page.getByRole("button", { name: "Kooperatif değiştir" });
  await trigger.focus();
  check(
    "trigger has aria-label",
    (await trigger.getAttribute("aria-label")) !== null,
  );
  await page.keyboard.press("Enter");
  await page.getByRole("menu").waitFor();
  check("menu opens with Enter", true);
  await page.keyboard.press("Escape");

  // --- J3: rejected switch — disabled item never reaches the API ------
  let coopPosts = 0;
  await page.route("**/api/auth/cooperative", (route) => {
    coopPosts += 1;
    route.continue();
  });
  await page.getByRole("button", { name: "Kooperatif değiştir" }).click();
  await menu
    .getByRole("menuitem", { name: new RegExp(COOP_NAME_C) })
    .click({ force: true });
  await sleep(400);
  check("disabled cooperative posts nothing", coopPosts === 0);
  await page.keyboard.press("Escape");
  // Direct API rejection for a suspended cooperative (server-side proof).
  // Requests run cross-origin against the API (same as apiFetch); the
  // session field is null pre-switch, so the assertion header uses the
  // resolved scope from cooperative-context — matching the mirror the
  // app actually sends.
  const rejectStatus = await evalOnce(page,
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
    { api: API, coopId: coopC },
  );
  check("suspended cooperative switch rejected (403)", rejectStatus === 403);
  await page.unroute("**/api/auth/cooperative");

  // --- J4: membership page + permission-gated navigation --------------
  await page.goto(`${WEB}/uyelikler`);
  await page.getByRole("heading", { name: "Üyelik Yönetimi" }).waitFor();
  check("memberships page loads", true);
  // The list is a fetch behind the heading — wait for the data itself.
  await page.getByRole("cell", { name: "B5 Üyesi" }).waitFor();
  check("member row visible", true);
  check(
    "admin row carries Sistem Yöneticisi badge",
    (await page.getByRole("cell", { name: "Sistem Yöneticisi" }).count()) >= 1,
  );

  // --- J5: authorized lifecycle action (suspend + reactivate) ---------
  const memberRow = page.getByRole("row", { name: /B5 Üyesi/ });
  const confirmDialog = page.getByRole("alertdialog");
  // Membership changes emit a realtime `authz` invalidation that
  // re-renders the list; a dialog opened in that window can lose the
  // open/close race and detach before the click lands. A failed click
  // never reaches the button, so retrying the open+confirm pair is safe
  // — the assertions below still verify the resulting state.
  const confirmRowAction = async (row, name) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      await row.getByRole("button", { name }).click();
      try {
        await confirmDialog.getByRole("button", { name }).click({
          timeout: 8000,
        });
        return;
      } catch (error) {
        if (attempt === 2) throw error;
      }
    }
  };
  await confirmRowAction(memberRow, "Üyeliği Askıya Al");
  await memberRow
    .getByRole("button", { name: "Üyeliği Etkinleştir" })
    .waitFor();
  check(
    "membership suspended badge",
    (await memberRow.textContent()).includes("Askıda"),
  );
  await confirmRowAction(memberRow, "Üyeliği Etkinleştir");
  await memberRow.getByRole("button", { name: "Üyeliği Askıya Al" }).waitFor();
  check(
    "membership reactivated badge",
    (await memberRow.textContent()).includes("Aktif Üye"),
  );

  // --- J6: last-admin protection --------------------------------------
  const adminRow = page.getByRole("row", { name: /B5 Yöneticisi/ });
  await confirmRowAction(adminRow, "Üyeliği Sonlandır");
  const lastAdminBlocked = await page
    .getByRole("alert")
    .first()
    .waitFor()
    .then(() => true)
    .catch(() => false);
  check("last admin cannot be removed", lastAdminBlocked);

  // --- J7: successful cooperative switch (WS connected) ----------------
  const switcherButton = page.getByRole("button", {
    name: "Kooperatif değiştir",
  });
  check(
    "switcher shows Ana Kooperatif",
    (await switcherButton.textContent()).includes(COOP_NAME_A),
  );
  // Wait for the realtime socket to be live before switching.
  await page.waitForFunction(
    () => document.querySelector("header")?.textContent?.includes("Canlı"),
    { timeout: 15000 },
  );
  await switchCooperative(page, COOP_NAME_B);
  await page.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    COOP_NAME_B,
    { timeout: 15000 },
  );
  check("switch to B reflected in header", true);
  // A realtime-driven reload can destroy the evaluate context mid-call;
  // retry through evalResilient, then record a failed check instead of
  // crashing the whole journey if it stays destroyed.
  const meAfter = await evalResilient(
    page,
    async (api) =>
      fetch(`${api}/api/auth/me`, { credentials: "include" }).then((r) =>
        r.json(),
      ),
    API,
  ).catch(() => null);
  check("authoritative scope is B", meAfter?.activeCooperativeId === coopB);
  await page.goto(`${WEB}/uyelikler`);
  await page.getByRole("heading", { name: "Üyelik Yönetimi" }).waitFor();
  // Wait for the scoped list to actually render before counting.
  await page.getByRole("cell", { name: "B5 Yöneticisi" }).waitFor();
  check(
    "scoped list shows only B members",
    (await page.getByRole("cell", { name: "B5 Üyesi" }).count()) === 0 &&
      (await page.getByRole("cell", { name: "B5 Yöneticisi" }).count()) === 1,
  );

  // --- J8: session restoration after refresh ---------------------------
  await page.reload();
  await page.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    COOP_NAME_B,
    { timeout: 15000 },
  );
  check("scope B survives reload", true);

  // --- J9: two tabs, one session — cross-tab sync (WS connected) -------
  const tabB = await ctx.newPage();
  await tabB.goto(`${WEB}/uyelikler`);
  await tabB.getByRole("heading", { name: "Üyelik Yönetimi" }).waitFor();
  await tabB.getByRole("cell", { name: "B5 Yöneticisi" }).waitFor();
  check("tab B renders B scope", true);
  // Switch back to A in tab 1 — tab 2 must mask + resync + reload to A.
  await switchCooperative(page, COOP_NAME_A);
  await tabB.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    COOP_NAME_A,
    { timeout: 20000 },
  );
  check("tab B adopted scope A without manual reload", true);
  await tabB.getByRole("cell", { name: "B5 Üyesi" }).waitFor();
  check("tab B now lists A members", true);

  // --- J10: cross-tab sync while the WebSocket is unavailable ---------
  // Abort socket upgrades on tab B ONLY — HTTP stays reachable so the
  // BroadcastChannel path must carry the invalidation.
  await tabB.route("**/api/realtime**", (route) => route.abort());
  await switchCooperative(page, COOP_NAME_B);
  await tabB.waitForFunction(
    (expected) =>
      document
        .querySelector('header button[aria-label="Kooperatif değiştir"]')
        ?.textContent?.includes(expected),
    COOP_NAME_B,
    { timeout: 20000 },
  );
  check("tab B synced via BroadcastChannel without WS", true);

  // --- J11: stale-data prevention on the masked path -------------------
  // While the invalidation is being processed the page must not keep
  // rendering the previous cooperative's rows; assert the final state
  // shows B data only.
  await tabB.getByRole("cell", { name: "B5 Yöneticisi" }).waitFor();
  check(
    "no stale A rows in tab B after sync",
    (await tabB.getByRole("cell", { name: "B5 Üyesi" }).count()) === 0,
  );

  // --- J12: mobile viewport -------------------------------------------
  const mobile = await browser.newContext({
    locale: "tr-TR",
    viewport: { width: 375, height: 812 },
    hasTouch: true,
  });
  const mobPage = await mobile.newPage();
  await login(mobPage, "b5admin", "b5-parola-gizli-1");
  const mobTrigger = mobPage.getByRole("button", {
    name: "Kooperatif değiştir",
  });
  await mobTrigger.waitFor();
  check("mobile: switcher reachable", await mobTrigger.isVisible());
  await mobTrigger.click();
  check(
    "mobile: menu opens and lists cooperatives",
    (await mobPage
      .getByRole("menuitem", { name: new RegExp(COOP_NAME_B) })
      .count()) === 1,
  );
  await mobPage.keyboard.press("Escape");
  await mobile.close();

  // --- J13: unauthorized access ----------------------------------------
  const ctx2 = await browser.newContext({ locale: "tr-TR" });
  const plain = await ctx2.newPage();
  await login(plain, "b5plain", "b5-parola-gizli-3");
  check(
    "memberships nav hidden without memberships.read",
    (await plain.getByRole("link", { name: "Üyelikler" }).count()) === 0,
  );
  await plain.goto(`${WEB}/uyelikler`);
  await plain.waitForTimeout(1200);
  check(
    "unauthorized membership list shows denial, not data",
    (await plain.getByRole("cell", { name: "B5 Üyesi" }).count()) === 0,
  );
  const deniedCode = await evalResilient(plain, async (api) => {
    const response = await fetch(`${api}/api/memberships`, {
      credentials: "include",
    });
    return response.status;
  }, API);
  check("memberships API denied for plain user (403)", deniedCode === 403);
  await ctx2.close();

  await closeBrowser();
  if (process.exitCode !== 1) log("ALL JOURNEYS PASSED");
} catch (error) {
  fail(String(error?.stack ?? error));
} finally {
  await shutdown();
}
