-- M1-P5 B2 — realtime replay / amplification hardening.
--
-- Residual risk left by B1 (documented in 0061): kooperatif_app can
-- LISTEN on `kooperatif_domain_changed` and replay a captured, validly
-- signed v1 payload forever. The MAC proves authenticity but not
-- freshness or uniqueness, so one captured notification can be replayed
-- into unbounded WebSocket deliveries and REST refetches for the
-- payload's cooperative (the B1 scope filter already confines the
-- blast radius to that cooperative — replay cannot cross tenants).
--
-- What changes:
--   1. Payload contract v2:
--      `v2|<cooperative_id|->|<domain>|<nonce>|<ts_ms>|<hmac>`
--      - nonce: gen_random_uuid() per notification (unforgeable by
--        itself, but makes every authentic payload distinct).
--      - ts_ms: emit time (clock_timestamp, epoch milliseconds).
--      - hmac: SHA256 over `domain|coop|nonce|ts_ms` keyed by the
--        same owner-only `realtime_notify_key` from 0061.
--      v1 payloads are no longer accepted — there is no downgrade lane.
--   2. Freshness: app_notify_verify rejects payloads whose timestamp
--      is older than REPLAY_WINDOW (120 s) or more than FUTURE_SKEW
--      (10 s) ahead of the database clock. A payload captured
--      yesterday is worthless.
--   3. Uniqueness: realtime_notify_dedup (owner-only, no grants) is a
--      bounded seen-set. app_notify_verify inserts the nonce inside
--      the same statement that authenticates it; ON CONFLICT means
--      "already delivered once" → empty result. The insert happens in
--      the verifying (definer) transaction, and the check+insert is
--      atomic with respect to concurrent verifiers via the PK.
--      Rows outlive the replay window (30 min horizon, pruned
--      amortized on each call) so memory/disk stays bounded.
--   4. The dedup table is cluster-global by construction: any number
--      of listener instances share the same seen-set, so a replay
--      fired at instance N cannot resurface through instance M.
--      Listener restarts keep dedup state — a restart during a replay
--      flood does not re-deliver.
--
-- Design choices and why alternatives were rejected:
--   - Timestamp alone: insufficient — two notifications in the same
--     millisecond would collide; nonce guarantees distinctness.
--   - In-memory dedup in the Rust listener: does not survive restarts
--     and diverges across multi-instance deployments; DB-side dedup
--     is shared, durable-enough and costs zero listener memory.
--   - Per-cooperative rate limiting/coalescing: not needed as a
--     security boundary once replay is identity-neutralized — every
--     distinct legitimate commit still delivers (its nonce is new),
--     and the frontend already coalesces 150 ms. Documented, not
--     implemented.
--   - Redis/Kafka: unnecessary — the dedup set is small and the
--     database is already the authority on authenticity.
--
-- Residual, documented: within REPLAY_WINDOW an attacker can still
-- amplify ONE authentic payload into exactly ONE delivery (the first
-- arrival wins; ordering between original and replay is irrelevant
-- because dedup collapses them). Forged payloads without valid MAC
-- remain dropped. The verify function call per plausible payload is
-- the remaining cost surface; the Rust listener pre-filters shape and
-- staleness cheaply before that call.
--
-- Append-only. No historical migration is modified.

-- 1. Bounded, owner-only dedup set. No runtime-role privileges at
--    all; the verifying definer (owner) reads/writes it internally.
CREATE TABLE public.realtime_notify_dedup (
    nonce   uuid PRIMARY KEY,
    seen_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX realtime_notify_dedup_seen_at_idx
    ON public.realtime_notify_dedup (seen_at);
REVOKE ALL ON public.realtime_notify_dedup FROM PUBLIC;

-- 2. Trigger function: emit the v2 envelope. Row-derived cooperative
--    scope is unchanged from 0061 (NEW for INSERT/UPDATE, OLD for
--    DELETE, '-' for the never-bootstrapped NULL lane).
CREATE OR REPLACE FUNCTION public.kooperatif_notify_domain()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_coop  uuid;
    v_key   bytea;
    v_scope text;
    v_nonce uuid;
    v_ts    text;
BEGIN
    v_coop := CASE WHEN TG_OP = 'DELETE'
                   THEN OLD.cooperative_id
                   ELSE NEW.cooperative_id END;
    v_scope := coalesce(v_coop::text, '-');
    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint::text;

    SELECT s.value INTO v_key
      FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'realtime_notify_key missing — cannot emit signed domain signal';
    END IF;

    PERFORM pg_notify(
        'kooperatif_domain_changed',
        'v2|' || v_scope || '|' || TG_ARGV[0] || '|' || v_nonce::text || '|' || v_ts || '|' ||
        encode(hmac(convert_to(TG_ARGV[0] || '|' || v_scope || '|' || v_nonce::text || '|' || v_ts, 'UTF8'),
                    v_key, 'sha256'), 'hex'));
    RETURN NULL;
END;
$$;

REVOKE ALL ON FUNCTION public.kooperatif_notify_domain() FROM PUBLIC;

-- 3. Verifier: parse → MAC → freshness → atomic dedup → claims.
--    Any failure mode returns an empty result; the secret and the
--    dedup table never touch the caller's privileges.
CREATE OR REPLACE FUNCTION public.app_notify_verify(p_payload text)
RETURNS TABLE (cooperative_id uuid, domain text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public, pg_catalog
AS $$
DECLARE
    v_parts  text[];
    v_key    bytea;
    v_coop   uuid;
    v_nonce  uuid;
    v_ts_ms  bigint;
    v_emit   timestamptz;
    v_now    timestamptz := clock_timestamp();
BEGIN
    v_parts := string_to_array(p_payload, '|');
    IF v_parts IS NULL OR array_length(v_parts, 1) <> 6
       OR v_parts[1] <> 'v2' THEN
        RETURN;
    END IF;

    BEGIN
        v_coop := CASE WHEN v_parts[2] = '-'
                       THEN NULL ELSE v_parts[2]::uuid END;
        v_nonce := v_parts[4]::uuid;
        v_ts_ms := v_parts[5]::bigint;
    EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range THEN
        RETURN;
    END;

    -- Freshness before MAC: stale/implausible payloads are cheap to
    -- drop and their nonces must NOT consume dedup rows.
    v_emit := to_timestamp(v_ts_ms / 1000.0);
    IF v_emit < v_now - interval '120 seconds'
       OR v_emit > v_now + interval '10 seconds' THEN
        RETURN;
    END IF;

    SELECT s.value INTO v_key
      FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RETURN;
    END IF;
    IF v_parts[6] <> lower(encode(
            hmac(convert_to(v_parts[3] || '|' || v_parts[2] || '|' ||
                            v_parts[4] || '|' || v_parts[5], 'UTF8'),
                 v_key, 'sha256'), 'hex')) THEN
        RETURN;
    END IF;

    -- Atomic first-writer-wins dedup: the PK conflict marks this
    -- nonce as already delivered. Replays — even ones that beat the
    -- original to the listener — collapse to a single delivery.
    INSERT INTO public.realtime_notify_dedup (nonce)
    VALUES (v_nonce)
    ON CONFLICT (nonce) DO NOTHING;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    -- Amortized prune: the horizon exceeds the replay window, so a
    -- nonce only needs retention while replays of its payload could
    -- still pass freshness. Growth is bounded by emit volume × 30 min.
    DELETE FROM public.realtime_notify_dedup
     WHERE seen_at < v_now - interval '30 minutes';

    cooperative_id := v_coop;
    domain := v_parts[3];
    RETURN NEXT;
END;
$$;

REVOKE ALL ON FUNCTION public.app_notify_verify(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_notify_verify(text) TO kooperatif_app;

-- ── Postchecks (fail the migration on drift) ────────────────────────

DO $$
DECLARE
    v_key     bytea;
    v_nonce   uuid;
    v_ts      text;
    v_mac     text;
    v_payload text;
    v_claims  int;
BEGIN
    -- v2 self-check: craft a fully valid payload as the owner,
    -- verify it once (must return claims), then again (dedup drop).
    SELECT s.value INTO v_key FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'realtime_notify_key was not provisioned';
    END IF;

    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint::text;
    v_mac := lower(encode(hmac(
        convert_to('payments|-|' || v_nonce::text || '|' || v_ts, 'UTF8'),
        v_key, 'sha256'), 'hex'));
    v_payload := 'v2|-|payments|' || v_nonce::text || '|' || v_ts || '|' || v_mac;

    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 1 THEN
        RAISE EXCEPTION 'valid v2 payload was not verified';
    END IF;
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 0 THEN
        RAISE EXCEPTION 'replayed v2 payload was not deduplicated';
    END IF;

    -- Stale but correctly-signed payload must be rejected.
    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp() - interval '1 hour') * 1000)::bigint::text;
    v_mac := lower(encode(hmac(
        convert_to('payments|-|' || v_nonce::text || '|' || v_ts, 'UTF8'),
        v_key, 'sha256'), 'hex'));
    v_payload := 'v2|-|payments|' || v_nonce::text || '|' || v_ts || '|' || v_mac;
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 0 THEN
        RAISE EXCEPTION 'expired v2 payload was not rejected';
    END IF;

    -- Bad MAC on otherwise-valid v2 shape must be rejected.
    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint::text;
    v_payload := 'v2|-|payments|' || v_nonce::text || '|' || v_ts || '|' ||
                 repeat('0', 64);
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 0 THEN
        RAISE EXCEPTION 'tampered v2 payload was not rejected';
    END IF;

    -- v1 payloads have no downgrade lane.
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(
        'v1|-|payments|' || repeat('0', 64));
    IF v_claims <> 0 THEN
        RAISE EXCEPTION 'v1 payload was not rejected';
    END IF;

    IF EXISTS (
        SELECT 1 FROM pg_proc p
         WHERE p.oid = 'public.app_notify_verify(text)'::regprocedure
           AND (p.proacl IS NULL
                OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) x
                            WHERE x.grantee = 0))) THEN
        RAISE EXCEPTION 'app_notify_verify must not be PUBLIC-executable';
    END IF;
    IF NOT has_function_privilege('kooperatif_app',
            'public.app_notify_verify(text)', 'EXECUTE') THEN
        RAISE EXCEPTION 'kooperatif_app lacks app_notify_verify EXECUTE';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_class c
         WHERE c.oid = 'public.realtime_notify_dedup'::regclass
           AND EXISTS (SELECT 1 FROM aclexplode(c.relacl) x
                        WHERE x.grantee <> 0
                          AND x.grantee <> c.relowner)) THEN
        RAISE EXCEPTION 'realtime_notify_dedup must be owner-only';
    END IF;
    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal
           AND tgfoid = 'public.kooperatif_notify_domain()'::regprocedure)
       <> 37 THEN
        RAISE EXCEPTION 'rt_* trigger inventory drifted';
    END IF;
END $$;
