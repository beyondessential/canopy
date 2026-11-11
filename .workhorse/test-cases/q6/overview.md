# Render markdown from memory entries and notes

Notes on records, issues and incidents, and incident memory entry bodies, show as rendered markdown with single line breaks kept and unsafe inline HTML dropped (verifies spec: FLT, INC, MEM).

## Rendered as markdown

- [x] A group's notes render emphasis and links (verifies spec: FLT)
- [x] An earlier revision in a group's notes history renders as markdown (verifies spec: FLT)
- [x] A machine's notes render as markdown (verifies spec: FLT)
- [x] An application's notes, and its group's notes on the application page, render as markdown (verifies spec: FLT)
- [x] An incident note renders as markdown (verifies spec: INC)
- [x] An issue note renders as markdown (verifies spec: FLT)
- [x] A memory entry's body renders as markdown when the entry is opened (verifies spec: MEM)

## Plain text written before markdown

- [x] A note written as separate lines keeps each line break (verifies spec: FLT)
- [x] An inline HTML event handler in a note does not run (verifies spec: FLT)

## Feel

- [x] Rendered notes sit flush in their card, with no extra space above the first line or below the last
- [x] A long URL or unbroken string in a note wraps instead of overflowing its card
