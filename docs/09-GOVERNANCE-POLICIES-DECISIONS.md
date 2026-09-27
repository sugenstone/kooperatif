# 09 --- Governance, Policies and Decisions

## Purpose

Allow cooperative operating rules to change through authorized decisions
without hard-coding mutable business policy into application logic.

## Principle

**Invariant != Policy**

Invariant: - cannot be overridden by Board vote; - protects structural
integrity.

Policy: - configurable/versioned rule; - may change through authorized
governance.

## Governance entities

Candidate concepts: - Board/Committee; - Board Member assignment; -
Meeting; - Agenda Item; - Proposal; - Vote; - Decision; - Policy; -
Policy Version; - Effective Date; - supporting Document.

Exact legal board structure is configurable/domain-specific.

## Decision

A Decision should preserve: - decision number; - decision date; -
governing body; - subject; - text/summary; - vote result; - participants
where required; - attachments; - effective date; - affected Policies; -
status.

Decision Date and Effective Date are distinct.

## Policy

Policy is the stable conceptual rule.

Examples: - Period Assessment Policy; - Share Acquisition Fee Policy; -
Allocation Priority Policy; - Share Return Principal Policy; - Profit
Right Policy; - Value Protection Policy; - Approval Threshold Policy; -
Investment Valuation Policy.

## Policy Version

Each change creates a new version.

Candidate fields: - Policy; - version number; - status; -
effective-from; - effective-to; - parameters; - Decision reference; -
author/approver; - publication timestamp.

Published/effective versions are immutable.

## Effective dating

At a given business instant, the system resolves the applicable Policy
Version.

Historical transaction/calculation stores/references the version
actually used.

A later Policy change must not silently recalculate crystallized
historical rights unless an explicit authorized retroactive rule exists.

Retroactivity is exceptional and must be explicit.

## Policy parameters

Parameters may include: - amount; - percentage; - duration; -
eligibility; - allocation order; - index; - approval threshold; -
waiting period.

Parameters require typed/versioned validation.

Do not accept arbitrary unvalidated JSON as business logic.

## Dynamic rules

The application should support dynamic configuration where the business
truly changes, but not a general-purpose code/eval rules engine.

Prefer known Policy types with validated parameters and explicit
calculation handlers.

## Voting

If voting is modeled: - eligible voters; - vote choices; -
quorum/threshold; - opening/closing; - result; - abstention; -
conflict/recusal if needed.

Exact cooperative legal rules remain unresolved until specified.

## Approval vs Board Decision

Operational Approval and Governance Decision are related but distinct.

Example: - Policy says payments above X require approval; - Board
Decision creates/changes that Policy.

Do not model every operational approval as a Board vote.

## Policy simulation

Before activating high-impact financial Policy, future UI may show
impact preview: - affected Shares/Shareholders; - estimated
assessment; - effective date.

Preview does not create authoritative financial effects.

## Audit

Audit: - proposal; - vote; - Decision; - Policy Version creation; -
approval/publication; - effective date; - supersession.

## Permissions

Separate: - governance view; - proposal; - vote; - decision finalize; -
Policy draft; - Policy approve/publish; - emergency override if ever
allowed.

## Open decisions

-   exact board/committee structure;
-   quorum/voting rules;
-   electronic voting/legal evidentiary needs;
-   retroactive policy authority;
-   emergency policy process;
-   policy type catalog.
