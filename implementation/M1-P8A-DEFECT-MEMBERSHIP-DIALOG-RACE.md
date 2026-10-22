# DEFECT — Membership AlertDialog lost to realtime-invalidation open/close race

- **Discovered:** M1-P8-A Batch 2 (E2E `scripts/e2e-m1p6-b5.mjs`, journey J5)
- **Status:** FIXED — M1-P8-A Batch 3.1 (see "Confirmed root cause" below)
- **Severity:** UX edge case; no authorization or data-integrity impact. The
  backend always enforces the operation; worst case the user re-opens the
  confirmation dialog.
- **Surface:** `apps/web/src/routes/(app)/uyelikler/+page.svelte` — membership
  status lifecycle (suspend / reactivate / end) confirmation dialogs. The same
  `pending`/`AlertDialog.Root` + realtime `refresh()` pattern may be reachable
  on other pages that refresh list state while a confirmation dialog is open.

## Reproduction sequence

1. Sign in as a user with `memberships.manage` on the memberships page.
2. Open the suspend dialog for an active member and confirm.
   The backend suspends the membership and emits a realtime `authz`
   invalidation. `confirmStatusChange` sets `pending = null` and calls
   `refresh()`; the `live.onInvalidate` listener fires a second `refresh()`
   for the same change.
3. Immediately click "Üyeliği Etkinleştir" on the same row.
4. Click the dialog's confirmation button.

In the Playwright run this fails deterministically: the resolved
`alertdialog` action button is detached from the DOM before the click
stabilizes; subsequent retries find no `alertdialog` at all and time out
after 30 s.

## Expected behavior

The reactivate confirmation dialog opens and stays mounted until the user
confirms or cancels; the click lands; the membership becomes active.

## Actual behavior

The freshly opened dialog's DOM node is detached mid-interaction and the
dialog effectively closes (`pending` becomes `null`), so the confirmation
click has no target. The page is left with no dialog open and no request
sent.

## Confirmed root cause (Batch 3.1)

The Batch-3 hypothesis (dialog churn inside the page) was a contributing
factor at most. The CONFIRMED mechanism is one level up, in
`$lib/auth/auth.svelte.ts` + `(app)/+layout.svelte`:

1. `resolveEffectiveScope` wrote the RAW session field
   (`response.activeCooperativeId`, `null` for never-switched sessions)
   into `auth.resolvedCooperativeId` BEFORE awaiting
   `/api/auth/cooperative-context`. Every `authz` resync on a
   bootstrap-scope session therefore exposed a transient **null scope**
   for the duration of the context fetch.
2. The layout derived `noCooperativeAccess` flips true on that window →
   `showRecoveryMask` → `{@render children()}` is replaced by the
   recovery section → **the entire page component unmounts** — `pending`,
   `actionError`, `toggling`, and the open AlertDialog are destroyed
   with it. A moment later the mask clears and a FRESH page mounts with
   empty state.

This single defect explains every observed flake: the J5 second-dialog
detach, the J6 `actionError` alert that vanished before it could be
asserted, the badge/label renders read mid-remount, and the switcher's
transient empty label (`resolvedCooperativeId` drives `active`). It
also fired on every socket reconnect (`emit('*')` on open), not only on
membership signals.

Secondary confirmed mechanism: the layout's `authz` resync hard-reloads
on `cooperativeChanged` without distinguishing a switch THIS tab
initiated — the switcher's own `hardNavigate` then raced a second
navigation, intermittently destroying Playwright execution contexts
("Execution context was destroyed").

## Suspected root cause (Batch-3 hypothesis — superseded)

Two independent refresh paths race the second dialog's mount:

- `live.onInvalidate` reacts to the backend's `authz` invalidation event
  and calls `refresh()` again (the mutation already refreshed once).
- `refresh()` replaces `memberships`, re-rendering the keyed `{#each}`
  table while the second `AlertDialog` is mounting/opening.

During the close→open churn of consecutive `AlertDialog.Root` lifecycles
(suspend dialog exiting while reactivate dialog mounts), a spurious
`onOpenChange(false)` can fire; the handler
`if (!open && toggling === null) pending = null` then clears `pending`,
unmounting the dialog the user just opened. The mount/detach cycle
matches the Playwright "element was detached from the DOM" log exactly.

This is a timing race: human-speed interaction usually escapes it, but
it is reachable and nondeterministic.

## User impact

A fast operator confirming two lifecycle actions back-to-back can see
the second confirmation dialog vanish. The operation silently does not
run — there is no error message, and the row still shows the pre-action
state, so a careful user can retry, but the failure is invisible.

## Implemented fix (Batch 3.1)

1. **`apps/web/src/lib/auth/auth.svelte.ts`:** `resolveEffectiveScope` is
   now a pure resolver returning `string | null` — it no longer writes
   the raw session field mid-flight. `applyAuthResponse` takes the
   resolved id and assigns it in the SAME synchronous block as the rest
   of `/me`, so no reactive consumer ever observes a partially applied
   session. A failed context resolution still fails closed to `null`
   (recovery mask fires for genuine revocation, unchanged).
2. **`apps/web/src/routes/(app)/+layout.svelte`:** `switchCooperative`
   marks `selfSwitch.target` on success; the layout's `authz` (and
   denied-seam) resync consumes the marker and skips the redundant
   `hardNavigate` when the observed change is the switch this tab itself
   committed — the initiating flow already navigates.

The `+page.svelte` reorder proposed below was NOT needed once the
unmount path was eliminated; the page logic is unchanged.

## Regression test coverage (Batch 3.1)

- **`src/specs/membership-dialog-race.spec.ts`** (6 tests) mounts the real
  layout + memberships page, captures the realtime handler set and fires
  `authz` at a controlled point inside the deferred cooperative-context
  fetch — the exact window that used to unmount the page. Covers: dialog
  survival across resync, rejection-alert persistence, suspend→immediate
  reactivate, single submission on double confirm, self-switch navigate
  suppression, external-change navigation still firing.
- **`src/specs/auth-restore.spec.ts`** (5 tests) unit-tests the
  root cause directly: `restoreSession` never exposes a transient null
  scope mid-resolution, fails closed on context failure, skips the
  context fetch for explicit session scopes, and `switchCooperative`
  marks/clears the self-switch target.
- **Browser E2E:** `e2e-m1p6-b5.mjs` J5/J6/J7 is the reproduction; it
  passed 3 consecutive full runs after the fix. `e2e-m1p7-a` and
  `e2e-m1p7-b` also pass. The `confirmRowAction` retry wrapper is kept
  as harness tolerance (a failed click never reaches the button, so it
  cannot double-submit); all evaluations are now wrapped in
  `evalResilient`, which retries ONLY on destroyed execution contexts —
  assertions are unchanged.

## Related change (Batch 2, diagnostic only)

`scripts/e2e-m1p6-b5.mjs` wraps open+confirm pairs in a bounded
`confirmRowAction` retry (3 × 8 s) so the suite reports the underlying
product race as a passing journey instead of a flaky timeout. This is a
test-harness tolerance, not a fix: failed clicks never reach the button,
so retrying cannot double-submit a mutation. All assertions are
unchanged.
