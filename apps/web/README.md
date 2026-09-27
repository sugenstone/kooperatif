# @kooperatif/web

SvelteKit + TypeScript + shadcn-svelte frontend of the Kooperatif
application (STEP-001 infrastructure shell).

See the repository [README](../../README.md) for tooling, quality gates
and the local development environment, and
[docs/12-UI-UX-SYSTEM.md](../../docs/12-UI-UX-SYSTEM.md) for the UI/UX
rules this app must follow.

## Commands

```bash
pnpm dev      # dev server (http://localhost:5173)
pnpm check    # svelte-check
pnpm lint     # prettier --check + eslint
pnpm test     # vitest (jsdom, unit + component)
pnpm build    # production build (adapter-node)
```
