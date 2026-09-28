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
 * Canonical scale-2 API value → tr-TR input-string ("300.00" → "300,00").
 * Prefilling an operator input with the raw canonical value would be
 * misparsed (tr-TR '.' is the thousands separator).
 */
export function canonicalToTryInput(amount: string): string {
	return amount.replace('.', ',');
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

/** Canonical scale-2 decimal string → exact kuruş (BigInt). */
function toKurus(amount: string): bigint {
	const negative = amount.startsWith('-');
	const [integer, fraction = '00'] = amount.replace('-', '').split('.');
	const value = BigInt(integer) * 100n + BigInt(fraction.padEnd(2, '0').slice(0, 2));
	return negative ? -value : value;
}

/**
 * Exact decimal comparison for canonical scale-2 strings (ADR-004:
 * preview math is advisory but must still be exact — never float).
 * Returns < 0, 0 or > 0.
 */
export function compareDecimals(a: string, b: string): number {
	const diff = toKurus(a) - toKurus(b);
	return diff < 0n ? -1 : diff > 0n ? 1 : 0;
}

/** Exact a - b for canonical scale-2 decimal strings. */
export function subtractDecimals(a: string, b: string): string {
	const diff = toKurus(a) - toKurus(b);
	const negative = diff < 0n;
	const abs = negative ? -diff : diff;
	const integer = (abs / 100n).toString();
	const fraction = (abs % 100n).toString().padStart(2, '0');
	return `${negative ? '-' : ''}${integer}.${fraction}`;
}

/** Exact a + b for canonical scale-2 decimal strings. */
export function addDecimals(a: string, b: string): string {
	const sum = toKurus(a) + toKurus(b);
	const negative = sum < 0n;
	const abs = negative ? -sum : sum;
	const integer = (abs / 100n).toString();
	const fraction = (abs % 100n).toString().padStart(2, '0');
	return `${negative ? '-' : ''}${integer}.${fraction}`;
}
