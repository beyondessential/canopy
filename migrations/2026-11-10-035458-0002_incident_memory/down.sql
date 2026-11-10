DELETE FROM mcp_tokens WHERE 'notes' = ANY (grants);
ALTER TABLE mcp_tokens
	DROP CONSTRAINT mcp_tokens_grants_known,
	ADD CONSTRAINT mcp_tokens_grants_known CHECK (grants <@ ARRAY['triage']);

DROP TABLE memory_entry_revisions;
DROP TABLE memory_entries;
