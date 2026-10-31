-- Session generation for user JWTs.
--
-- Access and refresh tokens carry this value. Deactivation and password changes increment it,
-- so previously issued tokens stop matching even if the caller still holds an unexpired refresh
-- token. Existing rows stay at 0, which is also the claim value of tokens issued before this
-- column existed, so those tokens keep working until the next revocation.

ALTER TABLE users
  ADD COLUMN IF NOT EXISTS session_version BIGINT NOT NULL DEFAULT 0;
