-- The page each operator has chosen to open at the root, keyed by their
-- lowercased tailnet login.
CREATE TABLE operator_homepages (
	login TEXT PRIMARY KEY,
	path TEXT NOT NULL
);
