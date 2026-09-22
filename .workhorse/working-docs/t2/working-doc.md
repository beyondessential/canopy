---
status: draft
---

# Gate the administrative surface behind safety modes

An operator's session runs in read-only, write, or danger mode; the mode gates every handler on the private server's administrative surface, is visible with its remaining time, and returns to read-only on its own.
