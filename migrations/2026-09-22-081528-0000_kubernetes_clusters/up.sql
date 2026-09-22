-- A Kubernetes cluster canopy monitors through a relay running inside it (spec
-- K8S, "Cluster registry"). A registered cluster is nothing more than a relay
-- identity and a name: canopy holds no connection credential for a cluster, so
-- there is nothing here to encrypt, rotate, or persist beyond what identifies
-- the relay.
--
-- Named `kubernetes_clusters` rather than `clusters`, which would be far too
-- generic beside server groups, backup repos, and the CNPG clusters inside the
-- namespaces this feature reads.
CREATE TABLE kubernetes_clusters (
	id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
	-- What an operator sees in the host picker.
	name TEXT NOT NULL,
	-- The relay's identity. One relay per cluster, so this is unique. The row
	-- is what accounts for the minted identity, so deleting the identity takes
	-- the cluster with it.
	relay_identity_id UUID NOT NULL UNIQUE REFERENCES devices (id) ON DELETE CASCADE,
	-- Null while the registration is a draft; set the moment the relay first
	-- connects and answers, which is what turns a draft into a registered
	-- cluster. Only a registered cluster hosts applications, is offered as a
	-- host, or carries checks.
	registered_at TIMESTAMPTZ,
	-- When canopy last had a Ping answered by this cluster's relay, written by
	-- the relay hub. Read by registration to confirm the relay is answering,
	-- and shown to an operator. Never cleared on disconnect: it is the "when
	-- did we last hear from this" an operator wants when a cluster goes quiet.
	last_answered_at TIMESTAMPTZ,
	created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
	updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT diesel_manage_updated_at('kubernetes_clusters');

