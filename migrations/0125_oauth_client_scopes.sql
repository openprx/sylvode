-- OAuth scopes (round 2).
--
-- * The `ramflux` client may also request the write scopes and the forms scopes, whose API routes
--   are now reachable with a delegated token (forms:read, forms:write, work_items:write,
--   notifications:write). A grant that was consented before still holds only what its owner
--   ticked; this widens what a new consent can offer.
-- * `surfaces:embed` leaves the vocabulary: no route required it and nothing issued the tickets it
--   was meant for. Every stored list that could still name it is cleaned, so a stored scope never
--   fails to parse.
--
-- Re-runnable: every statement leaves the same rows behind when it runs again.

UPDATE oauth_clients
   SET allowed_scopes = ARRAY['projects:read', 'work_items:read', 'work_items:write', 'forms:read', 'forms:write',
                              'notifications:read', 'notifications:write', 'tasks:read', 'offline_access'],
       updated_at = now()
 WHERE client_id = 'ramflux';

UPDATE oauth_clients
   SET allowed_scopes = array_remove(allowed_scopes, 'surfaces:embed'), updated_at = now()
 WHERE 'surfaces:embed' = ANY(allowed_scopes);

UPDATE oauth_grants
   SET scopes = array_remove(scopes, 'surfaces:embed')
 WHERE 'surfaces:embed' = ANY(scopes);

UPDATE oauth_tokens
   SET scopes = array_remove(scopes, 'surfaces:embed')
 WHERE 'surfaces:embed' = ANY(scopes);

UPDATE oauth_authorization_requests
   SET requested_scopes = array_remove(requested_scopes, 'surfaces:embed')
 WHERE 'surfaces:embed' = ANY(requested_scopes);
