CREATE TABLE admins (
	email TEXT PRIMARY KEY,
	created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

INSERT INTO admins (email, created_at)
SELECT login, created_at
FROM access_list_entries
WHERE level = 'admin';

DROP TABLE access_list_entries;
DROP TYPE access_level;
