# 73: A configuration file caught half-saved never becomes the known-good

**Blocked by:** none

**Status:** needs-triage

**What to build:** a configuration edit saved by truncating the file and writing it again — which some tools
do instead of an atomic rename — is never mistaken, in the instant between the two, for an empty and therefore
valid configuration.

The daemon re-reads both configuration levels on every request and once a second (ticket 42), and an empty file
parses as the defaults. Read in that instant, the empty file becomes the configuration in force and the
known-good; the full file replaces it a second later. If the full file is then invalid, the last known-good is
the empty one, and the Root runs on the defaults — the outcome ticket 04 forbids. Found while making ticket
44's tests deterministic (they now save atomically); editors save atomically and do not trigger it.

- [ ] A file that changed within the last moment is not adopted until it has stayed the same for a short
      settle interval, or an equivalent guard keeps a transient empty read from becoming the known-good
- [ ] An invalid edit following a truncate-and-rewrite save still leaves the previous real configuration in
      force
