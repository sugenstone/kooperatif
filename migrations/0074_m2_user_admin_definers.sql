-- M2 — user administration definers (scoped lane).
--
-- Two credential-bound SECURITY DEFINER writers behind the cooperative
-- scope seal — the same boundary shape as app_membership_create (0070):
--
--   * app_user_create_scoped — cooperative administrator provisions a
--     NEW global identity AND its membership in the sealed scope in one
--     transaction. The cooperative is derived from the sealed verified
--     scope (app_rbac_verified_scope_in) — callers can never name a
--     target cooperative, so cross-cooperative provisioning is
--     structurally impossible. The global identity is owned by the
--     platform: only the membership is cooperative-scoped.
--
--   * app_user_update_scoped — edit-safe profile fields (display_name)
--     for a member of the sealed scope. Username/status/password are
--     deliberately not mutable here: status moves through the platform
--     lane (app_platform_user_set_status), password through the auth
--     lane.
--
-- Audit: the membership INSERT mints cooperative_membership_granted
-- through the audited transition trigger; the identity mint and profile
-- update emit user_created / user_updated via app_audit_emit — the same
-- emit lane the transition trigger uses. The runtime role keeps zero
-- direct privileges on users/cooperative_memberships.
--
-- Permission: memberships.manage (or the broader users.manage) verified
-- INSIDE the definer — never trusted from the request.
--
-- Append-only. No historical migration is modified.

SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

-- ------------------------------------------------------------------
-- app_user_create_scoped: provision identity + membership atomically.
-- p_password_hash is the Argon2id PHC string computed Rust-side;
-- p_verifier_password is the plaintext used ONLY to seed the DB-side
-- bcrypt login_verifier checked by app_session_open.
-- Returns the new user id.
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_user_create_scoped(
    p_username           text,
    p_display_name       text,
    p_password_hash      text,
    p_verifier_password  text
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope       uuid := public.app_rbac_verified_scope_in(
        ARRAY['memberships.manage', 'users.manage']);
    v_actor       uuid := nullif(current_setting('app.user_id', true), '')::uuid;
    v_coop_status text;
    v_user_id     uuid;
BEGIN
    IF v_scope IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: memberships.manage is cooperative-scoped'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT c.status INTO v_coop_status
      FROM public.cooperatives c
     WHERE c.id = v_scope;
    IF v_coop_status IS DISTINCT FROM 'active' THEN
        RAISE EXCEPTION 'cooperative_not_ready: %', v_coop_status
            USING ERRCODE = 'check_violation';
    END IF;

    IF btrim(p_username) = '' OR btrim(p_display_name) = '' THEN
        RAISE EXCEPTION 'user_fields_required'
            USING ERRCODE = 'check_violation';
    END IF;
    IF length(btrim(p_username)) > 120 OR length(btrim(p_display_name)) > 120 THEN
        RAISE EXCEPTION 'user_field_too_long'
            USING ERRCODE = 'check_violation';
    END IF;
    IF p_verifier_password IS NULL OR length(p_verifier_password) < 8 THEN
        RAISE EXCEPTION 'password_too_short'
            USING ERRCODE = 'check_violation';
    END IF;

    INSERT INTO public.users
        (username, display_name, status, password_hash, login_verifier,
         must_change_password)
    VALUES (btrim(p_username), btrim(p_display_name), 'active',
            p_password_hash, crypt(p_verifier_password, gen_salt('bf')),
            true)
    RETURNING id INTO v_user_id;
    -- PD-14 (recommended): an operator-provisioned credential is a
    -- one-time credential — the user must replace it at next login.
    -- The unique lower(username) index raises 23505 on collision;
    -- callers map it to conflict.

    INSERT INTO public.cooperative_memberships
        (cooperative_id, user_id, status, created_by_user_id)
    VALUES (v_scope, v_user_id, 'active', v_actor);
    -- mints cooperative_membership_granted via transition trigger.

    PERFORM public.app_audit_emit('user_created', jsonb_build_object(
        'user_id', v_user_id,
        'cooperative_id', v_scope,
        'lane', 'scoped_admin_provision'));

    RETURN v_user_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_user_create_scoped(text, text, text, text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_create_scoped(text, text, text, text)
    TO kooperatif_app;

-- ------------------------------------------------------------------
-- app_user_update_scoped: edit-safe profile fields for a member of
-- the sealed scope. Only display_name is mutable here.
-- Returns the updated user id; NULL when the target is not a member
-- of the scope (caller maps to 404 — existence is not leaked).
-- ------------------------------------------------------------------
CREATE OR REPLACE FUNCTION public.app_user_update_scoped(
    p_user_id      uuid,
    p_display_name text
)
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_scope uuid := public.app_rbac_verified_scope_in(
        ARRAY['memberships.manage', 'users.manage']);
BEGIN
    IF v_scope IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: memberships.manage is cooperative-scoped'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    IF btrim(coalesce(p_display_name, '')) = '' THEN
        RAISE EXCEPTION 'user_fields_required'
            USING ERRCODE = 'check_violation';
    END IF;
    IF length(btrim(p_display_name)) > 120 THEN
        RAISE EXCEPTION 'user_field_too_long'
            USING ERRCODE = 'check_violation';
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM public.cooperative_memberships m
         WHERE m.cooperative_id = v_scope AND m.user_id = p_user_id)
    THEN
        RETURN NULL; -- not a member of this scope -> 404, no leak
    END IF;

    UPDATE public.users u
       SET display_name = btrim(p_display_name),
           updated_at   = now()
     WHERE u.id = p_user_id;

    PERFORM public.app_audit_emit('user_updated', jsonb_build_object(
        'user_id', p_user_id,
        'cooperative_id', v_scope,
        'fields', jsonb_build_array('display_name')));

    RETURN p_user_id;
END;
$function$;
REVOKE ALL ON FUNCTION public.app_user_update_scoped(uuid, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_update_scoped(uuid, text)
    TO kooperatif_app;
