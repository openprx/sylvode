-- The time a site was last published (console "last published"). Set when a site goes from not
-- published to published and kept when it is unpublished or disabled, so it reads "last published at".
-- Sites published before this migration keep NULL. Safe to run again.

ALTER TABLE sites ADD COLUMN IF NOT EXISTS published_at TIMESTAMPTZ NULL;
