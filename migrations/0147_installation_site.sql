-- The site an installation's pages live in: set when the install creates its draft site, read by every
-- upgrade, nulled when the site is deleted (the next upgrade then creates a fresh draft site).
ALTER TABLE installations ADD COLUMN IF NOT EXISTS site_id UUID NULL REFERENCES sites(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS installations_site ON installations (site_id) WHERE site_id IS NOT NULL;
UPDATE installations i SET site_id = p.site_id
  FROM (SELECT DISTINCT ON (installation_id) installation_id, site_id FROM site_pages
        WHERE installation_id IS NOT NULL ORDER BY installation_id, generation DESC NULLS LAST, id) p
 WHERE i.id = p.installation_id AND i.site_id IS NULL;
