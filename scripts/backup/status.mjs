// backup:status — report backup health without opening shell history.
// Prints NEVER_RUN | HEALTHY | STALE | FAILED and exits non-zero unless
// HEALTHY, so an external scheduler/monitor can alert on it (docs/23,
// ADR-013 observability: silent backup failure is not acceptable).
//
//   pnpm backup:status
import { evaluateStatus, loadConfig, listArtifacts } from './lib.mjs';

const cfg = loadConfig();
const { state, status, ageHours } = evaluateStatus(cfg);
const artifacts = listArtifacts(cfg);

console.log(`[backup:status] state=${state}`);
console.log(`[backup:status] dir=${cfg.backupDir}`);
console.log(`[backup:status] artifacts=${artifacts.length}`);
if (status.lastAttempt) {
	console.log(`[backup:status] last attempt: ${status.lastAttempt.at} → ${status.lastAttempt.result}`);
}
if (status.lastSuccess) {
	console.log(
		`[backup:status] last success: ${status.lastSuccess.at} artifact=${status.lastSuccess.artifact}` +
			(ageHours !== undefined ? ` age=${ageHours.toFixed(1)}h (max ${cfg.maxAgeHours}h)` : '')
	);
}
if (status.lastVerify) {
	console.log(`[backup:status] last verify: ${status.lastVerify.at} → ${status.lastVerify.result}`);
}
if (status.lastDrill) {
	console.log(`[backup:status] last drill: ${status.lastDrill.at} → ${status.lastDrill.result}`);
}
if (status.lastRetention) {
	console.log(`[backup:status] last retention: ${status.lastRetention.at} mode=${status.lastRetention.mode}`);
}

process.exitCode = state === 'HEALTHY' ? 0 : 2;
