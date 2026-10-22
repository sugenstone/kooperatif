/**
 * Dirty-form registry (M1-P7-A, matrix T7.2).
 *
 * Cooperative-scoped form pages register an `isDirty` predicate here;
 * the app layout and the cooperative switcher consult the registry
 * before any navigation/switch that would discard unsaved input.
 *
 * Contract:
 * - The registry stores PREDICATES, not form values — no field data is
 *   copied, mirrored, or persisted (unsaved input must never cross a
 *   cooperative boundary).
 * - Registration is bound to component lifecycle: `$effect`-driven
 *   cleanup removes the predicate when the page unmounts, so the
 *   registry cannot leak a stale dirty flag.
 * - `bypassDirtyGuard()` marks the NEXT guard check as authorized —
 *   used for confirmed switches and server-driven adoptions (a
 *   cross-tab switch is already committed server-side and cannot be
 *   vetoed locally).
 */
const guards = new Map<symbol, () => boolean>();
let bypassNext = false;

/** True while at least one mounted form reports unsaved changes. */
export function hasDirtyForms(): boolean {
	for (const isDirty of guards.values()) {
		if (isDirty()) return true;
	}
	return false;
}

/** Authorize the next dirty-guarded navigation (one-shot). */
export function bypassDirtyGuard(): void {
	bypassNext = true;
}

/** Check + consume the one-shot bypass. */
export function consumeDirtyBypass(): boolean {
	const value = bypassNext;
	bypassNext = false;
	return value;
}

export interface DirtyFormHandle {
	/**
	 * Permanently clear the guard — call AFTER a successful save, before
	 * navigating away, so the post-save navigation is not re-guarded.
	 */
	clear(): void;
}

/**
 * Register a form's dirty predicate for the lifetime of the calling
 * component. Must be called during component initialization (uses
 * `$effect`). `isDirty` should report "meaningful unsaved changes"
 * only — a pristine or just-saved form returns false.
 */
export function useDirtyForm(isDirty: () => boolean): DirtyFormHandle {
	const key = Symbol('dirty-form');
	$effect(() => {
		guards.set(key, isDirty);
		return () => guards.delete(key);
	});
	return {
		clear() {
			guards.delete(key);
		}
	};
}
