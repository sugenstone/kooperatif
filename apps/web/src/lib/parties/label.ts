import type { ShareholderListItem } from '@kooperatif/contracts';

/**
 * Canonical shareholder display identity (STEP-004 §20/§21).
 *
 * Names may repeat: every surface that shows a shareholder MUST show
 * the disambiguating context (guardian + family sequence). The backend
 * provides `displayLabel`; this helper exists for locally-composed
 * items so formatting never drifts between components.
 */
export function shareholderLabel(item: {
	firstName: string;
	lastName: string;
	guardianFirstName?: string | null;
	guardianLastName?: string | null;
	familySequence?: number | null;
}): string {
	let label = `${item.firstName} ${item.lastName}`;
	if (item.guardianFirstName && item.guardianLastName) {
		label += ` · Vasi: ${item.guardianFirstName} ${item.guardianLastName}`;
	}
	if (item.familySequence != null) {
		label += ` · Aile No ${item.familySequence}`;
	}
	return label;
}

/** Re-export for convenience so components import from one place. */
export type { ShareholderListItem };
