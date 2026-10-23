# Ansible scope history

Each scope (a group, an environment, a machine) opens the history of every variable changed there, removed ones included.

## Scope history

- [x] A scope's history lists every set and removal of a variable at that scope, newest first, each naming its variable (verifies spec: ANSVAR)
- [x] A removal is in the history after the variable has been set again, when it is off the list of recent removals (verifies spec: ANSVAR)
- [x] A removal older than a week, and so off the list of recent removals, is in the history (verifies spec: ANSVAR)
- [x] Putting back a removal from the scope's history restores the value it removed (verifies spec: ANSVAR)
- [x] A group's history holds none of its environments' changes, and an environment's none of its group's (verifies spec: ANSVAR)
- [x] A machine's history holds its own variables' changes and none of its environment's (verifies spec: ANSVAR)
- [x] A change whose entry holds no value shows no value and offers nothing to put back, and is not shown as null (verifies spec: ANSVAR)
- [x] A removal of a variable whose value was null shows null and puts it back (verifies spec: ANSVAR)
- [ ] A secret removal in the scope's history can be put back while its value is held, and is not undoable once it is forgotten (verifies spec: ANSVAR)

## Recent removals

- [x] A group's recent removals are not crowded out by a busy environment's removals, nor the reverse (verifies spec: ANSVAR)
