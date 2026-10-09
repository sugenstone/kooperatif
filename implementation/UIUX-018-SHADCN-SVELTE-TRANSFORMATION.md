# UIUX-018 — Premium shadcn-svelte Application Transformation

## Scope

Presentation-layer modernization of the SvelteKit frontend onto the
official shadcn-svelte component foundation. No backend, schema,
contract, authorization, idempotency, or financial-semantics change.

Baseline: `main` at `efd0425` (UI-TR-001 committed). Clean tree.

## Initial UI audit

- App shell was a hand-rolled top/bottom nav (`app-nav.svelte`) with a
  custom mobile menu; no sidebar component set, no theme support, no
  toast system, no breadcrumb.
- `PageHeader` and `EmptyState` existed since STEP-017 but `EmptyState`
  was unused by list pages (inline `<p>` placeholders); `PageHeader`
  was applied to list screens only.
- Dashboard showed a flat 4-card grid with no grouping or descriptions.
- TV display was functional and grouped (UI-TR-001) but overflowed on
  narrow viewports and used fixed sizes only.
- Missing primitives: sidebar, sheet, breadcrumb, dropdown-menu,
  alert-dialog, dialog, select, separator, skeleton, sonner, tabs,
  tooltip, scroll-area.

## Design direction

Restrained financial-SaaS: semantic tokens only (no ad-hoc colors),
existing light/dark palette extended with `--success`, `--warning`,
`--info` tokens in both themes; Lucide iconography; lucide + mode-watcher

- svelte-sonner as the only new runtime dependencies.

## Components introduced

Installed via the official shadcn-svelte registry (CLI):
`alert-dialog`, `breadcrumb`, `dialog`, `dropdown-menu`, `scroll-area`,
`select`, `separator`, `sheet`, `sidebar`, `skeleton`, `sonner`,
`tabs`, `tooltip`, plus the `is-mobile` hook.

Application wrappers (`$lib/components/`):

- `app-sidebar.svelte` — grouped permission-filtered nav, Lucide icons,
  active-route marking (`aria-current="page"` + `isActive`), brand
  header, user account menu + logout in the footer, `Sidebar.Rail`.
- `app-header.svelte` — sticky header: sidebar trigger (Turkish
  `aria-label="Menü"`, `aria-expanded`), breadcrumb derived from the
  route map, realtime status chip, light/dark toggle, user dropdown.
- `page-header.svelte` — extended (backward-compatible) to also accept
  localized `title`/`description` strings in addition to i18n keys.
- `stat-card.svelte` — KPI card (label, value, hint, icon, tone).
- `empty-state.svelte` — extended to accept `message`/`icon`/`action`
  while keeping the STEP-017 `messageKey`/`hintKey` API.
- `list-skeleton.svelte` — skeleton rows for first-load states.
- `specs/app-shell-harness.svelte` — test harness rendering the real
  `Sidebar.Provider` + `AppSidebar` + trigger for isolated specs.

## Application shell

`(app)/+layout.svelte` now composes `Sidebar.Provider → AppSidebar →
Sidebar.Inset → AppHeader → <main id="main-content">`. Root layout adds
`ModeWatcher` (light/dark, persisted) and a `Toaster` (richColors,
bottom-right).

Navigation groups (permission-filtered; backend remains authoritative):
Genel / Üyeler ve Hisseler / Aidat ve Tahsilat / Finans / Sosyal Yardım /
Yönetim — matching the mandated grouping with the application's real
routes; no invented pages, no global search (search is documented as a
missing capability for later approval — no endpoint exists).

Generated `sidebar-inset.svelte` was patched to render `<div>` instead
of `<main>` (nested `<main>` landmarks are invalid) and gained
`min-w-0` so wide tables scroll inside the inset instead of overflowing
the page.

## Dashboard

Rewritten to the UI-TR-001 metric groups — Aidat Durumu / Kasa ve
Faaliyetler / Sosyal Yardımlar — using `StatCard` with the same help
texts as the TV surface (`tv.help.*`), plus a secondary "Üyeler ve
Bekleyen İşler" row and a permission-gated "Hızlı İşlemler" card.
All values come from the canonical `/api/reports/overview`; zero
browser-side financial math.

## Data tables / list pages

All 17 primary list screens keep `PageHeader` (title + description +
actions). All inline empty-state paragraphs on list pages were
retrofitted to the shared `EmptyState` (icon + message, `role=status`).
Table containers already scroll horizontally (`table-container`);
the 768px overflow was caused by the missing `min-w-0` in the inset —
fixed at the layout level.

## Live TV

Same 3 groups and help texts preserved; added responsive breakpoints
(1-col → 3-col at `md`), distance-friendly scaled typography,
`break-words` on large amounts, and a wrapping header so the clock,
status chip, mode selector and fullscreen button never overlap.
Read-only, WebSocket re-sync, stale banner and denial screen unchanged.

## Accessibility results

- Single `<main>` landmark (inset fix).
- Sidebar trigger exposes `aria-label="Menü"` + `aria-expanded`
  (verified open/close in E2E).
- Mobile nav is a real dialog (focus management by bits-ui) that closes
  on Escape and on navigation.
- Active route carries `aria-current="page"`.
- Empty states use `role="status"`; skeletons are `aria-hidden`.
- Help/explanations are visible text — no tooltip-only information.
- E2E a11y block (labels, skip link, tab order, aria-expanded,
  aria-current) passes.

## Missing capability (documented, not fabricated)

- Global search / command palette: no backend search endpoint exists —
  deferred, requires approval for a new API surface.
- Charts on the dashboard: the overview endpoint exposes totals, not a
  time series — deferred rather than fabricating a series.
- Detail-page inline empty states (inside section tables) still use the
  legacy paragraph style; list-level states are unified.

## Regression results

| Gate                                                             | Result                |
| ---------------------------------------------------------------- | --------------------- |
| Prettier                                                         | clean                 |
| ESLint                                                           | clean                 |
| svelte-check                                                     | 0 errors / 0 warnings |
| Vitest                                                           | 168/168               |
| `pnpm build`                                                     | PASS                  |
| RC E2E A–K + UI drill + a11y + Firefox/WebKit                    | PASS                  |
| UIUX E2E (360/390/768/1440px, overflow, mobile sheet, inputmode) | PASS                  |

Screenshots: `artifacts/step017/{360x800,390x844,768x1024,1440x900}-*.png`.

## Financial invariants

Untouched: NUMERIC(19,2) storage, canonical money strings, tr-TR comma
input, payment/allocation/reversal/credit semantics, restricted social
aid reservation, share returns, investments, idempotency, RBAC,
WebSocket contracts, audit trails. All payload assertions in the RC
suite pass unchanged.

## Changed files

- `apps/web/package.json`, `pnpm-lock.yaml` — 3 runtime deps.
- `apps/web/src/lib/utils.ts` — `WithoutChild(ren)` type helpers.
- `apps/web/src/routes/layout.css` — success/warning/info tokens.
- `apps/web/src/routes/+layout.svelte` — ModeWatcher + Toaster.
- `apps/web/src/routes/(app)/+layout.svelte` — sidebar shell.
- `apps/web/src/routes/(app)/+page.svelte` — dashboard redesign.
- `apps/web/src/routes/(tv)/canli-ekran/+page.svelte` — responsive polish.
- 17 list pages — `EmptyState` retrofit (kullanicilar also gained PageHeader).
- `apps/web/src/lib/components/{app-sidebar,app-header,stat-card,list-skeleton}.svelte` — new.
- `apps/web/src/lib/components/{page-header,empty-state}.svelte` — API extensions.
- `apps/web/src/lib/components/ui/*` — generated shadcn components.
- `apps/web/src/lib/components/app-nav.svelte` — deleted (superseded).
- `apps/web/vitest-setup.ts` — jsdom `matchMedia` stub.
- `apps/web/src/specs/{nav-shell,authz-helper,reports-pages,app-shell-harness}` — shell assertions.
- `apps/web/src/lib/i18n/{tr-TR,en}.ts` — nav group + home keys.
- `scripts/e2e-step017-{rc,uiux}.mjs` — updated to the new shell's
  a11y contract (dialog on mobile, `[data-slot="sidebar"]`, trigger
  `aria-expanded`).
