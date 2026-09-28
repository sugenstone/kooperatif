/**
 * Backend API base URL. `PUBLIC_API_BASE_URL` is a build-time public env
 * variable (see `.env.example`); `envPrefix: 'PUBLIC_'` in vite.config
 * exposes it through `import.meta.env` and lets an explicitly exported
 * process env override the committed `.env` value (Vite precedence).
 * Production prefers same-origin routing per ADR-012, where an explicit
 * value must be provided.
 */
export const API_BASE_URL: string = import.meta.env.PUBLIC_API_BASE_URL ?? 'http://localhost:8080';
