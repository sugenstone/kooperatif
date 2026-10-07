/**
 * Governance, decisions & voting contracts (STEP-014, docs/09).
 *
 * Four concepts stay separate:
 *   BODY        — organ identity (`governance_bodies`); `bodyType` is a
 *                 bounded free-text label because docs/09 leaves the
 *                 legal organ catalog configurable — no fixed enum is
 *                 invented.
 *   MEMBERSHIP  — temporal Person -> Body link
 *                 (`governance_memberships`); `endedAt === null` means
 *                 active. At most ONE active membership per
 *                 (body, person). `title` is a free-text seat label
 *                 with NO attached powers.
 *   DECISION    — formal evidence (`governance_decisions`).
 *                 `decisionOn` (Decision Date) is distinct from
 *                 `effectiveOn` (Effective Date, docs/15) — the latter
 *                 never auto-flips status. Material content freezes
 *                 the moment status leaves 'draft'.
 *   VOTE        — one recorded row per (decision, person)
 *                 (`governance_votes`); immutable once cast.
 *
 * Boundaries:
 *   - Governance is EVIDENCE, not money: no request here creates an
 *     Account Movement or touches any financial aggregate.
 *   - RBAC != GOVERNANCE (docs/18): `governance.manage` lets an
 *     operator RECORD facts; it never makes them a member or voter.
 *     Vote eligibility is server-side: the person must hold an ACTIVE
 *     membership in the decision's body at cast time.
 *   - OUTCOME IS OPERATOR-RECORDED (Model B): quorum, majority, tie
 *     rules, weighted/proxy/secret voting are open decisions in
 *     docs/09 and are NOT implemented. Finalization stores the
 *     formally decided outcome plus a frozen tally snapshot
 *     (eligible/approve/reject/abstain counts).
 *   - The recorded-vote model stores the voting Person AND the login
 *     actor (`recordedBy`) — they are never conflated.
 *   - No DELETE exists anywhere: drafts may be cancelled; opened and
 *     finalized decisions are permanent evidence.
 *
 * Business dates (`decisionOn`, `effectiveOn`) cross the API as
 * `YYYY-MM-DD`; timestamps are RFC3339.
 */

export type GovernanceBodyStatus = 'active' | 'closed';

export type GovernanceDecisionStatus =
	| 'draft'
	| 'open'
	| 'approved'
	| 'rejected'
	| 'cancelled';

export type GovernanceDecisionOutcome = 'approved' | 'rejected';

export type GovernanceVoteChoice = 'approve' | 'reject' | 'abstain';

export interface GovernanceBodyListItem {
	id: string;
	bodyNumber: number;
	name: string;
	bodyType: string;
	status: GovernanceBodyStatus;
	/** Derived active-seat count — never stored. */
	activeMembers: number;
	createdAt: string;
}

export interface GovernanceMembership {
	id: string;
	bodyId: string;
	personId: string;
	personName: string;
	/** Bounded free-text seat label — carries NO powers. */
	title: string | null;
	startedAt: string;
	endedAt: string | null;
	endReason: string | null;
	/** Derived: true while the term is open. */
	active: boolean;
	createdAt: string;
}

export interface GovernanceBodyDetail {
	id: string;
	bodyNumber: number;
	name: string;
	bodyType: string;
	description: string | null;
	status: GovernanceBodyStatus;
	closedAt: string | null;
	/** Full membership timeline — current AND historical. */
	memberships: GovernanceMembership[];
	createdAt: string;
	updatedAt: string;
}

export interface GovernanceDecisionListItem {
	id: string;
	decisionNumber: number;
	bodyId: string;
	bodyName: string;
	title: string;
	status: GovernanceDecisionStatus;
	decisionOn: string;
	effectiveOn: string | null;
	voteCount: number;
	createdAt: string;
}

export interface GovernanceVote {
	id: string;
	decisionId: string;
	/** The membership row that established eligibility at cast time. */
	membershipId: string;
	personId: string;
	personName: string;
	membershipTitle: string | null;
	choice: GovernanceVoteChoice;
	castAt: string;
	note: string | null;
	/** Login actor who RECORDED the vote — not the voter. */
	recordedBy: string;
	createdAt: string;
}

export interface GovernanceDecisionDetail {
	id: string;
	decisionNumber: number;
	bodyId: string;
	bodyName: string;
	title: string;
	decisionText: string;
	/** Formal Decision Date — != Effective Date (docs/15). */
	decisionOn: string;
	effectiveOn: string | null;
	status: GovernanceDecisionStatus;
	openedAt: string | null;
	finalizedAt: string | null;
	/** Frozen finalization snapshot (null until finalized). */
	eligibleCount: number | null;
	approveCount: number | null;
	rejectCount: number | null;
	abstainCount: number | null;
	cancelledAt: string | null;
	cancellationReason: string | null;
	votes: GovernanceVote[];
	/** Active memberships of the body — the open-decision electorate. */
	eligibleMembers: GovernanceMembership[];
	createdAt: string;
	updatedAt: string;
}

export const GOVERNANCE_BODIES_PATH = '/api/governance/bodies';
export const governanceBodyPath = (id: string): string => `/api/governance/bodies/${id}`;
export const governanceBodyClosePath = (id: string): string =>
	`/api/governance/bodies/${id}/close`;
export const governanceBodyMembershipsPath = (id: string): string =>
	`/api/governance/bodies/${id}/memberships`;
export const governanceMembershipEndPath = (id: string): string =>
	`/api/governance/memberships/${id}/end`;

export const GOVERNANCE_DECISIONS_PATH = '/api/governance/decisions';
export const governanceDecisionPath = (id: string): string =>
	`/api/governance/decisions/${id}`;
export const governanceDecisionUpdatePath = (id: string): string =>
	`/api/governance/decisions/${id}/update`;
export const governanceDecisionOpenPath = (id: string): string =>
	`/api/governance/decisions/${id}/open`;
export const governanceDecisionCancelPath = (id: string): string =>
	`/api/governance/decisions/${id}/cancel`;
export const governanceDecisionVotesPath = (id: string): string =>
	`/api/governance/decisions/${id}/votes`;
export const governanceDecisionFinalizePath = (id: string): string =>
	`/api/governance/decisions/${id}/finalize`;

/**
 * Person lookup for membership/voter selection — scoped to
 * `governance.manage` so the operator resolves the canonical Person
 * without holding full parties read rights.
 */
export const GOVERNANCE_PERSONS_PATH = '/api/governance/persons';

export interface CreateGovernanceBodyRequest {
	name: string;
	bodyType: string;
	description?: string | null;
	idempotencyKey: string;
}

export interface CreateGovernanceMembershipRequest {
	personId: string;
	title?: string | null;
	/** RFC-3339; may be backdated, never future. */
	startedAt: string;
	idempotencyKey: string;
}

export interface EndGovernanceMembershipRequest {
	endedAt: string;
	reason: string;
}

export interface CreateGovernanceDecisionRequest {
	bodyId: string;
	title: string;
	decisionText: string;
	/** Formal Decision Date (`YYYY-MM-DD`). */
	decisionOn: string;
	effectiveOn?: string | null;
	idempotencyKey: string;
}

export interface UpdateGovernanceDecisionRequest {
	title: string;
	decisionText: string;
	decisionOn: string;
	effectiveOn?: string | null;
}

export interface CancelGovernanceDecisionRequest {
	reason: string;
}

export interface CastGovernanceVoteRequest {
	/** The voting Person — eligibility is verified server-side. */
	personId: string;
	choice: GovernanceVoteChoice;
	note?: string | null;
	idempotencyKey: string;
}

export interface FinalizeGovernanceDecisionRequest {
	/** The formally decided outcome — recorded, never computed. */
	outcome: GovernanceDecisionOutcome;
}
