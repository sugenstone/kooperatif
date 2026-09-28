import type { ShareholderListItem } from '@kooperatif/contracts';

/**
 * Canonical shareholder display identity (STEP-004 §20/§21, hardened by
 * STEP-005 §5/§6/§75).
 *
 * Guardian/Vasi is part of every shareholder's operational identity —
 * NOT merely a duplicate-name disambiguator. It is rendered for ALL
 * shareholders; an absent guardian shows the explicit "Belirtilmemiş"
 * segment rather than being silently omitted. The backend provides
 * `displayLabel`; this helper exists for locally-composed items so
 * formatting never drifts between components.
 */
export function shareholderLabel(item: {
	firstName: string;
	lastName: string;
	guardianFirstName?: string | null;
	guardianLastName?: string | null;
	familySequence?: number | null;
}): string {
	let label = `${item.firstName} ${item.lastName}`;
	label +=
		item.guardianFirstName && item.guardianLastName
			? ` · Vasi: ${item.guardianFirstName} ${item.guardianLastName}`
			: ' · Vasi: Belirtilmemiş';
	if (item.familySequence != null) {
		label += ` · Aile No ${item.familySequence}`;
	}
	return label;
}

/** Re-export for convenience so components import from one place. */
export type { ShareholderListItem };
