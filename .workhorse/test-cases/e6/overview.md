# Vulnerabilities page performance

## Search

- [x] A details-only hit still matches, and ranks below summary and package hits (verifies spec: VUL)
- [x] Details match every word of the search by its beginning, in any order (verifies spec: VUL)
- [x] Symbols in the search are not read as full-text query syntax
- [x] Typos are still tolerated in the identifier, summary and package names (verifies spec: VUL)

## Production check

- [ ] The vulnerabilities page loads in well under a second on production
- [ ] Moving between pages and changing a filter responds in well under a second on production
- [ ] A search returns in well under a second on production
