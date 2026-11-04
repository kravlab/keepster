# An Update is done by the CLI alone

`keepster update` downloads a release, checks it, puts its binaries in place of the running `keepster` and
`keepster-tray`, and restarts the daemon and the tray, all in the CLI process and as the person who ran it. The
daemon never fetches a release and offers no endpoint that starts an Update, so neither the tray nor the web UI can
start one. A reader who finds no "Update" in the tray menu will reasonably ask why.

## Considered Options

- **The daemon runs the Update as a Job, on request from any client.** Rejected: the daemon would gain outbound
  network access it has never needed, and any client of the Loopback Listener would be able to make the daemon
  replace its own binaries. Both widen what a compromised browser session or local client can reach, against the
  Loopback Listener's role as the daemon's one, inbound, network surface. A daemon replacing the file it runs from
  is also the hardest variant to get right on Windows.
- **The tray offers an item that runs `keepster update`.** Rejected for now: the tray would have to survive being
  replaced by the process it started. Whether the tray advertises a newer release belongs to checking for updates
  (kravlab/keepster#38), which is separate from installing one.
- **`keepster update` downloads and runs the install script.** Rejected: the CLI would execute downloaded code, and
  the replacement logic would live in two shell dialects instead of one binary.

## Consequences

- **The download is checked against the release's `SHA256SUMS`, exactly as the install scripts do.** This guards
  against a damaged download, not against a substituted release; signing would have to come to both paths at once,
  so that an Update is never weaker or stronger than a first install.
- **The binaries replaced are those beside the running `keepster`**, whatever directory that is.
- **The set of running parts is kept.** The daemon is restarted if its autostart is installed; the tray only if its
  autostart entry exists or it is running now. Configuration is not touched.
- **The running tray is ended through its lock file.** The CLI reads the pid from `tray.pid`, ends that process,
  waits for the lock to be released and starts the new tray. `keepster install` does the same, so re-running the
  install script, or installing by hand, also leaves the new tray running.
- **Files are replaced before anything restarts**, with the old ones moved aside, on every platform, so the capture
  gap is the restart alone. The copies moved aside are the rollback: if the daemon does not answer `/v1/version`
  with the new release in time, they are put back and the old daemon and tray restarted.
- **A rollback can leave Roots Unavailable.** If the new daemon started far enough to rewrite Registry records to a
  newer container version (ADR 0010), the old daemon finds them too new (`store_too_new`) until a later Update
  succeeds.
- **Going back to an older release is an Update made only on the person's explicit word**, with a warning that a
  newer store or Segment format stays unreadable to it (ADR 0010).
- **Installs made by a package manager are not handled here.** How an Update recognises one and refuses, naming the
  manager's own command, belongs to the packages' own work (kravlab/keepster#22).
