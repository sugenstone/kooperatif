# ADR-004 --- Money and Value Representation

**Status:** Accepted

## Decision

Use PostgreSQL exact `NUMERIC` representation and exact decimal
arithmetic in Rust. Frontend/API contracts must preserve decimals
without lossy JavaScript floating-point assumptions.

Use typed asset/value codes, e.g. `TRY`, future fiat currencies, and
commodity units such as gram gold.

## Quantity vs valuation

Authoritative asset quantity is distinct from its valuation.

Example: - quantity: `125.4375 XAU_GRAM` - valuation rate:
dated/source-attributed TRY rate - valuation: derived result

Changing market price never rewrites historical asset quantity.

## Precision

Precision/scale and rounding are defined per value type/use case. Do not
use `f32`/`f64` for authoritative financial calculations.
