# 94: A Capture Preview before registering

**Blocked by:** 93 (Registering a directory from the web UI)

**Status:** ready-for-agent

**What to build:** registering from the web UI checks what watching the directory would capture first, exactly as
`keepster watch` does. Part of spec 91 (user stories 14–21).

Before `POST /v1/roots` the Root management module runs `GET /v1/preview` for the chosen path at its default depth,
which sends the summary alone, and shows that the walk is alive from its progress records. When the summary's
`past_ceiling` is empty, registering goes on without a question. When it is not, the summary is shown and the
developer is asked whether to watch the directory anyway; declining registers nothing. A preview that fails other
than as busy lets registering go on, as the CLI does; a busy preview (`409`, `preview_busy`) stops the add and says to
try again. Cancelling while the walk runs closes the stream, which stops it.

- [ ] A directory within the claim is registered with no question
- [ ] A directory past the claim shows the summary and asks; yes registers, no registers nothing
- [ ] A failed preview still registers; a busy one stops with "try again"
- [ ] Progress is visible while the walk runs, and cancelling stops it
- [ ] Every new text is a key in the strings module
- [ ] Tests of the add flow at the `fetch` seam, the preview answered as an NDJSON stream
- [ ] `ui/dist` is rebuilt and committed, and the size budgets hold
