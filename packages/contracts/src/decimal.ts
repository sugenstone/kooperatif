/**
 * Exact decimal value transported as a canonical decimal string
 * (e.g. `"125.4375"`), per ADR-004 (Money and Value Representation).
 *
 * Rules for this contract primitive:
 * - Authoritative financial/quantity values MUST use `DecimalString` on
 *   the wire. PostgreSQL `NUMERIC` serializes to this form; Rust keeps
 *   exact decimal arithmetic server-side.
 * - Consumers MUST NOT coerce these values to `number`. Formatting for
 *   display uses locale-aware decimal parsing/formatting utilities.
 * - The canonical form is a plain decimal literal without exponent
 *   notation and without grouping separators. Scale is defined per value
 *   type by the backend.
 */
export type DecimalString = string & { readonly __brand: 'DecimalString' };

const DECIMAL_STRING_PATTERN = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$/;

/** Validates the canonical decimal-string form (no exponent, no grouping). */
export function isDecimalString(value: string): value is DecimalString {
	return DECIMAL_STRING_PATTERN.test(value);
}

/**
 * Constructor for values received from the backend. Throws on
 * non-canonical input so contract violations fail loudly instead of
 * silently propagating lossy or malformed values.
 */
export function asDecimalString(value: string): DecimalString {
	if (!isDecimalString(value)) {
		throw new Error(`Value is not a canonical decimal string: ${value}`);
	}
	return value;
}
