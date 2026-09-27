# 17 --- Calculation Rules Registry

## Purpose

Provide one registry for formulas and calculation semantics. Unknown
formulas remain explicitly unresolved; agents must not invent them.

## Status vocabulary

-   `DEFINED` --- safe to implement as specified.
-   `POLICY_DRIVEN` --- formula/parameters come from a versioned Policy.
-   `UNRESOLVED` --- do not implement business assumption.

## Period Assessment

Status: `POLICY_DRIVEN`

Known direction: - assessment is created per eligible Share; - Period
policy defines amount/rule and eligibility; - created assessment
preserves governing Policy Version.

Exact eligibility and formula: unresolved until Period specification.

## Shareholder total outstanding debt

Status: `DEFINED`

Derived as the sum of eligible open outstanding obligations attributed
to that Shareholder under historical/current ownership semantics.

Do not store as independently editable truth.

## Family total outstanding debt

Status: `DEFINED`

Derived presentation aggregate of selected/current Family members'
individual obligations.

Family aggregate does not transfer ownership of debt between members.

## New Cash Collected

Status: `DEFINED`

Actual qualifying new shareholder-payment value entering Financial
Accounts during the report interval.

Excludes Prior Excess Applied.

## Prior Excess Applied

Status: `DEFINED`

Value received in an earlier event/period and allocated to current
Period obligations.

## Total Period Debt Satisfied

Status: `DEFINED WITH EXTENSIBILITY`

At minimum includes valid settlement allocations to Period Assessments,
including Prior Excess Applied and current-payment allocations.

Other settlement sources require explicit specification.

## Outstanding Period Debt

Status: `DEFINED`

`Period Assessment Total - valid satisfied amount`, subject to
reversals/corrections.

## Payment allocation priority

Status: `UNRESOLVED / POLICY_DRIVEN`

Do not choose oldest-first, newest-first, current-period-first or Share
order automatically until approved.

## Individual overpayment excess attribution

Status: `POLICY_DRIVEN`

Must have explicit owner/context.

## Family overpayment excess attribution

Status: `UNRESOLVED / POLICY_DRIVEN`

Never infer from payer, guardian, equal split or Family membership.

## Share Acquisition Fee

Status: `POLICY_DRIVEN`

May be zero/optional. Founder-period vs later allocation may affect
default rule.

## Share Invested Amount

Status: `POLICY_DRIVEN`

Must exclude acquisition fee unless an explicit policy says otherwise.

## Reference Share Value

Status: `UNRESOLVED / POLICY_DRIVEN`

Potentially based on Net Asset Value and eligible Share count, but no
formula is authorized yet.

## Cooperative Net Asset Value

Status: `UNRESOLVED / POLICY_DRIVEN`

Potential components: - Financial Accounts; - Investments; - Business
Holdings; - Real Estate; - Receivables; - Other Assets; - minus
Liabilities.

Inclusion, valuation basis and timing remain policy-driven.

## Profit Right

Status: `UNRESOLVED / POLICY_DRIVEN`

Do not implement a formula yet.

Required future inputs may include: - economic participation interval; -
Share ownership; - realized/unrealized results; - expenses; -
liabilities; - asset valuations; - Policy Version; - exit cutoff.

## Refundable Invested Amount

Status: `UNRESOLVED / POLICY_DRIVEN`

May differ from raw historical payments.

## Value Protection

Status: `POLICY_DRIVEN`

Potential modes: - none; - inflation/index; - gold; - fixed rate; -
approved formula.

Base right remains immutable.

## Investment expected income

Status: `DEFINED AS FORECAST DATA`

Expected income is stored/derived forecast and is never automatically
realized income.

## Business Holding distributions

Status: `DEFINED SEMANTICALLY`

Business profit, declared distribution/receivable and cash received are
separate events.

## Real estate valuation

Status: `POLICY_DRIVEN`

Valuation source/date/method must be preserved.

## Social Aid fund availability

Status: `POLICY_DRIVEN WITH INVARIANTS`

Restricted and unrestricted resources must remain distinguishable. Exact
allocation priority requires Social Aid specification.

## Calculation explainability

For every implemented financial formula, record enough metadata to
answer: - inputs; - formula/policy; - Policy Version; - effective
date; - valuation date where relevant; - source transactions; - rounding
rule; - resulting value.

## Rounding

Status: `UNRESOLVED`

Do not independently choose rounding precision/mode for financial
rights, gold/commodity units or allocation residuals. Define per value
type before implementation.

## Currency/commodity conversion

Status: `UNRESOLVED`

Do not invent exchange-rate or gold conversion sources.
