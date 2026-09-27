/**
 * Backend API base URL. `PUBLIC_API_BASE_URL` is a build-time public env
 * variable (see `.env.example`); production prefers same-origin routing
 * per ADR-012, where an explicit value must be provided.
 */
export const API_BASE_URL: string = import.meta.env.PUBLIC_API_BASE_URL ?? 'http://localhost:8080';
