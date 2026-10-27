import { toast } from 'svelte-sonner';
import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

/**
 * Shared mutation feedback — a single sonner call site so every page
 * uses the same accessible live-region toasts (the global <Toaster/>
 * in the root layout survives client-side navigation).
 *
 * Call only after a backend-confirmed result; never log payloads.
 */
export function notifySuccess(key: MessageKey): void {
	toast.success(t(key));
}

export function notifyError(key: MessageKey): void {
	toast.error(t(key));
}
