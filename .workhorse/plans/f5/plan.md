# Import an Ansible inventory file into an environment

## Notes

- **All-or-nothing against the secret store.** Imported secrets go to the secret store, which sits outside the database transaction. Applying writes each secret through `audit.commit_ahead(…)` before the transaction commits. If a later write or the commit fails, the import has to remove the secrets it already wrote, so a failed import leaves nothing in the store that no variable row points at.
- **Parse on the server.** The preview and the apply should run the same parsing, matching, and rule checks, so neither the SPA nor a stale preview can apply something the server would not have proposed. The SPA sends the file text with the operator's host assignments, skips, and secret marks, and the server re-derives everything else.
- **The old files are the test corpus.** Check the parser against the shapes found in the ops repo's existing inventories: nested `children` under `facility`, hosts keyed by `ip:port`, and structured values carrying a secret field (an IAM key pair). Keep client names out of fixtures.
