# 98: The header fits a Root with a long path

**Blocked by:** None (can start immediately)

**Status:** migrated

**Migrated to:** #34

**What to build:** whatever the paths of the Watched Roots, the web UI's header shows every control it holds within
the window: the Root picker, the shown Root's state and reason, Manage Roots, the search field and the theme.

Today the Root picker is as wide as the longest Root path, because nothing in the header or the global `select` style
limits it. With a long path the header grows past the window, and the controls after the picker (the search field,
the theme picker, and since ticket 92 Manage Roots) are cut off at its right edge, out of reach. This was so before
ticket 92; ticket 92 only adds controls to the same line. Ticket 92 keeps the state reason readable with a minimum
width, but the overflow itself is not addressed there.

Found while checking ticket 92 in a browser, with a Root whose path is about 90 characters long, in a window
1100 px wide.

- [ ] With a Root path far wider than the window, every header control is visible and usable
- [ ] The picker still lets a person tell the Roots apart (for example the full path in a tooltip, or the end of the
      path kept rather than the start)
- [ ] With short paths the header looks as it does today
