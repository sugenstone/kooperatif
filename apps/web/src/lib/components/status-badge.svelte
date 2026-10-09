<script lang="ts">
	/**
	 * Status badge (UIUX-019): Badge + a colored dot so state is never
	 * conveyed by hue alone — the label text is always present.
	 * Tone maps to the semantic tokens added in UIUX-018.
	 */
	import { Badge } from '$lib/components/ui/badge';
	import { cn } from '$lib/utils.js';

	let {
		label,
		tone = 'neutral'
	}: {
		label: string;
		tone?: 'neutral' | 'success' | 'warning' | 'danger' | 'info';
	} = $props();

	const dots: Record<string, string> = {
		neutral: 'bg-muted-foreground',
		success: 'bg-success',
		warning: 'bg-warning',
		danger: 'bg-destructive',
		info: 'bg-info'
	};
	const variants: Record<string, 'default' | 'secondary' | 'outline' | 'destructive'> = {
		neutral: 'secondary',
		success: 'secondary',
		warning: 'secondary',
		danger: 'destructive',
		info: 'secondary'
	};
</script>

<Badge variant={variants[tone]} class="gap-1.5">
	<span class={cn('size-1.5 rounded-full', dots[tone])} aria-hidden="true"></span>
	{label}
</Badge>
