-- The levels a list can confer. The all level has no list: every tailnet user
-- holds it.
CREATE TYPE access_level AS ENUM ('admin', 'pm');

-- Each level's list of logins, held lowercased so a list compares ignoring
-- case as the tailnet policy does.
CREATE TABLE access_list_entries (
	level access_level NOT NULL,
	login TEXT NOT NULL CHECK (login <> ''),
	created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
	PRIMARY KEY (level, login)
);

-- Every administrator stays one, under the earliest of the spellings that
-- lowercase alike.
INSERT INTO access_list_entries (level, login, created_at)
SELECT 'admin', lower(btrim(email)), min(created_at)
FROM admins
WHERE btrim(email) <> ''
GROUP BY lower(btrim(email));

DROP TABLE admins;
