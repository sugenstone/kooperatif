// proc-group.mjs — owned process-group cleanup for E2E scripts
// (M1-P8-A pre-commit fix). Each long-lived child (backend via
// `cargo run`, frontend via `pnpm dev`) is spawned in a dedicated
// POSIX process group whose pgid equals the child's pid, so shutdown
// can signal the whole tree — descendants like the server binary or
// vite would otherwise survive a SIGTERM to the wrapper and keep the
// suite ports bound for the next run.
//
// Safety contract:
//   * killService only ever signals a process group it created itself
//     via spawnService — the group marker is set at spawn time and a
//     call against an unmarked child is refused;
//   * SIGTERM first, bounded wait, SIGKILL only if the group still has
//     members after the grace period;
//   * already-exited and already-cleaned children are no-ops;
//   * Windows keeps the existing taskkill /T semantics (taskkill
//     already walks the tree; `detached` is meaningless there).
import { spawn, spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";

const OWNED_GROUP = Symbol("e2e.ownedProcessGroup");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/// Spawn a shell command as the leader of its own process group on
/// POSIX. `detached` is ignored by Node on Windows, where taskkill /T
/// already terminates the tree.
export function spawnService(command, opts = {}) {
	// `detached` only on POSIX — on Windows it would allocate a new
	// console and change local behavior; taskkill /T needs no group.
	const child = spawn(command, {
		...opts,
		shell: true,
		detached: process.platform !== "win32"
	});
	if (process.platform !== "win32") {
		child[OWNED_GROUP] = true;
	}
	return child;
}

/// Is `pid` a zombie? A zombie still answers kill(,0) and keeps the
/// pgid entry, but holds no resources (ports are freed) and cannot
/// act — waiting on it or escalating to SIGKILL is pointless.
function isZombie(pid) {
	try {
		const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
		return stat.slice(stat.lastIndexOf(")") + 2).startsWith("Z");
	} catch {
		return false; // pid gone, or no /proc (non-Linux POSIX)
	}
}

function groupAlive(pgid) {
	try {
		process.kill(-pgid, 0);
	} catch {
		return false;
	}
	// On Linux, filter zombies out of the liveness check: a group whose
	// only survivors are zombies is effectively dead. Without this the
	// shutdown path would sit out the full grace period and emit a
	// misleading SIGKILL warning whenever descendants are reaped late
	// (e.g. orphaned under a PID 1 that does not reap).
	try {
		for (const entry of readdirSync("/proc")) {
			const pid = Number(entry);
			if (!pid) continue;
			try {
				const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
				const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
				if (Number(fields[2]) === pgid && fields[0] !== "Z") return true;
			} catch {}
		}
		return false;
	} catch {
		return true; // no /proc — fall back to the signal probe result
	}
}

/// Terminate the process group owned by `child`. Never throws — cleanup
/// paths must not mask the suite result.
export async function killService(
	child,
	{ termTimeoutMs = 8000, killTimeoutMs = 2000, pollMs = 100 } = {}
) {
	if (!child?.pid) return;
	const pgid = child.pid;
	if (process.platform === "win32") {
		spawnSync("taskkill", ["/pid", String(pgid), "/T", "/F"], { stdio: "pipe" });
		return;
	}
	if (child[OWNED_GROUP] !== true) {
		console.warn(
			`[proc-group] refusing to signal pid ${pgid}: not a spawnService-owned process group`
		);
		return;
	}
	if (!groupAlive(pgid)) return;
	try {
		process.kill(-pgid, "SIGTERM");
	} catch {
		return;
	}
	let deadline = Date.now() + termTimeoutMs;
	while (Date.now() < deadline && groupAlive(pgid)) {
		await sleep(pollMs);
	}
	if (!groupAlive(pgid)) return;
	try {
		process.kill(-pgid, "SIGKILL");
	} catch {
		return;
	}
	deadline = Date.now() + killTimeoutMs;
	while (Date.now() < deadline && groupAlive(pgid)) {
		await sleep(pollMs);
	}
	if (groupAlive(pgid)) {
		console.warn(
			`[proc-group] process group ${pgid} survived SIGKILL — leaking descendants`
		);
	}
}
