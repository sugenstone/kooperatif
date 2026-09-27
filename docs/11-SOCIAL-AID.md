# 11 --- Social Aid

## Purpose

Model the cooperative's non-profit Social Aid activity inside the same
application while keeping its finance, permissions and reporting clearly
separated from cooperative commercial/economic activity.

## Core rule

Social Aid is a separate financial context.

Its: - accounts/funds; - income/donations; - expenses/aid payments; -
balances; - reports; - permissions

must not be silently merged into cooperative profit or ordinary
operating cash.

## Participants

A Social Aid Supporter/Donor: - may be a Shareholder; - may be a
Guardian; - may be an outside person/entity; - need not have any
cooperative Share.

A Beneficiary: - may be a Shareholder; - may be an outside person; -
must not automatically gain cooperative membership/rights.

Use Party identity where appropriate to avoid duplicate real-person
records.

## Social Aid Financial Accounts/Funds

Examples: - Social Aid TL Cash; - Social Aid Bank Account; - Restricted
Education Fund; - Emergency Support Fund.

These are separate from ordinary cooperative Financial Accounts in
reporting/context, even if architecture reuses common ledger primitives.

## Donation/Support receipt

A support payment records: - supporter/payer; - amount/value; -
method; - Social Aid destination account/fund; - date; -
restriction/purpose if any; - receipt/document if enabled.

Donation does not create Share ownership, Period credit or Shareholder
excess.

## Anonymous support

May be supported if governance/legal policy permits.

Even then, financial transaction and receipt/account movement remain
authoritative.

## Restricted funds

A contribution may be restricted to a purpose.

Restricted balance cannot be used for another purpose without an
explicit legally/policy-valid reclassification process.

## Aid Request

Candidate fields: - applicant/requester; - Beneficiary; -
category/purpose; - requested amount/value; - supporting documents; -
privacy classification; - submission date; - status; - review notes; -
Decision/Approval; - approved amount; - funding source/fund.

## Aid Decision

Approval is separate from Payment.

Approved aid may create: - commitment/payable; - payment schedule; - one
or multiple actual Payments.

## Aid Payment

Actual aid disbursement: - reduces Social Aid Financial Account; -
references approved Aid Request/Decision where required; - preserves
Beneficiary; - method/date; - operator; - documents; -
reversal/correction history.

## Social Aid expenses

Administrative expenses related to Social Aid are tracked separately
from aid distributed to Beneficiaries.

Reports should distinguish: - donations/support received; - aid
distributed; - administrative/operating expenses; - restricted
balances; - unrestricted balances; - commitments/payables.

## No profit

Do not label positive Social Aid balance/surplus as cooperative profit.

Unspent resources remain Social Aid resources according to policy.

## Privacy

Social Aid may contain highly sensitive personal/supporting information.

Use: - narrower permissions; - minimum necessary display; - attachment
protection; - audit of sensitive access/export where appropriate.

Do not expose Beneficiary details on general cooperative dashboards
without need/permission.

## Receipts and donor statements

Support receipts/statements may be generated separately from cooperative
Shareholder receipts.

Do not mix donation receipts with Period Payment receipts.

## Cross-context transfer

Moving value between ordinary cooperative finance and Social Aid is not
a casual account transfer.

It requires explicit authorized domain event/Decision and classification
because economic ownership/purpose differs.

No implementation may permit unrestricted cross-context transfers by
default.

## Reporting

Candidate reports: - Social Aid account balances; - supporter/donation
report; - restricted-fund report; - aid distributed; - pending
commitments; - administrative expenses; - Beneficiary history with
restricted permission.

## Open decisions

-   legal structure/authority of Social Aid activity;
-   anonymous donation rules;
-   donation cancellation/refund;
-   restricted-fund reclassification;
-   aid approval thresholds;
-   Beneficiary data retention;
-   public transparency reports;
-   tax/receipt requirements.
