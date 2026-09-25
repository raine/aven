-- Bootstrap staging stores only bytes verified against their descriptor slot
-- and never reclaims them, so a delayed PUT cannot change staging and needs no
-- per-candidate fence.
ALTER TABLE server_bootstrap_candidates DROP COLUMN epoch;
