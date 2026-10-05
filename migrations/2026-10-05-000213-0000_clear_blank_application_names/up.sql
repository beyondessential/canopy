UPDATE applications SET name = NULL WHERE btrim(name) = '';
