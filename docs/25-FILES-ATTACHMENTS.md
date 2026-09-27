# 25 --- Files and Attachments

## Purpose

Manage source documents attached to domain entities. This is distinct
from generated PDF/report output.

## Examples

-   payment bank receipt/dekont;
-   title deed/tapu;
-   lease contract;
-   Board Decision scan;
-   Share transfer document;
-   appraisal;
-   investment contract;
-   Social Aid evidence;
-   identity/supporting documents where legally appropriate.

## Attachment model

Attachment should preserve: - tenant; - storage identity; - original
filename; - content type; - size; - checksum where useful; - uploader; -
upload time; - linked entity/entities; - document category; - access
classification; - archive/version relationship where applicable.

## Security

Files are private by default unless explicitly classified otherwise.

Access is backend-authorized.

Do not expose predictable public storage URLs for sensitive documents.

## Versioning

Where a document can be replaced by a newer version, preserve historical
version relationship rather than silently overwriting bytes.

## Deletion

Referenced/evidentiary files should normally archive rather than
destructively disappear.

Exact retention depends on legal/operational policy.

## Generated documents

Generated receipts/reports are governed by Reporting and Document
Design.

Do not confuse uploaded source files with generated documents.

## Storage architecture

Object/file storage provider is an architecture/deployment decision.

Database should not casually store large file blobs unless an ADR
explicitly chooses that model.

## Validation

Validate: - allowed size; - content/type; - upload integrity; - malware
strategy where appropriate.

## Social Aid

Sensitive beneficiary attachments require narrower permissions than
ordinary financial documents where appropriate.
