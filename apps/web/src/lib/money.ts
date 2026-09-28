/**
 * Exact TRY helpers for agreed business values (ADR-004: never f64).
 * Amounts cross the API as decimal strings; all parsing/formatting here
 * is string-level so large values never lose precision.
 */

/**
 * Parse a tr-TR operator input ("50.000,00" / "50000" / "50000,25")
 * into a canonical decimal string ("50000.00"). Returns `null` when the
 * input is non-empty but malformed; callers map `null` to a validation
 * message and MUST NOT treat it as zero (NULL vs 0, STEP-005 §17).
 */
export function parseTryInput(raw: string): string | null {
	const trimmed = raw.trim();
	if (!trimmed) return null;
	// tr-TR: '.' is the thousands separator, ',' the decimal separator.
	const normalized = trimmed.replace(/\./g, '').replace(',', '.');
	if (!/^\d+(\.\d{1,2})?$/.test(normalized)) return null;
	const [integer, fraction = ''] = normalized.split('.');
	return `${integer}.${fraction.padEnd(2, '0')}`;
}

/**
 * Format a canonical decimal string for display as tr-TR TRY
 * ("50000.00" → "50.000,00 ₺"). String-level grouping — no float.
 */
export function formatTry(amount: string | null | undefined): string {
	if (amount == null || amount === '') return '';
	const [integer, fraction = '00'] = amount.split('.');
	const grouped = integer.replace(/\B(?=(\d{3})+(?!\d))/g, '.');
	return `${grouped},${fraction.padEnd(2, '0').slice(0, 2)} ₺`;
}
