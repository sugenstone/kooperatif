<script lang="ts">
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';

	/**
	 * REQ-027 / M0-D1: the single application-level confirmation pattern
	 * for destructive and financial actions. Mount conditionally
	 * (`{#if pending}`); `onDismiss` clears the pending state on any
	 * non-confirm close (Cancel, Escape, overlay). While `busy`, the
	 * dialog cannot be dismissed and both buttons are disabled, so a
	 * confirmed action cannot be submitted twice.
	 */
	let {
		title,
		description,
		confirmKey = 'common.confirm',
		busy = false,
		onConfirm,
		onDismiss
	}: {
		title: MessageKey;
		description: MessageKey;
		confirmKey?: MessageKey;
		busy?: boolean;
		onConfirm: () => void;
		onDismiss: () => void;
	} = $props();
</script>

<AlertDialog.Root
	open={true}
	onOpenChange={(next) => {
		if (!next && !busy) onDismiss();
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{t(title)}</AlertDialog.Title>
			<AlertDialog.Description>{t(description)}</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={busy}>{t('common.cancel')}</AlertDialog.Cancel>
			<AlertDialog.Action disabled={busy} onclick={onConfirm}>
				{t(confirmKey)}
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
