<script lang="ts">
	/**
	 * Test harness (M1-P7-A): mounts a dirty-form registration with a
	 * test-controlled predicate so specs can drive `hasDirtyForms()`,
	 * unmount cleanup and `clear()` without a full page component.
	 */
	import { useDirtyForm, type DirtyFormHandle } from '$lib/forms/dirty-forms.svelte';

	let {
		isDirty,
		onReady
	}: { isDirty: () => boolean; onReady?: (handle: DirtyFormHandle) => void } = $props();

	const handle = useDirtyForm(() => isDirty());
	$effect(() => {
		onReady?.(handle);
	});
</script>
