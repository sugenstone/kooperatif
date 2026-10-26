-- M2: user administration, forced password change, audit indexing.
-- PD-14 (recommended): admin reset issues one-time temporary password;
-- user must change it at next login.
-- PD-15 (recommended): audit view via dedicated roles; no automatic
-- deletion (retention >= 10 years handled operationally, not here).
-- NOT APPLIED to any database; clean-replay verification pending.

ALTER TABLE public.users
  ADD COLUMN must_change_password boolean NOT NULL DEFAULT false,
  ADD COLUMN password_changed_at timestamptz NULL;

COMMENT ON COLUMN public.users.must_change_password IS
  'True when an admin reset issued a temporary password (PD-14) or policy forces rotation.';

COMMENT ON COLUMN public.users.password_changed_at IS
  'Timestamp of last password change; NULL = never changed after provisioning.';

-- Audit viewer access patterns (PD-15):
--   list per cooperative, ordered by time; filter by event type; join actor.
CREATE INDEX security_events_coop_time_idx
  ON public.security_events (cooperative_id, occurred_at DESC);

CREATE INDEX security_events_event_type_idx
  ON public.security_events (event_type);

CREATE INDEX security_events_user_idx
  ON public.security_events (user_id)
  WHERE user_id IS NOT NULL;
