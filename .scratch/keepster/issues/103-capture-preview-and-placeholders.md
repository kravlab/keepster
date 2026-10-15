# 103: Capture Preview cannot tell a carried-forward placeholder from a Withheld one

**Blocked by:** 75 (Cloud placeholders are never hydrated)

**Status:** migrated

**Migrated to:** #39

**What to build:** a Capture Preview of a folder synced by OneDrive, iCloud Drive or a similar client reports each
cloud placeholder as the next capture would treat it: kept at the version already held, or Withheld
`cloud_placeholder`.

Ticket 20 promised that a preview reports the same Withheld set a real capture would, `cloud_placeholder` included,
"from the file attributes ticket 10 already reads". Ticket 75 made the capture's answer depend on more than the
attributes: a placeholder is carried forward when its size equals the held blob's and its mtime equals the `p`
record or the stat cache's evidence, and Withheld otherwise. The preview walk (`crates/keepster-store/src/history/
preview.rs`) opens no History Store, so it sees neither. Today it reports a placeholder as an ordinary file to be
captured, and never asks whether it is one. It also reads each directory's `.gitignore` with no such question, so
an ignore file that is a placeholder is downloaded; the capture never reads one, and carries its directory forward
instead.

Found while building ticket 75.

- [ ] A preview recognises a placeholder from the same metadata the capture reads, and never opens it — an
      ignore file included
- [ ] What it reports for one is decided: the capture's answer, which needs the baseline and the evidence, or a
      class of its own saying the answer depends on the version held
- [ ] Whatever is decided, the preview and the capture never disagree in silence
