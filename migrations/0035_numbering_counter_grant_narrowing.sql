-- M1-P4-P2B — numbering counter grant narrowing.
--
-- Finding (P2B grant audit): the 0033 business-CRUD loop covers every
-- cooperative_id NOT NULL table except cooperative_memberships, so
-- cooperative_number_counters received full CRUD including DELETE.
-- The runtime only reaches the counter through the SECURITY INVOKER
-- allocator/reserve functions (numbering.rs has no direct table
-- access), which require SELECT, INSERT and UPDATE — never DELETE.
-- A deleted counter row would silently restart the series at 1 and
-- collide with issued document numbers, so DELETE is revoked.
SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';

REVOKE DELETE ON TABLE public.cooperative_number_counters FROM kooperatif_app;

-- Fail-closed post-check: the minimal verb set holds exactly.
DO $$
BEGIN
    IF NOT (
        has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'SELECT')
        AND has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'INSERT')
        AND has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'UPDATE')
        AND NOT has_table_privilege('kooperatif_app', 'public.cooperative_number_counters', 'DELETE')
    ) THEN
        RAISE EXCEPTION 'cooperative_number_counters grant set is not {SELECT, INSERT, UPDATE}';
    END IF;
END $$;
