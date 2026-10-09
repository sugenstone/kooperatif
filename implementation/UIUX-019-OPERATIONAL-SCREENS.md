# UIUX-019 — Premium Operational Screens & shadcn-svelte Deep Transformation

## Scope

Presentation-layer transformation of all operational screens onto the
shadcn-svelte foundation established by UIUX-018. No backend, schema,
contract, authorization, idempotency, or financial-semantics change.
Turkish remains the default product language; all identifiers and API
payloads remain unchanged.

Baseline: `main` at `3d61987` (UIUX-018 committed), clean tree.

## Screen inventory

52 routes total. Audit finding: UIUX-018 modernized the shell,
dashboard and primitives, but operational pages still used
hand-rolled markup: native `<select>` filters, inline empty-state
placeholders, bespoke `pager` markup, per-page spinner blocks,
`confirm()` for destructive actions, and inconsistent
search/pagination controls.

Pages were grouped into three classes:

- **Transformed** — meaningful page-level changes (see matrix below).
- **Already at target** — detail/form pages that already used the
  Card/Badge/sectioned structure from STEP-017/018 and required no
  functional change (e.g. `hissedarlar/[id]`, `hisseler/[id]`,
  `tahakkuklar/[id]`, `roller/[id]`, `kullanicilar/[id]`,
  `gelirler/[id]`, `giderler/[id]`, `transferler/[id]`,
  `aileler/[id]`, `kurullar/[id]`, `kurullar/yeni`, `donemler/yeni`,
  `hissedarlar/yeni`, `hisseler/yeni`, `hisse-iadeleri/yeni`,
  `sosyal-yardim/yeni`).
- **Out of scope / intentional exception** — `login` (auth surface),
  `(tv)/canli-ekran` (read-only TV wall; retains its native select,
  which is a display-only period switcher — converting it would add
  floating-layer overhead to a kiosk surface with no benefit).

## Design principles

- Truthful controls only: every filter/search/page-size control maps
  to a real backend query parameter; no client-side fake sorting or
  filtering of a single page.
- Shared operational vocabulary: one toolbar, one pager, one empty
  state, one error state, one status badge, one money renderer.
- No placeholder financial data; every number shown is canonical API
  data.
- Keyboard/screen-reader access for every new interactive primitive.

## Components introduced

Application components (`apps/web/src/lib/components/`):

| Component             | Purpose                                                                                  |
| --------------------- | ---------------------------------------------------------------------------------------- |
| `list-toolbar.svelte` | Search input + filter slot + reset + "N sonuç" summary, with debounced server-side query |
| `pager.svelte`        | Consistent pagination: prev/next, "Sayfa X / Y", total count, optional page-size Select  |
| `empty-state.svelte`  | (pre-existing, now universally adopted)                                                  |
| `error-state.svelte`  | API error banner with localized key → message + retry                                    |
| `status-badge.svelte` | Centralized status → label/tone/icon mapping (not color-only)                            |
| `money-text.svelte`   | Right-aligned, tabular-nums, TR-formatted amount with optional muted/tone                |
| `step-header.svelte`  | Numbered step header for multi-stage forms (payment entry)                               |

Test utility: `apps/web/src/specs/select-helper.ts` (`pickSelectOption`)
opens a Bits UI Select by accessible name/id, picks an option, and
waits for the layer to close and the body pointer-lock to release —
this prevents jsdom pointer-events races between consecutive Select
interactions.

## Transformation matrix

| Route                                                                                                                                                                                                                                                                                      | Previous issue                           | Improvement                                                                                         | Status               |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------- | --------------------------------------------------------------------------------------------------- | -------------------- |
| `tahsilatlar`                                                                                                                                                                                                                                                                              | Plain table, no toolbar                  | ListToolbar search, StatusBadge, MoneyText, Pager, skeleton/error                                   | COMPLETE             |
| `tahsilatlar/yeni`                                                                                                                                                                                                                                                                         | Native selects, flat form                | StepHeader sections, shadcn Select for shareholder/assessment/account, allocation prefill preserved | COMPLETE             |
| `tahsilatlar/[id]`                                                                                                                                                                                                                                                                         | Native allocation select                 | shadcn Select + Pager on allocations                                                                | COMPLETE             |
| `donemler`                                                                                                                                                                                                                                                                                 | No search, bespoke pager                 | ListToolbar, StatusBadge, Pager                                                                     | COMPLETE             |
| `donemler/[id]`                                                                                                                                                                                                                                                                            | Bespoke sub-pager                        | Pager on assessment list                                                                            | COMPLETE             |
| `hissedarlar`                                                                                                                                                                                                                                                                              | No search                                | ListToolbar, StatusBadge, Pager                                                                     | COMPLETE             |
| `aileler`                                                                                                                                                                                                                                                                                  | Bare create input, bespoke pager         | Toolbar + Pager + Input                                                                             | COMPLETE             |
| `hisseler`                                                                                                                                                                                                                                                                                 | No filter                                | Toolbar/status filter, StatusBadge, Pager                                                           | COMPLETE             |
| `kategoriler`                                                                                                                                                                                                                                                                              | Bare create form, no filter              | Select for kind filter + StatusBadge + Pager                                                        | COMPLETE             |
| `finansal-hesaplar`                                                                                                                                                                                                                                                                        | No type/status filter                    | ListToolbar + type + status Selects, MoneyText balance, Pager                                       | COMPLETE             |
| `finansal-hesaplar/[id]`                                                                                                                                                                                                                                                                   | Bespoke movements pager                  | Pager on movement list                                                                              | COMPLETE             |
| `finansal-hesaplar/yeni`                                                                                                                                                                                                                                                                   | Native selects                           | shadcn Select                                                                                       | COMPLETE             |
| `transferler`                                                                                                                                                                                                                                                                              | No status filter (no search API)         | StatusBadge, MoneyText, Pager; no fake search added (documented gap)                                | COMPLETE             |
| `transferler/yeni`                                                                                                                                                                                                                                                                         | Native selects                           | shadcn Select for source/destination                                                                | COMPLETE             |
| `gelirler`                                                                                                                                                                                                                                                                                 | Native filter selects                    | ListToolbar + shadcn Selects (account/category/status), MoneyText, Pager                            | COMPLETE             |
| `giderler`                                                                                                                                                                                                                                                                                 | Same                                     | Same                                                                                                | COMPLETE             |
| `gelirler/yeni`, `giderler/yeni`                                                                                                                                                                                                                                                           | Native selects                           | shadcn Selects (account/category)                                                                   | COMPLETE             |
| `gelir-gider`                                                                                                                                                                                                                                                                              | Plain summary                            | StatCard summary + tabular money columns                                                            | COMPLETE             |
| `hisse-iadeleri`                                                                                                                                                                                                                                                                           | No toolbar                               | ListToolbar, StatusBadge, MoneyText, Pager                                                          | COMPLETE             |
| `hisse-iadeleri/[id]`                                                                                                                                                                                                                                                                      | Native settle-account select             | shadcn Select                                                                                       | COMPLETE             |
| `yatirimlar`                                                                                                                                                                                                                                                                               | Native filters                           | ListToolbar + status Select, StatusBadge, Pager                                                     | COMPLETE             |
| `yatirimlar/[id]`                                                                                                                                                                                                                                                                          | Native leg/account selects               | shadcn Selects                                                                                      | COMPLETE             |
| `yatirimlar/yeni`                                                                                                                                                                                                                                                                          | Native selects                           | shadcn Selects                                                                                      | COMPLETE             |
| `sosyal-yardim`                                                                                                                                                                                                                                                                            | Native filters                           | ListToolbar + type/status Selects, restricted-fund text preserved                                   | COMPLETE             |
| `sosyal-yardim/[id]`                                                                                                                                                                                                                                                                       | Native account/person selects            | shadcn Selects; restricted-availability wording preserved                                           | COMPLETE             |
| `yonetim`                                                                                                                                                                                                                                                                                  | Native filters                           | ListToolbar + status Select, StatusBadge, Pager                                                     | COMPLETE             |
| `yonetim/kurullar`                                                                                                                                                                                                                                                                         | Plain list                               | Toolbar, StatusBadge, Pager                                                                         | COMPLETE             |
| `yonetim/kararlar/[id]`                                                                                                                                                                                                                                                                    | Native vote selects                      | shadcn Selects                                                                                      | COMPLETE             |
| `yonetim/kararlar/yeni`                                                                                                                                                                                                                                                                    | Native board select                      | shadcn Select                                                                                       | COMPLETE             |
| `raporlar`                                                                                                                                                                                                                                                                                 | Native report-type select, plain summary | shadcn Select + StatCard summary                                                                    | COMPLETE             |
| `report-panel`, `report-trend`                                                                                                                                                                                                                                                             | Native selects                           | shadcn Selects                                                                                      | COMPLETE             |
| `kullanicilar`                                                                                                                                                                                                                                                                             | `confirm()` lifecycle flow               | Accessible AlertDialog for disable/enable; CSRF + lockout handling preserved; StatusBadge           | COMPLETE             |
| `oturumlar`                                                                                                                                                                                                                                                                                | No header/badges                         | PageHeader, StatusBadge, skeleton                                                                   | COMPLETE             |
| `roller`                                                                                                                                                                                                                                                                                   | Plain list                               | PageHeader, StatusBadge, skeleton                                                                   | COMPLETE             |
| `hissedarlar/[id]`, `hisseler/[id]`, `aileler/[id]`, `tahakkuklar/[id]`, `gelirler/[id]`, `giderler/[id]`, `transferler/[id]`, `kurullar/[id]+yeni`, `roller/[id]`, `kullanicilar/[id]`, `donemler/yeni`, `hissedarlar/yeni`, `hisseler/yeni`, `hisse-iadeleri/yeni`, `sosyal-yardim/yeni` | —                                        | Already met target: Card sections, Badges, dl summaries                                             | COMPLETE (unchanged) |
| `(tv)/canli-ekran`                                                                                                                                                                                                                                                                         | Display surface                          | Intentional exception (see inventory)                                                               | COMPLETE (unchanged) |

## Payment UX

`tahsilatlar/yeni` retains its existing flow (shareholder → period →
assessment → amount/account/date/reference) and gains:

- `StepHeader` numbered sections replacing flat field soup.
- shadcn Selects for shareholder, assessment and financial account,
  each `aria-labelledby` associated with a visible Label and a stable
  `id` for tests/assistive tech.
- Assessment re-selection still re-prefills the editable amount from
  the selected assessment's canonical remaining amount — verified by
  `payments-pages.spec.ts` (300,00 → 1234,56).
- Idempotency key generated once per logical submission; submit
  disabled while pending (no double-submit, no new key on retry).
- No invented allocation prediction: review text shows only canonical
  fields (amount, payer/debtor, account).

## Financial UX

- Financial account list now exposes the backend's `accountType` and
  `status` filter params (previously unused), canonical balance via
  MoneyText.
- Transfer form uses shadcn Selects for source/destination; the
  existing explanatory copy that a transfer does not create income is
  unchanged.
- Social Aid keeps its restricted-availability wording and the
  restricted-fund explanatory text verbatim; hard-reservation rules
  untouched.

## Governance and reporting

- `yonetim` decision list: search + status filter (server params),
  StatusBadge for decision state, Pager.
- Reports: report-type and trend-month Selects converted; summary
  cards use StatCard. No new endpoints, no fabricated charts.

## Accessibility

- Every Select trigger is `aria-labelledby` to a visible Label and has
  a stable `id` (replacing `for`-target semantics which do not apply
  to button triggers).
- AlertDialog replaces `confirm()` — focus trap, Escape, labelled
  action buttons.
- Empty states keep `role="status"`; skeletons `aria-hidden`.
- vitest-setup stubs added for jsdom: `matchMedia`,
  `scrollIntoView`, `hasPointerCapture`/`setPointerCapture`/
  `releasePointerCapture`.

## Responsive verification

`scripts/e2e-step017-uiux.mjs` extended from 4 → 6 viewports
(360×800, 390×844, 768×1024, 1024×768, 1440×900, 1920×1080) and from
9 → 18 routes (added gelirler, giderler, sosyal-yardim, yatirimlar,
transferler, hisse-iadeleri, yonetim, gelir-gider, canli-ekran).
Assertions: no horizontal overflow, mobile nav opens/closes, money
inputs `inputmode=decimal`, landmark sanity. Result: PASS.

## Screenshots

- `artifacts/step017/before/` — 36 images (9 routes × 4 viewports)
  captured from HEAD (`3d61987`, UIUX-018 state).
- `artifacts/step017/after/` — 108 images (18 routes × 6 viewports)
  captured from the working tree.
- `artifacts/step017/` top level — after set (108) for convenience.

## Tests

- Prettier: `All matched files use Prettier code style!`
- `svelte-check`: 0 errors, 0 warnings
- Vitest: **24 files / 168 tests passed**
- `pnpm build` (adapter-node): PASS
- RC E2E (`scripts/e2e-step017-rc.mjs`): **PASS** — journeys A–K +
  UI drill + a11y + Firefox/WebKit
- UIUX E2E (`scripts/e2e-step017-uiux.mjs`): **PASS** — 6 viewports

Spec updates were mechanical adaptations (native `selectOptions` →
`pickSelectOption`, `confirm()` → AlertDialog assertions); no
behavioral assertion was weakened — allocation-prefill, CSRF token,
`lockout_prevented`, restricted-fund text and permission-gating
assertions are unchanged.

## Financial and security invariants

Zero changes to: storage, schema, API contracts, Rust Decimal /
NUMERIC(19,2), payment allocation, credit offset, idempotency keys,
reversal, account-movement semantics, share ownership, restricted
Social Aid hard reservation, investment accounting, auth/RBAC, audit,
WebSocket contracts. Money payloads remain canonical decimal strings;
TR-locale entry is display-layer only.

## Known limitations

- `transferler` has no server-side search param → no search control
  shown (truthful-controls principle); gap documented here.
- Detail pages that already met target quality were intentionally not
  rewritten.
- TV display retains one native select (kiosk surface exception).
- No backend changes were made; no deployment, no release tag.

## Changed files

48 modified + 7 added, all under `apps/web/` and `scripts/`:

- New: `error-state`, `list-toolbar`, `money-text`, `pager`,
  `status-badge`, `step-header` components; `select-helper.ts` spec
  utility.
- i18n: `tr-TR.ts`, `en.ts` (+4 keys: pagination/toolbar labels).
- Pages/components: 33 route pages + `report-panel` + `report-trend`.
- Specs: 8 spec files + `vitest-setup.ts` jsdom stubs.
- `scripts/e2e-step017-uiux.mjs`: route/viewport expansion.
