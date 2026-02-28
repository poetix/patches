---
id: "0016"
title: Update documentation and fix broken links
priority: low
created: 2026-02-28
depends_on: []
epic: "E002"
---

## Summary

CLAUDE.md's workspace layout section lists only two crates but `patches-engine` now exists. The E001 epic's ticket table links to `tickets/open/` but all six tickets are in `tickets/closed/`. Fix both.

## Acceptance criteria

### CLAUDE.md workspace layout
- [ ] Workspace layout section updated to include `patches-engine` with a brief description of its role (builder, engine, CPAL integration, examples)

### E001 epic links
- [ ] Ticket links in `epics/open/E001-foundational-audio-engine.md` updated to point to `tickets/closed/` instead of `tickets/open/`
- [ ] Consider whether E001 should move to `epics/closed/` (all its tickets are closed, though some acceptance criteria reference a binary that doesn't exist as specified — it's an example, not a `src/bin/`)

### E001 epic status
- [ ] If E001's acceptance criteria are met by the `sine_tone` example, move the epic to `epics/closed/`
- [ ] If not, document what remains in a note

## Notes

This ticket has no code changes — documentation only. Can be done at any point in the epic.
