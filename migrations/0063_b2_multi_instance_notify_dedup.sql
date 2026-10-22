-- M1-P5 B2 fix — multi-instance delivery correctness.
--
-- Defect in 0062: the dedup primary key was `nonce` alone. PostgreSQL
-- NOTIFY broadcasts to EVERY listening backend; two independent app
-- instances each run a listener + verifier. The first verifier
-- consumed the nonce and delivered; the second got an ON CONFLICT hit
-- and dropped a legitimate event — replay protection had become
-- global suppression of honest listeners.
--
-- Correct semantics: deduplication must be per-LISTENER, not global.
-- Replay prevention is still global in effect (every instance drops
-- replays after its own first delivery), but each listener instance
-- must independently receive each authentic event exactly once.
--
-- Fix: namespace dedup rows by the verifying backend's own
-- pg_backend_pid(). The pid is chosen by PostgreSQL, not supplied by
-- the caller — so it is unforgeable. Each listener instance holds a
-- dedicated single-connection verify pool (see listen_loop), giving it
-- a stable pid for its lifetime; an attacker's replays only ever burn
-- rows under the attacker's own pid and can never suppress a real
-- listener. A reconnect produces a new pid: in-window replays may then
-- deliver once more — bounded and already covered by the Resync that
-- every reconnect publishes.
--
-- Row-volume bound is unchanged (30 min amortized prune); the worst
-- case multiplies retained nonces by the number of concurrent
-- listener backends, which is small.
--
-- Append-only. No historical migration is modified.

ALTER TABLE public.realtime_notify_dedup
    ADD COLUMN listener_pid integer NOT NULL DEFAULT 0;
ALTER TABLE public.realtime_notify_dedup
    DROP CONSTRAINT realtime_notify_dedup_pkey;
ALTER TABLE public.realtime_notify_dedup
    ADD PRIMARY KEY (nonce, listener_pid);

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

    -- Per-listener dedup: the row is keyed by (nonce, THIS backend's
    -- pid), so every independent listener instance receives each
    -- authentic event exactly once, while replays through the same
    -- backend collapse. Caller-supplied scope can never be chosen —
    -- pg_backend_pid() is server-assigned.
    INSERT INTO public.realtime_notify_dedup (nonce, listener_pid)
    VALUES (v_nonce, pg_backend_pid())
    ON CONFLICT (nonce, listener_pid) DO NOTHING;
    IF NOT FOUND THEN
        RETURN;
    END IF;

    -- Amortized prune: the horizon exceeds the replay window.
    DELETE FROM public.realtime_notify_dedup
     WHERE seen_at < v_now - interval '30 minutes';

    cooperative_id := v_coop;
    domain := v_parts[3];
    RETURN NEXT;
END;
$$;

REVOKE ALL ON FUNCTION public.app_notify_verify(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_notify_verify(text) TO kooperatif_app;

-- ── Postchecks ─────────────────────────────────────────────────────

DO $$
DECLARE
    v_key     bytea;
    v_nonce   uuid;
    v_ts      text;
    v_mac     text;
    v_payload text;
    v_claims  int;
BEGIN
    SELECT s.value INTO v_key FROM public.app_secrets s
     WHERE s.name = 'realtime_notify_key';
    v_nonce := gen_random_uuid();
    v_ts := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint::text;
    v_mac := lower(encode(hmac(
        convert_to('payments|-|' || v_nonce::text || '|' || v_ts, 'UTF8'),
        v_key, 'sha256'), 'hex'));
    v_payload := 'v2|-|payments|' || v_nonce::text || '|' || v_ts || '|' || v_mac;

    -- First verify on THIS backend delivers.
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 1 THEN
        RAISE EXCEPTION 'valid v2 payload was not verified';
    END IF;
    -- Replay on the SAME backend dedups.
    SELECT count(*) INTO v_claims FROM public.app_notify_verify(v_payload);
    IF v_claims <> 0 THEN
        RAISE EXCEPTION 'same-backend replay was not deduplicated';
    END IF;
    -- Dedup row must carry this backend's pid.
    IF NOT EXISTS (SELECT 1 FROM public.realtime_notify_dedup
                   WHERE nonce = v_nonce
                     AND listener_pid = pg_backend_pid()) THEN
        RAISE EXCEPTION 'dedup row missing listener_pid attribution';
    END IF;
    -- Multi-instance lane: the same nonce under a DIFFERENT pid must
    -- still deliver. Simulate a foreign listener backend directly.
    INSERT INTO public.realtime_notify_dedup (nonce, listener_pid)
    VALUES (v_nonce, pg_backend_pid() + 1);
    IF NOT FOUND THEN
        RAISE EXCEPTION 'per-listener namespace did not accept a second backend';
    END IF;

    IF (SELECT count(*) FROM pg_trigger
         WHERE tgname LIKE 'rt_%' AND NOT tgisinternal
           AND tgfoid = 'public.kooperatif_notify_domain()'::regprocedure)
       <> 37 THEN
        RAISE EXCEPTION 'rt_* trigger inventory drifted';
    END IF;
END $$;
