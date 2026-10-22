// proc-group.test.mjs — focused tests for the E2E process-group
// cleanup helper (M1-P8-A pre-commit fix). Uses disposable `node`/`sh`
// children only; never touches project services.
//
//   node --test scripts/lib/proc-group.test.mjs
//
// Group-semantics tests are POSIX-only — on Windows the taskkill /T
// path is covered by the single-process kill test. Every test cleans
// up through finally so a failed assertion can never orphan a child.
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import net from "node:net";
import { readdirSync, readFileSync } from "node:fs";
import { spawnService, killService } from "./proc-group.mjs";

const IS_WIN = process.platform === "win32";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const alive = (pid) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
};

// A zombie answers kill(,0) but is dead: under a non-reaping init or
// before libuv delivers the exit event, a terminated process may still
// appear alive — treat zombies as gone.
const isZombie = (pid) => {
	try {
		const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
		return stat.slice(stat.lastIndexOf(")") + 2).startsWith("Z");
	} catch {
		return false;
	}
};

const gone = (pid) => !alive(pid) || isZombie(pid);

const groupAlive = (pid) => {
	try {
		process.kill(-pid, 0);
	} catch {
		return false;
	}
	try {
		for (const entry of readdirSync("/proc")) {
			const member = Number(entry);
			if (!member) continue;
			try {
				const stat = readFileSync(`/proc/${member}/stat`, "utf8");
				const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
				if (Number(fields[2]) === pid && fields[0] !== "Z") return true;
			} catch {}
		}
		return false;
	} catch {
		return true;
	}
};

/// Wait for the child's exit event (reaping) — bounded so a stuck
/// handle can never hang the suite.
const waitExit = (child, ms = 5000) =>
	new Promise((r) => {
		if (child.exitCode !== null || child.signalCode) return r();
		const t = setTimeout(r, ms);
		child.once("exit", () => {
			clearTimeout(t);
			r();
		});
	});

/// Collect stdout until `prefix:<value>` appears; returns the value.
function readTag(child, prefix, timeoutMs = 5000) {
	return new Promise((resolve, reject) => {
		let buf = "";
		const timer = setTimeout(() => reject(new Error("tag timeout")), timeoutMs);
		child.stdout.on("data", (d) => {
			buf += d;
			const m = buf.match(new RegExp(`${prefix}:(\\d+)`));
			if (m) {
				clearTimeout(timer);
				resolve(Number(m[1]));
			}
		});
	});
}

const canConnect = (port) =>
	new Promise((resolve) => {
		const s = net.connect(port, "127.0.0.1");
		s.once("connect", () => {
			s.destroy();
			resolve(true);
		});
		s.once("error", () => resolve(false));
	});

async function untilConnectable(port, tries = 60) {
	for (let i = 0; i < tries; i++) {
		if (await canConnect(port)) return true;
		await sleep(100);
	}
	return false;
}

// POSIX: a grandchild must die with the group — the original defect.
test("descendants are terminated with the process group", { skip: IS_WIN }, async () => {
	const code =
		'const{spawn}=require("node:child_process");const c=spawn(process.execPath,["-e","setInterval(()=>{},1e3)"],{stdio:"ignore"});console.log("GC:"+c.pid);setInterval(()=>{},1e3)';
	const child = spawnService(`node -e ${JSON.stringify(code)}`, {
		stdio: ["ignore", "pipe", "inherit"]
	});
	try {
		const gcPid = await readTag(child, "GC");
		await sleep(300); // grandchild fully spawned
		assert.ok(alive(child.pid), "child alive before kill");
		assert.ok(alive(gcPid), "grandchild alive before kill");
		await killService(child);
		await waitExit(child);
		assert.ok(gone(child.pid), "child gone");
		assert.ok(gone(gcPid), "grandchild gone");
		assert.equal(groupAlive(child.pid), false, "group empty");
	} finally {
		await killService(child);
	}
});

// POSIX: the real defect — a descendant holding a port must release it.
test("port held by a descendant is released", { skip: IS_WIN }, async () => {
	const port = 45620 + (process.pid % 500);
	const code = `const{spawn}=require("node:child_process");spawn(process.execPath,["-e","require('net').createServer().listen(${port},'127.0.0.1')"],{stdio:"ignore"});setInterval(()=>{},1e3)`;
	const child = spawnService(`node -e ${JSON.stringify(code)}`, {
		stdio: ["ignore", "pipe", "inherit"]
	});
	try {
		assert.ok(await untilConnectable(port), "grandchild port listening");
		await killService(child);
		await waitExit(child);
		await sleep(200);
		assert.equal(await canConnect(port), false, "port refused after kill");
	} finally {
		await killService(child);
	}
});

test("already-exited child is a safe no-op", async () => {
	const child = spawnService(
		IS_WIN ? "cmd /c exit 0" : "true",
		{ stdio: "ignore" }
	);
	try {
		await waitExit(child);
		await killService(child); // must not throw or hang
		assert.ok(true);
	} finally {
		await killService(child);
	}
});

test("repeated cleanup is safe", async () => {
	const child = spawnService(
		IS_WIN
			? "cmd /c ping -n 300 127.0.0.1"
			: "sleep 300",
		{ stdio: "ignore" }
	);
	try {
		await killService(child);
		await killService(child); // second call must be a no-op
		if (!IS_WIN) assert.equal(groupAlive(child.pid), false);
	} finally {
		await killService(child);
	}
});

test("unrelated processes in other groups are untouched", { skip: IS_WIN }, async () => {
	const victim = spawnService("sleep 300", { stdio: "ignore" });
	const bystander = spawnService("sleep 300", { stdio: "ignore" });
	try {
		await killService(victim);
		await waitExit(victim); // reap before asserting — exit is async
		assert.ok(gone(victim.pid), "victim gone");
		assert.ok(alive(bystander.pid), "bystander survives — own group untouched");
	} finally {
		await killService(victim);
		await killService(bystander); // runs even when the assert fails
	}
});

test("unmarked children are refused — no signal sent", { skip: IS_WIN }, async () => {
	// A child NOT spawned through spawnService carries no OWNED_GROUP
	// marker; killService must refuse rather than guess at its pgid.
	const child = spawn("sleep 300", { shell: true, stdio: "ignore" });
	try {
		await killService(child);
		assert.ok(alive(child.pid), "unmarked child left running");
	} finally {
		child.kill("SIGKILL");
	}
});

// Windows parity: the taskkill path still terminates the owned child.
test("windows taskkill path terminates the child", { skip: !IS_WIN }, async () => {
	const child = spawnService("cmd /c ping -n 300 127.0.0.1", {
		stdio: "ignore"
	});
	try {
		await sleep(300);
		await killService(child);
		await waitExit(child);
		assert.ok(gone(child.pid), "child gone via taskkill /T");
	} finally {
		await killService(child);
	}
});
