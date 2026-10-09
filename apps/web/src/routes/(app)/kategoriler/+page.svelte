<script lang="ts">
	import PageHeader from '$lib/components/page-header.svelte';
	import { Badge } from '$lib/components/ui/badge';
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
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { apiErrorKey } from '$lib/api-errors';
	import { apiFetch, ApiError } from '$lib/api-client';
	import { auth, can } from '$lib/auth/auth.svelte';
	import { t, type MessageKey } from '$lib/i18n/i18n.svelte';
	import {
		FINANCIAL_CATEGORIES_PATH,
		financialCategoryPath,
		financialCategoryStatusPath,
		type CreateFinancialCategoryRequest,
		type FinancialCategory,
		type FinancialCategoryList,
		type FinancialCategoryType,
		type UpdateFinancialCategoryRequest
	} from '@kooperatif/contracts';

	const PAGE_SIZE = 50;

	let page = $state(1);
	let data = $state<FinancialCategoryList | null>(null);
	let loadError = $state<MessageKey | null>(null);
	let actionError = $state<MessageKey | null>(null);

	let search = $state('');
	let categoryType = $state<'' | FinancialCategoryType>('');
	let status = $state<'' | 'active' | 'inactive'>('');

	// --- Create -------------------------------------------------------------
	let newType = $state<FinancialCategoryType>('expense');
	let newName = $state('');
	let newDescription = $state('');
	let creating = $state(false);

	// --- Rename --------------------------------------------------------------
	let editingId = $state<string | null>(null);
	let editName = $state('');
	let editDescription = $state('');
	let editExpectedUpdatedAt = $state('');
	let acting = $state(false);

	async function refresh(): Promise<void> {
		loadError = null;
		try {
			const params = new SvelteURLSearchParams({
				page: String(page),
				pageSize: String(PAGE_SIZE)
			});
			if (search.trim()) params.set('search', search.trim());
			if (categoryType) params.set('categoryType', categoryType);
			if (status) params.set('status', status);
			data = await apiFetch<FinancialCategoryList>(`${FINANCIAL_CATEGORIES_PATH}?${params}`);
		} catch (error) {
			loadError = apiErrorKey(error);
		}
	}

	function categoryError(error: unknown): MessageKey {
		return error instanceof ApiError && error.code === 'conflict'
			? 'categories.errors.duplicate'
			: apiErrorKey(error);
	}

	async function createCategory(): Promise<void> {
		if (creating || !newName.trim()) return;
		creating = true;
		actionError = null;
		try {
			const request: CreateFinancialCategoryRequest = {
				categoryType: newType,
				name: newName.trim()
			};
			if (newDescription.trim()) request.description = newDescription.trim();
			await apiFetch<FinancialCategory>(FINANCIAL_CATEGORIES_PATH, {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: request
			});
			newName = '';
			newDescription = '';
			await refresh();
		} catch (error) {
			actionError = categoryError(error);
		} finally {
			creating = false;
		}
	}

	function startEdit(category: FinancialCategory): void {
		editingId = category.id;
		editName = category.name;
		editDescription = category.description ?? '';
		editExpectedUpdatedAt = category.updatedAt;
		actionError = null;
	}

	async function saveEdit(): Promise<void> {
		if (acting || !editingId || !editName.trim()) return;
		acting = true;
		actionError = null;
		try {
			const request: UpdateFinancialCategoryRequest = {
				name: editName.trim(),
				expectedUpdatedAt: editExpectedUpdatedAt
			};
			if (editDescription.trim()) request.description = editDescription.trim();
			await apiFetch<FinancialCategory>(financialCategoryPath(editingId), {
				method: 'PATCH',
				csrfToken: auth.csrfToken,
				body: request
			});
			editingId = null;
			await refresh();
		} catch (error) {
			actionError = categoryError(error);
		} finally {
			acting = false;
		}
	}

	async function setStatus(
		category: FinancialCategory,
		target: 'active' | 'inactive'
	): Promise<void> {
		if (acting) return;
		acting = true;
		actionError = null;
		try {
			await apiFetch<FinancialCategory>(financialCategoryStatusPath(category.id), {
				method: 'POST',
				csrfToken: auth.csrfToken,
				body: { status: target }
			});
			await refresh();
		} catch (error) {
			actionError = apiErrorKey(error);
		} finally {
			acting = false;
		}
	}

	const pages = $derived(data ? Math.max(1, Math.ceil(data.totalCount / data.pageSize)) : 1);

	$effect(() => {
		void refresh();
	});
</script>

<svelte:head>
	<title>{t('categories.title')} — {t('app.title')}</title>
</svelte:head>

<section class="flex flex-col gap-6">
	<PageHeader titleKey="categories.title" descriptionKey="categories.description">
		{#snippet actions()}
			<Button variant="outline" href="/gelir-gider">{t('incomeExpense.summary.title')}</Button>
		{/snippet}
	</PageHeader>

	<div class="flex flex-wrap items-end gap-3">
		<div class="flex flex-col gap-1">
			<Label for="cat-search">{t('common.search')}</Label>
			<Input
				id="cat-search"
				class="w-56"
				bind:value={search}
				onchange={() => {
					page = 1;
					void refresh();
				}}
			/>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="cat-type">{t('categories.type')}</Label>
			<select
				id="cat-type"
				class="w-36 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={categoryType}
				onchange={() => {
					page = 1;
					void refresh();
				}}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="income">{t('categories.typeIncome')}</option>
				<option value="expense">{t('categories.typeExpense')}</option>
			</select>
		</div>
		<div class="flex flex-col gap-1">
			<Label for="cat-status">{t('categories.status')}</Label>
			<select
				id="cat-status"
				class="w-36 rounded-md border bg-background px-3 py-2 text-sm"
				bind:value={status}
				onchange={() => {
					page = 1;
					void refresh();
				}}
			>
				<option value="">{t('incomeExpense.all')}</option>
				<option value="active">{t('categories.active')}</option>
				<option value="inactive">{t('categories.inactive')}</option>
			</select>
		</div>
	</div>

	{#if can('income_expense.manage')}
		<Card class="w-full max-w-3xl">
			<CardHeader>
				<CardTitle>{t('categories.create')}</CardTitle>
			</CardHeader>
			<CardContent class="flex flex-wrap items-end gap-3">
				<div class="flex flex-col gap-1">
					<Label for="new-type">{t('categories.type')}</Label>
					<select
						id="new-type"
						class="w-32 rounded-md border bg-background px-3 py-2 text-sm"
						bind:value={newType}
					>
						<option value="income">{t('categories.typeIncome')}</option>
						<option value="expense">{t('categories.typeExpense')}</option>
					</select>
				</div>
				<div class="flex flex-col gap-1">
					<Label for="new-name">{t('categories.name')}</Label>
					<Input
						id="new-name"
						class="w-56"
						bind:value={newName}
						placeholder={t('categories.namePlaceholder')}
					/>
				</div>
				<div class="flex flex-col gap-1">
					<Label for="new-desc">{t('incomeExpense.description')}</Label>
					<Input id="new-desc" class="w-56" bind:value={newDescription} />
				</div>
				<Button disabled={creating || !newName.trim()} onclick={() => void createCategory()}>
					{t('categories.create')}
				</Button>
			</CardContent>
		</Card>
	{/if}

	{#if loadError}
		<p class="text-sm text-destructive">{t(loadError)}</p>
	{:else if data === null}
		<p class="text-sm text-muted-foreground">{t('roles.loading')}</p>
	{:else if data.items.length === 0}
		<p class="text-sm text-muted-foreground">{t('categories.empty')}</p>
	{:else}
		<Card class="w-full">
			<CardHeader>
				<CardTitle>{t('categories.title')}</CardTitle>
				<CardDescription>{data.totalCount}</CardDescription>
			</CardHeader>
			<CardContent>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>{t('categories.name')}</TableHead>
							<TableHead>{t('categories.type')}</TableHead>
							<TableHead>{t('incomeExpense.description')}</TableHead>
							<TableHead>{t('categories.entryCount')}</TableHead>
							<TableHead>{t('categories.status')}</TableHead>
							{#if can('income_expense.manage')}
								<TableHead></TableHead>
							{/if}
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each data.items as category (category.id)}
							<TableRow>
								<TableCell>
									{#if editingId === category.id}
										<Input class="w-48" bind:value={editName} />
									{:else}
										{category.name}
									{/if}
								</TableCell>
								<TableCell>
									{category.categoryType === 'income'
										? t('categories.typeIncome')
										: t('categories.typeExpense')}
								</TableCell>
								<TableCell>
									{#if editingId === category.id}
										<Input class="w-48" bind:value={editDescription} />
									{:else}
										{category.description ?? '—'}
									{/if}
								</TableCell>
								<TableCell>{category.entryCount}</TableCell>
								<TableCell>
									{#if category.status === 'active'}
										<Badge>{t('categories.active')}</Badge>
									{:else}
										<Badge variant="outline">{t('categories.inactive')}</Badge>
									{/if}
								</TableCell>
								{#if can('income_expense.manage')}
									<TableCell class="flex gap-2">
										{#if editingId === category.id}
											<Button
												variant="outline"
												size="sm"
												disabled={acting || !editName.trim()}
												onclick={() => void saveEdit()}
											>
												{t('accounts.save')}
											</Button>
											<Button variant="ghost" size="sm" onclick={() => (editingId = null)}>
												{t('common.cancel')}
											</Button>
										{:else}
											<Button variant="ghost" size="sm" onclick={() => startEdit(category)}>
												{t('categories.rename')}
											</Button>
											{#if category.status === 'active'}
												<Button
													variant="outline"
													size="sm"
													disabled={acting}
													onclick={() => void setStatus(category, 'inactive')}
												>
													{t('categories.deactivate')}
												</Button>
											{:else}
												<Button
													variant="outline"
													size="sm"
													disabled={acting}
													onclick={() => void setStatus(category, 'active')}
												>
													{t('categories.activate')}
												</Button>
											{/if}
										{/if}
									</TableCell>
								{/if}
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</CardContent>
		</Card>

		<div class="flex items-center justify-between">
			<Button
				variant="outline"
				size="sm"
				disabled={page <= 1}
				onclick={() => {
					page -= 1;
					void refresh();
				}}
			>
				{t('pagination.previous')}
			</Button>
			<span class="text-sm text-muted-foreground">
				{t('pagination.pageInfo')
					.replace('{page}', String(page))
					.replace('{pages}', String(pages))
					.replace('{total}', String(data.totalCount))}
			</span>
			<Button
				variant="outline"
				size="sm"
				disabled={page >= pages}
				onclick={() => {
					page += 1;
					void refresh();
				}}
			>
				{t('pagination.next')}
			</Button>
		</div>
	{/if}

	{#if actionError}
		<p class="text-sm text-destructive">{t(actionError)}</p>
	{/if}
</section>
