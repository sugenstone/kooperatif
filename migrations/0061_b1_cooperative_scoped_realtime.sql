-- M1-P5 B1 — cooperative-scoped realtime delivery (discovery: RT-1/RT-2).
--
-- The 0016 contract emitted pg_notify('kooperatif_domain_changed',
-- <domain>) — a payload with no tenant identity. Every connected
-- socket received a delivery attempt for every commit anywhere in the
-- database; the only filter was a permission check. Under the
-- multi-cooperative target architecture that is a cross-cooperative
-- information channel (foreign commit timing/volume) plus a refetch
-- amplification vector.
--
-- What changes:
--   1. Payload contract v1: `v1|<cooperative_id|->|<domain>|<hmac>` —
--      the cooperative id is derived from the affected ROW (NEW/OLD
--      .cooperative_id), never from client input or GUCs; '-' encodes
--      the NULL-cooperative legacy lane so never-bootstrapped
--      installations keep receiving their signals.
--   2. Forgery control: PostgreSQL grants no privilege over NOTIFY —
--      `kooperatif_app` can emit arbitrary payloads on the channel
--      (discovery RT-2, live-verified). The compensating control is a
--      per-database HMAC-SHA256 over `domain|cooperative` keyed by a
--      random secret that lives in `app_secrets` (owner-only ACL; the
--      runtime role holds no SELECT). The trigger computes it as the
--      definer owner; the listener verifies through the definer
--      `app_notify_verify`, which returns parsed fields only for a
--      valid signature and never exposes the key.
--      Residual, documented: a principal that can LISTEN on the channel
--      can replay observed signed payloads — impact is a same-scope
--      spurious refetch only (payloads carry no data). No defense in
--      this step; tracked as a bounded follow-up.
--   3. `kooperatif_notify_domain` becomes SECURITY DEFINER: it must
--      read the owner-only secret; `search_path` is pinned and the
--      trigger invoker's privileges are irrelevant to its internals.
--
-- The listener (Rust) drops anything that fails verification — old
-- bare-tag payloads, malformed envelopes and forged payloads alike —
-- and routes `Changed{domain, cooperative}` only to sockets whose
-- verified scope equals the event cooperative.
--
-- Append-only. No historical migration is modified. Delivery-side
-- filtering happens in the server before WebSocket send.

-- 1. Per-database signing key. Random per database, generated at
--    migration time; app_secrets is owner-only (S-series hardening),
--    so neither the runtime role nor a pg_dump restore of grants can
--    expose it. ON CONFLICT keeps a deterministic replay idempotent.
INSERT INTO public.app_secrets (name, value)
VALUES ('realtime_notify_key', gen_random_bytes(32))
ON CONFLICT (name) DO NOTHING;

-- 2. Trigger function: emit the signed v1 envelope. Row-derived scope:
--    DELETE signals OLD.cooperative_id; INSERT/UPDATE signal
--    NEW.cooperative_id. A NULL cooperative (pre-tenancy residue; all
--    37 rt_-triggered tables are NOT NULL today) maps to the '-'
--    placeholder so the never-bootstrapped lane keeps working.
CREATE OR REPLACE FUNCTION public.kooperatif_notify_domain()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_coop uuid;
    v_key  bytea;
    v_scoped text;
BEGIN
    v_coop := CASE WHEN TG_OP = 'DELETE'
                   THEN OLD.cooperative_id
                   ELSE NEW.cooperative_id END;
    v_scoped := coalesce(v_coop::text, '-');

    SELECT s.value INTO v_key
      FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'realtime_notify_key missing — cannot emit signed domain signal';
    END IF;

    PERFORM pg_notify(
        'kooperatif_domain_changed',
        'v1|' || v_scoped || '|' || TG_ARGV[0] || '|' ||
        encode(hmac(convert_to(TG_ARGV[0] || '|' || v_scoped, 'UTF8'), v_key, 'sha256'), 'hex'));
    RETURN NULL;
END;
$$;

-- Trigger functions are invoked internally by the trigger machinery;
-- direct EXECUTE must not be available to PUBLIC.
REVOKE ALL ON FUNCTION public.kooperatif_notify_domain() FROM PUBLIC;

-- 3. Listener-side verification: parse + MAC-check a payload and return
--    its claims only when the signature matches. The secret never
--    leaves the database — callers learn `(cooperative_id, domain)` for
--    authentic payloads and an empty result for everything else.
CREATE OR REPLACE FUNCTION public.app_notify_verify(p_payload text)
RETURNS TABLE (cooperative_id uuid, domain text)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_parts text[];
    v_key   bytea;
    v_coop  uuid;
BEGIN
    v_parts := string_to_array(p_payload, '|');
    IF v_parts IS NULL OR array_length(v_parts, 1) <> 4
       OR v_parts[1] <> 'v1' THEN
        RETURN;
    END IF;
    IF v_parts[2] = '-' THEN
        v_coop := NULL;
    ELSE
        BEGIN
            v_coop := v_parts[2]::uuid;
        EXCEPTION WHEN invalid_text_representation THEN
            RETURN;
        END;
    END IF;

    SELECT s.value INTO v_key
      FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RETURN;
    END IF;
    IF v_parts[4] <> lower(encode(
            hmac(convert_to(v_parts[3] || '|' || v_parts[2], 'UTF8'), v_key, 'sha256'),
            'hex')) THEN
        RETURN;
    END IF;

    cooperative_id := v_coop;
    domain := v_parts[3];
    RETURN NEXT;
END;
$$;

REVOKE ALL ON FUNCTION public.app_notify_verify(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_notify_verify(text) TO kooperatif_app;

-- ── Postchecks (fail the migration on drift) ────────────────────────

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.app_secrets
                   WHERE name = 'realtime_notify_key'
                     AND octet_length(value) = 32) THEN
        RAISE EXCEPTION 'realtime_notify_key was not provisioned';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_proc
                   WHERE oid = 'public.kooperatif_notify_domain()'::regprocedure
                     AND prosecdef) THEN
        RAISE EXCEPTION 'kooperatif_notify_domain must be SECURITY DEFINER';
    END IF;
    IF NOT has_function_privilege('kooperatif_app',
            'public.app_notify_verify(text)', 'EXECUTE') THEN
        RAISE EXCEPTION 'kooperatif_app lacks app_notify_verify EXECUTE';
    END IF;
    -- PUBLIC EXECUTE = an explicit grantee=0 acl entry OR the implicit
    -- default (NULL proacl). Both must be absent on the two functions.
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.oid = 'public.app_notify_verify(text)'::regprocedure
           AND (p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0))) THEN
        RAISE EXCEPTION 'app_notify_verify must not be PUBLIC-executable';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.oid = 'public.kooperatif_notify_domain()'::regprocedure
           AND (p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0))) THEN
        RAISE EXCEPTION 'kooperatif_notify_domain must not be PUBLIC-executable';
    END IF;
    -- Every realtime trigger still bound to the notifier.
    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal
           AND tgfoid = 'public.kooperatif_notify_domain()'::regprocedure)
       <> 37 THEN
        RAISE EXCEPTION 'rt_* trigger inventory drifted';
    END IF;
END $$;
