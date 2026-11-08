-- Which reader made a repository's stored read, so a repository is read again
-- when Canopy has learnt to read something it could not before, and not only
-- when its default branch moves.
--
-- Spec: VUL (security/vulnerabilities), "Where a package sits".

-- Null on a read made before this was recorded, which is not the current
-- reader, so every repository is read once more.
ALTER TABLE repository_reads ADD COLUMN read_by INTEGER;
