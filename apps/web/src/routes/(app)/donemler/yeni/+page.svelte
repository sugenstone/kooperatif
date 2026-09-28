<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Alert } from '$lib/components/ui/alert';
	import { Button } from '$lib/components/ui/button';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch } from '$lib/api-client';
	import { auth } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import { parseTryInput } from '$lib/money';
	import { PERIODS_PATH, type AssessmentRuleType, type PeriodDetail } from '@kooperatif/contracts';

	let name = $state('');
	let collectionStartDate = $state('');
	let dueDate = $state('');
	let ruleType = $state<AssessmentRuleType>('per_shareholder');
	let baseAmount = $state('');
	let effectiveDate = $state('');
	let submitError = $state<MessageKey | null>(null);
	let submitting = $state(false);

	const isoDate = /^\d{4}-\d{2}-\d{2}$/;

	async function handleSubmit(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (submitting) return;
		if (
			!isoDate.test(collectionStartDate) ||
			!isoDate.test(dueDate) ||
			!isoDate.test(effectiveDate)
		) {
			submitError = 'periods.invalidDate';
			return;
		}
		const amount = parseTryInput(baseAmount);
		if (amount === null) {
			submitError = 'periods.invalidAmount';
			return;
		}
		submitting = true;
		submitError = null;
		try {
			const created = await apiFetch<PeriodDetail>(PERIODS_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: {
					name: name.trim(),
					collectionStartDate,
					dueDate,
					ruleType,
					baseAmount: amount,
					assessmentEffectiveDate: effectiveDate
				}
			});
			goto(resolve(`/donemler/${created.id}`));
		} catch (error) {
			submitError = apiErrorKey(error);
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{t('periods.create')} — {t('app.title')}</title>
</svelte:head>

<section class="mx-auto flex max-w-2xl flex-col gap-6">
	<div class="flex items-center gap-3">
		<Button variant="ghost" size="sm" href="/donemler">← {t('periods.title')}</Button>
	</div>
	<h1 class="text-2xl font-semibold tracking-tight">{t('periods.create')}</h1>

	{#if submitError}
		<Alert variant="destructive">{t(submitError)}</Alert>
	{/if}

	<form class="flex flex-col gap-6" onsubmit={(event) => void handleSubmit(event)}>
		<Card>
			<CardHeader>
				<CardTitle>{t('periods.create')}</CardTitle>
				<CardDescription>{t('periods.description')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex flex-col gap-2">
					<Label for="period-name">{t('periods.name')}</Label>
					<Input id="period-name" bind:value={name} />
				</div>
				<div class="grid grid-cols-2 gap-4">
					<div class="flex flex-col gap-2">
						<Label for="collection-start">{t('periods.collectionStart')}</Label>
						<Input id="collection-start" type="date" bind:value={collectionStartDate} />
					</div>
					<div class="flex flex-col gap-2">
						<Label for="due-date">{t('periods.dueDate')}</Label>
						<Input id="due-date" type="date" bind:value={dueDate} />
					</div>
				</div>
			</CardContent>
		</Card>

		<Card>
			<CardHeader>
				<CardTitle>{t('periods.rule')}</CardTitle>
				<CardDescription>{t('periods.ruleHelp')}</CardDescription>
			</CardHeader>
			<CardContent class="flex flex-col gap-4">
				<div class="flex gap-2">
					<Button
						type="button"
						size="sm"
						variant={ruleType === 'per_shareholder' ? 'default' : 'outline'}
						onclick={() => (ruleType = 'per_shareholder')}
					>
						{t('periods.rulePerShareholder')}
					</Button>
					<Button
						type="button"
						size="sm"
						variant={ruleType === 'per_share' ? 'default' : 'outline'}
						onclick={() => (ruleType = 'per_share')}
					>
						{t('periods.rulePerShare')}
					</Button>
				</div>
				<div class="flex max-w-[240px] flex-col gap-2">
					<Label for="base-amount">{t('periods.baseAmount')}</Label>
					<Input
						id="base-amount"
						inputmode="decimal"
						placeholder="1.000,00"
						bind:value={baseAmount}
					/>
				</div>
				<div class="flex max-w-[240px] flex-col gap-2">
					<Label for="effective-date">{t('periods.effectiveDate')}</Label>
					<Input id="effective-date" type="date" bind:value={effectiveDate} />
					<p class="text-xs text-muted-foreground">{t('periods.effectiveDateHelp')}</p>
				</div>
			</CardContent>
		</Card>

		<div>
			<Button type="submit" disabled={submitting || !name.trim()}>
				{t('periods.submitCreate')}
			</Button>
		</div>
	</form>
</section>
