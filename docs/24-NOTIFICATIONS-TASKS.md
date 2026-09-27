# 24 --- Notifications and Tasks

## Purpose

Support future obligations and operational follow-up without embedding
reminder logic randomly in domains.

## Candidate triggers

-   Period collection date approaching;
-   outstanding debt;
-   approval waiting;
-   receivable due;
-   Share return principal due;
-   Profit Right due;
-   rent receivable/payable due;
-   investment expected-income checkpoint;
-   contract expiry;
-   Policy effective date;
-   Social Aid review/payment task.

## Notification vs Task

Notification = information delivered to a user.

Task = actionable work item with owner/state/due date.

One domain event may create either/both according to policy.

## Channels

Initial in-app notifications/tasks.

Email/SMS/push are future integrations and must not be assumed.

## Rules

-   notifications respect tenant and permission scope;
-   sensitive details are minimized in notification previews;
-   duplicate notifications should be controlled;
-   acknowledgement/read state is separate from underlying domain
    completion;
-   task completion must not silently execute financial actions.

## Scheduling

Use reliable server-side scheduling/job mechanism selected by
architecture.

Do not depend on an operator keeping a browser open.

## Audit

Critical notification/task generation and completion may be auditable
where useful.

## Localization

Default notification copy is Turkish and localizable.
