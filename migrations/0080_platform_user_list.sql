-- 0080 — Platform user picker read model.
--
-- The first-administrator selector on /platform/kooperatifler/[id]
-- needs to enumerate existing identities WITHOUT a cooperative
-- membership. `users` is FORCE-RLS so only a SECURITY DEFINER read
-- can serve the lane. This function mirrors
-- app_cooperative_list_platform (0068 §5): session + NULL-scope
-- permission verified inside the function, bounded pagination,
-- deterministic ordering, and only non-sensitive identity columns —
-- never password_hash, login_verifier, reset or session material.

CREATE OR REPLACE FUNCTION public.app_user_list_platform(
    p_token_hash      bytea,
    p_now             timestamptz,
    p_search          text DEFAULT NULL,
    p_limit           integer DEFAULT 200
)
RETURNS TABLE (
    id               uuid,
    username         text,
    display_name     text,
    status           text
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path TO 'public', 'pg_catalog'
AS $function$
DECLARE
    v_user uuid;
    v_pattern text;
BEGIN
    v_user := public.app_session_owner(p_token_hash, p_now);
    IF v_user IS NULL THEN
        RAISE EXCEPTION 'insufficient_privilege: valid session required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF NOT public.app_has_platform_permission(v_user, 'users.read') THEN
        RAISE EXCEPTION 'insufficient_privilege: platform users.read required'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    v_pattern := CASE
        WHEN p_search IS NULL OR btrim(p_search) = '' THEN NULL
        ELSE '%' || replace(replace(replace(btrim(p_search), '\', '\\'), '%', '\%'), '_', '\_') || '%'
    END;

    RETURN QUERY
        SELECT u.id, u.username, u.display_name, u.status
          FROM public.users u
         WHERE (v_pattern IS NULL
                OR u.username ILIKE v_pattern ESCAPE '\'
                OR u.display_name ILIKE v_pattern ESCAPE '\')
         ORDER BY u.username, u.id
         LIMIT least(greatest(coalesce(p_limit, 200), 1), 500);
END
$function$;
REVOKE ALL ON FUNCTION public.app_user_list_platform(
    bytea, timestamptz, text, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.app_user_list_platform(
    bytea, timestamptz, text, integer) TO kooperatif_app;
