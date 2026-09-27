# ADR-007 --- File and Object Storage

**Status:** Accepted

## Decision

Store file metadata/relationships in PostgreSQL and file bytes in
private S3-compatible object storage behind a storage abstraction.

Local development may use a local adapter.

## Requirements

-   private by default;
-   backend-authorized access or short-lived authorized access;
-   version history where replacement matters;
-   SHA-256 integrity/checksum metadata;
-   content type/size validation;
-   sensitive Social Aid documents receive appropriately narrow access.

Do not store sensitive files at predictable public URLs.
