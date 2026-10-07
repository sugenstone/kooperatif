/**
 * Social Aid, donation & restricted fund contracts (STEP-013, docs/11).
 *
 * Three concepts stay separate:
 *   FUND          — purpose/restriction identity (`social_aid_funds`);
 *                   creating a Fund moves ZERO money and it is NEVER
 *                   a Financial Account ("Financial Account != Fund",
 *                   docs/01).
 *   DONATION      — restricted money received (`social_aid_donations`):
 *                   exactly ONE inflow Account Movement
 *                   (`sourceType: 'social_aid_donation'`) per posted row.
 *   DISBURSEMENT  — restricted money paid to a beneficiary
 *                   (`social_aid_disbursements`): exactly ONE outflow
 *                   Account Movement (`'social_aid_disbursement'`).
 *
 * Boundaries:
 *   - Restricted availability is DERIVED per (fund, account):
 *     sum(posted donations) - sum(posted disbursements) — never a
 *     stored/editable number.
 *   - A disbursement is gated on BOTH the fund's restricted
 *     availability in the chosen account AND the physical account
 *     balance — restricted money cannot be spent merely because
 *     unrelated cooperative cash or another account's restricted
 *     balance exists.
 *   - Donations/disbursements create NO Payment, Allocation,
 *     Shareholder Credit, operational Income/Expense, Transfer,
 *     Share Return or Investment rows — Social Aid is a separate
 *     financial context (docs/11).
 *   - Donor identity: `donorPersonId` links the canonical Person
 *     (shareholder, guardian or any registered person — membership
 *     never required); `donorDisplayName` covers external persons and
 *     organizations. At least one is required — anonymous donation
 *     rules are an open decision (docs/11) and not implemented.
 *   - Beneficiary identity mirrors the donor rule; beneficiary data is
 *     sensitive (docs/11 §privacy) and only the minimum fields exist.
 *   - The Aid Request/Decision approval pipeline is DEFERRED to the
 *     governance step — a disbursement carries a required `reason`
 *     instead. `reason` is why the aid was granted.
 *   - Reversals preserve history: `status` flips posted -> reversed,
 *     the bound movement flips active -> reversed. Donation reversal
 *     is refused when it would overdraw the physical account OR the
 *     fund's restricted availability (consumed restricted money can
 *     never be invalidated by a reversal).
 *   - Fund lifecycle: active -> closed | cancelled. Close is refused
 *     while any restricted availability remains; cancel is refused
 *     once any financial event exists. No reopen in STEP-013.
 *
 * Money (ADR-004): amounts are decimal STRINGS. `startsOn`/`endsOn`
 * are business DATEs (`YYYY-MM-DD`); `occurredAt`/timestamps are
 * RFC3339.
 */

export type SocialAidFundStatus = 'active' | 'closed' | 'cancelled';

export type SocialAidPostedStatus = 'posted' | 'reversed';

export interface SocialAidFundListItem {
	id: string;
	fundNumber: number;
	name: string;
	status: SocialAidFundStatus;
	/** Derived — sum of posted donations across every account. */
	totalDonated: string;
	/** Derived — sum of posted disbursements across every account. */
	totalDisbursed: string;
	/** Derived restricted availability — never a stored number. */
	available: string;
	createdAt: string;
}

export interface SocialAidFundAccount {
	financialAccountId: string;
	accountName: string;
	donated: string;
	disbursed: string;
	/** Restricted availability of this fund IN this account. */
	available: string;
	/** Physical balance of the account (all money, every context). */
	physicalBalance: string;
}

export interface SocialAidDonation {
	id: string;
	donationNumber: number;
	fundId: string;
	donorPersonId: string | null;
	/** Resolved person name when `donorPersonId` is linked. */
	donorName: string | null;
	/** Free-text identity for external persons/organizations. */
	donorDisplayName: string | null;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	occurredAt: string;
	reference: string | null;
	note: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
	status: SocialAidPostedStatus;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface SocialAidDisbursement {
	id: string;
	disbursementNumber: number;
	fundId: string;
	beneficiaryPersonId: string | null;
	beneficiaryName: string | null;
	beneficiaryDisplayName: string | null;
	financialAccountId: string;
	accountName: string;
	amount: string;
	currency: string;
	occurredAt: string;
	/** Why the aid was granted — required. */
	reason: string;
	reference: string | null;
	accountMovementId: string;
	movementStatus: 'active' | 'reversed';
	status: SocialAidPostedStatus;
	reversedAt: string | null;
	reversalReason: string | null;
}

export interface SocialAidFundDetail {
	id: string;
	fundNumber: number;
	name: string;
	description: string | null;
	startsOn: string | null;
	endsOn: string | null;
	status: SocialAidFundStatus;
	closedAt: string | null;
	cancelledAt: string | null;
	cancellationReason: string | null;
	totalDonated: string;
	totalDisbursed: string;
	available: string;
	accounts: SocialAidFundAccount[];
	donations: SocialAidDonation[];
	disbursements: SocialAidDisbursement[];
	createdAt: string;
	updatedAt: string;
}

export const SOCIAL_AID_FUNDS_PATH = '/api/social-aid/funds';
export const socialAidFundPath = (id: string): string => `/api/social-aid/funds/${id}`;
export const socialAidFundClosePath = (id: string): string => `/api/social-aid/funds/${id}/close`;
export const socialAidFundCancelPath = (id: string): string =>
	`/api/social-aid/funds/${id}/cancel`;
export const SOCIAL_AID_DONATIONS_PATH = '/api/social-aid/donations';
export const socialAidDonationPath = (id: string): string => `/api/social-aid/donations/${id}`;
export const socialAidDonationReversePath = (id: string): string =>
	`/api/social-aid/donations/${id}/reverse`;
export const SOCIAL_AID_DISBURSEMENTS_PATH = '/api/social-aid/disbursements';
export const socialAidDisbursementPath = (id: string): string =>
	`/api/social-aid/disbursements/${id}`;
export const socialAidDisbursementReversePath = (id: string): string =>
	`/api/social-aid/disbursements/${id}/reverse`;

/**
 * Person lookup for donor/beneficiary selection — scoped to
 * `social_aid.manage` so the aid operator can resolve the canonical
 * Person without holding full parties read rights.
 */
export const SOCIAL_AID_PERSONS_PATH = '/api/social-aid/persons';

export interface CreateSocialAidFundRequest {
	name: string;
	description?: string | null;
	startsOn?: string | null;
	endsOn?: string | null;
	idempotencyKey: string;
}

export interface CancelSocialAidFundRequest {
	reason: string;
}

export interface PostSocialAidDonationRequest {
	fundId: string;
	donorPersonId?: string | null;
	donorDisplayName?: string | null;
	financialAccountId: string;
	/** Exact decimal string (ADR-004). */
	amount: string;
	/** RFC-3339; may be backdated, never future. */
	occurredAt: string;
	reference?: string | null;
	note?: string | null;
	idempotencyKey: string;
}

export interface PostSocialAidDisbursementRequest {
	fundId: string;
	beneficiaryPersonId?: string | null;
	beneficiaryDisplayName?: string | null;
	financialAccountId: string;
	amount: string;
	occurredAt: string;
	/** Why the aid was granted — required. */
	reason: string;
	reference?: string | null;
	idempotencyKey: string;
}

export interface ReverseSocialAidEventRequest {
	reason: string;
}
