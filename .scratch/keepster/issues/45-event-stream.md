# 45: The live event stream

**Blocked by:** 43 (Triggers — the policy kernel, the watcher, debounce and the rate limit)

**Status:** ready-for-agent

**What to build:** an open client — the web UI later, `keepster log --follow` now — sees Snapshots and state changes
as they happen, and is told honestly when it missed some rather than shown a stale screen.

Server-sent events over the same HTTP surface, so cookies, the host and origin checks and the error model apply
unchanged and the stream works identically over the socket. Event ids are ULIDs; a reconnecting client sends the last
id it saw and the daemon replays from a bounded ring buffer. When the requested id has been evicted the daemon
neither stays silent nor pretends: it emits a resynchronise event and the client refetches.

- [ ] The stream delivers the event set for the behaviour that exists, with thin payloads — ids and flags, never
      content
- [ ] Reconnecting with the last seen id replays the events that followed it
- [ ] Reconnecting with an evicted id yields a resynchronise event instead of silence or a gap
- [ ] The ring buffer is bounded and documented, and the endpoint is described in the OpenAPI document
- [ ] `keepster log --follow` consumes the very same endpoint over the socket
- [ ] Server-side replay is tested through the harness; nothing about client-side reconnection is asserted here
