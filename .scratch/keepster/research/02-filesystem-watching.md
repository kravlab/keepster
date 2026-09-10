# Reliability limits of cross-platform filesystem watching

Research for ticket `02-filesystem-watching`. Primary sources only: `inotify(7)`, Linux kernel source, Apple's
*File System Events Programming Guide*, Microsoft's `ReadDirectoryChangesW` / `FILE_NOTIFY_INFORMATION`
reference, the `notify` crate's own source and issue tracker. Local measurements were taken on the Fedora
machine this was written on (Linux 7.1.6, ext4/btrfs userspace, `inotifywait` 3.x).

**Headline conclusion.** No platform offers a lossless change stream. Every backend has a documented condition
under which it tells you *"events were lost, rescan"* — and at least two conditions under which it loses
information while telling you nothing at all. A watcher is therefore an *optimisation* that lowers the latency
of noticing a change; it is never the mechanism that guarantees a change was noticed. Reconciliation is the
mechanism. The daemon must be architected so that correctness rests on Reconciliation and only responsiveness
rests on the watcher.

---

## 1. Failure modes per platform

### 1.1 Linux — inotify

**Sources.** `inotify(7)` — <https://man7.org/linux/man-pages/man7/inotify.7.html>;
kernel `fs/notify/inotify/inotify_user.c` —
<https://github.com/torvalds/linux/blob/master/fs/notify/inotify/inotify_user.c>.

#### F-L1. Queue overflow (`IN_Q_OVERFLOW`) — *announced loss*

> "The value in this file is used when an application calls `inotify_init(2)` to set an upper limit on the
> number of events that can be queued to the corresponding inotify instance. Events in excess of this limit
> are dropped, but an `IN_Q_OVERFLOW` event is always generated." — `inotify(7)`, `/proc` interfaces

> "`IN_Q_OVERFLOW` — Event queue overflowed (`wd` is -1 for this event)." — `inotify(7)`, inotify events

The overflow event carries **no path and no watch descriptor** (`wd == -1`). It says *something, somewhere,
under this inotify instance was lost* and nothing more. The man page's own advice:

> "Note that the event queue can overflow. In this case, events are lost. Robust applications should handle
> the possibility of lost events gracefully. For example, it may be necessary to rebuild part or all of the
> application cache." — `inotify(7)`, Limitations and caveats

Local limit on this machine (`/proc/sys/fs/inotify/`):

| Knob | Value here |
| --- | --- |
| `max_queued_events` | `16384` |
| `max_user_instances` | `1024` |
| `max_user_watches` | `133264` |

16384 queued events is not a large budget for this workload: a `cargo build`, an `npm install`, a `git
checkout` of a large branch or an AI agent rewriting a hundred files each produce thousands of events in a
burst, and every event costs a queue slot. The limit is **per inotify instance**, so the queue is shared by
every Project watched through that instance.

#### F-L2. Watch exhaustion (`ENOSPC`) — *announced at watch-install time*

> "`max_user_watches`: This specifies an upper limit on the number of watches that can be created per real
> user ID." — `inotify(7)`

Watches are per-user, not per-process, so Keepster competes with the user's editor, language servers,
file managers and `cargo watch` for the same 133264. Exhaustion surfaces as `ENOSPC` from
`inotify_add_watch(2)`; `notify` documents it as "Linux: Bad File Descriptor / No space left on device"
(<https://docs.rs/notify/latest/notify/#linux-bad-file-descriptor--no-space-left-on-device>).

Kernel memory per watch is defined as:

```c
/*
 * An inotify watch requires allocating an inotify_inode_mark structure as
 * well as pinning the watched inode. Doubling the size of a VFS inode
 * should be more than enough to cover the additional filesystem inode
 * size increase.
 */
#define INOTIFY_WATCH_COST	(sizeof(struct inotify_inode_mark) + \
				 2 * sizeof(struct inode))
```
— `fs/notify/inotify/inotify_user.c`

`struct inode` is roughly 600 bytes on x86-64, so a watch costs on the order of **1.3 KB of unswappable
kernel memory**, and it *pins the inode*.

#### F-L3. Non-recursive by design, with a documented race on new subtrees — *silent loss*

> "Inotify monitoring of directories is not recursive: to monitor subdirectories under a directory,
> additional watches must be created. This can take a significant amount time for large directory trees."
>
> "If monitoring an entire directory subtree, and a new subdirectory is created in that tree or an existing
> directory is renamed into that tree, be aware that by the time you create a watch for the new subdirectory,
> new files (and subdirectories) may already exist inside the subdirectory. Therefore, you may want to scan
> the contents of the subdirectory immediately after adding the watch (and, if desired, recursively add
> watches for any subdirectories that it contains)." — `inotify(7)`, Limitations and caveats

This is not theoretical. Measured locally (`inotifywait -r -m` on a watched directory `proj/`, moving a
populated directory in from outside):

```
MOVED_TO,ISDIR /proj/staging          <- the only event
(no CREATE for staging/deep)
(no CREATE for staging/deep/pre.txt)
```

`staging/deep/pre.txt` existed, entered the Working Tree, and was never named by any event. Subsequent edits
to it *were* reported, because the watcher had by then installed watches — but its arrival was invisible. A
daemon that builds its idea of the Working Tree from events alone would have a file it does not know exists.

`notify` reproduces exactly this shape: on `IN_CREATE|IN_ISDIR` under a recursive watch it queues a watch for
the new directory (`add_watch_by_event`, `notify/src/inotify.rs`) and its `walk_dirs` walker yields
directories to watch, but it **emits no synthetic Create events for the entries already inside**. See also
the closed feature request "Make results of initial scan available"
(<https://github.com/notify-rs/notify/issues/478>).

#### F-L4. Rename matching is inherently racy — *silent misinterpretation*

> "These two events are usually consecutive in the event stream […] However, this is not guaranteed. If
> multiple processes are triggering events for monitored objects, then (on rare occasions) an arbitrary
> number of other events may appear between the `IN_MOVED_FROM` and `IN_MOVED_TO` events. Furthermore, it is
> not guaranteed that the event pair is atomically inserted into the queue […]
>
> Matching up the `IN_MOVED_FROM` and `IN_MOVED_TO` event pair generated by `rename(2)` is thus inherently
> racy. (Don't forget that if an object is renamed outside of a monitored directory, there may not even be an
> `IN_MOVED_TO` event.)" — `inotify(7)`, Dealing with rename() events

The man page also warns that a `IN_MOVED_FROM` may be the last event that fits in the current `read(2)`
buffer, with its partner only arriving on the next read.

#### F-L5. Hardlinks — *silent loss of a path*

> "Inotify monitoring is inode-based: when monitoring a file (but not when monitoring the directory
> containing a file), an event can be generated for activity on any link to the file (in the same or a
> different directory)." — `inotify(7)`

Because the practical way to watch a tree is to watch *directories*, the daemon does not get the inode-based
behaviour. Measured locally, with a directory watch on `proj/`:

```
CREATE /proj/h1.txt … CLOSE_WRITE /proj/h1.txt
CREATE /proj/h2.txt                       <- ln h1.txt h2.txt
OPEN /proj/h2.txt
MODIFY /proj/h2.txt                       <- echo appended >> h2.txt
CLOSE_WRITE,CLOSE /proj/h2.txt
```

`h1.txt`'s **content changed and no event ever names `h1.txt`**. Any snapshot logic that only re-reads paths
mentioned in events writes a stale `h1.txt` into the Snapshot. `notify` inherits this; on macOS the situation
is worse (see F-M5).

#### F-L6. Symlinks — *escape from the Project boundary*

`inotify(7)` offers `IN_DONT_FOLLOW` ("Don't dereference *pathname* if it is a symbolic link"). Two distinct
hazards:

- **Writes leaving the tree.** Measured locally: `ln -s /outside proj/link` produced only `CREATE /proj/link`;
  a later `echo x > proj/link/via-symlink.txt` produced **no events at all**, because the write landed outside
  the watched set. Conversely `inotify(7)` warns: "when monitoring a directory, events are not generated for
  the files inside the directory when the events are performed via a pathname (i.e., a link) that lies outside
  the monitored directory."
- **Watches leaving the tree.** `notify`'s `Config::follow_symlinks` defaults to `true`
  (`notify/src/config.rs`), and its recursive walker descends into symlinked directories
  (`walk_dirs`, `notify/src/inotify.rs`, guard `kind.is_dir() || (follow_links && kind.is_symlink())`). With
  the default, a symlink to `$HOME` inside a Project makes the daemon watch — and, if the snapshot walker
  agrees, capture — the whole of `$HOME`. `notify` does detect `dev`/`ino` ancestor loops and reports
  "symbolic link loop".

#### F-L7. Filesystems inotify cannot see — *silent, total loss*

> "Inotify reports only events that a user-space program triggers through the filesystem API. As a result, it
> does not catch remote events that occur on network filesystems. (Applications must fall back to polling the
> filesystem to catch such events.) Furthermore, various pseudo-filesystems such as `/proc`, `/sys`, and
> `/dev/pts` are not monitorable with inotify."
>
> "The inotify API does not report file accesses and modifications that may occur because of `mmap(2)`,
> `msync(2)`, and `munmap(2)`."
>
> "If a filesystem is mounted on top of a monitored directory, no event is generated, and no events are
> generated for objects immediately under the new mount point." — `inotify(7)`

mmap-based writers are not exotic: SQLite in WAL mode, some editors, and many build tools write through
mappings.

#### F-L8. Events are coalesced, so they cannot be counted

> "If successive output inotify events produced on the inotify file descriptor are identical (same `wd`,
> `mask`, `cookie`, and `name`), then they are coalesced into a single event if the older event has not yet
> been read […] but also means that an application can't use inotify to reliably count file events."
> — `inotify(7)`, Notes

Harmless for this workload (we want "changed at least once"), but it rules out any design that infers *how
much* changed from event volume.

#### F-L9. Cost of recursively watching a 100k-file tree — measured

`notify`'s README states that "for recursive watched folders each file and folder inside counts towards the
limit" (<https://docs.rs/notify/latest/notify/#linux-bad-file-descriptor--no-space-left-on-device>). Reading
the implementation, that is pessimistic: `walk_dirs` yields **directories only**, so the watch count equals
the directory count, not the file count. Measured on two real trees on this machine, watch counts read from
`/proc/<pid>/fdinfo/*`:

| Tree | Files | Directories | Symlinks | Watches installed | Time to install |
| --- | --- | --- | --- | --- | --- |
| `/usr/lib` | 24 766 | 3 935 | 3 241 | 3 932 | ~0.57 s |
| `/usr/share` | 116 167 | 9 945 | 38 508 | 9 944 | ~3.9 s |

So a realistic 100k-file source tree costs roughly **8–10k watches, ~13 MB of kernel memory, and 1–4 seconds
of wall time to arm** — on top of the `read_dir`/`stat` traffic of the walk itself (a warm-cache
`find /usr/share -type d` takes 0.11 s, so the cost is dominated by `inotify_add_watch(2)`, ~0.4 ms per
watch). Watcher process RSS stayed small (4.4 MB for 9944 watches); the cost is kernel-side and in startup
latency, not user-space memory.

Two consequences: arming a watch is *slow enough to matter* at daemon start and after every Reconciliation
that rebuilds watches, and a handful of large Projects can consume a meaningful fraction of a shared
per-user budget.

---

### 1.2 macOS — FSEvents

**Sources.** *File System Events Programming Guide* —
<https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/TechnologyOverview/TechnologyOverview.html>,
<https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html>,
<https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/FileSystemEventSecurity/FileSystemEventSecurity.html>.

#### F-M1. Coalescing is the normal mode of operation — *by-design imprecision*

> "In addition to collapsing notifications for changes to multiple files into a single, per-directory
> notification, the file system events daemon also coalesces multiple notifications on a given directory if
> they occur in a short period of time. You will always receive at least one notification after the last
> change is made. Beyond that, the temporal granularity can be as coarse or as fine as you desire; you choose
> the minimum time between events when you register for notification." — Technology Overview

The guarantee is precisely *"at least one notification after the last change"*. It is a level-triggered
signal about a location, not an edge-triggered log of operations.

#### F-M2. Directory-level granularity by default — *no path detail*

> "The important point to take away is that the granularity of notifications is at a directory level. It
> tells you *only* that something in the directory has changed, but does not tell you *what* changed."
> — Technology Overview

`kFSEventStreamCreateFlagFileEvents` (macOS 10.7+) raises this to per-file events, and `notify` sets it:

```rust
flags: fs::kFSEventStreamCreateFlagFileEvents
     | fs::kFSEventStreamCreateFlagNoDefer
     | fs::kFSEventStreamCreateFlagWatchRoot,
```
— `notify/src/fsevent.rs`

But file-level events are a best effort layered on a directory-level substrate: when the daemon coalesces, it
degrades back to a directory notification with `MustScanSubDirs` (F-M3). Directory granularity alone is **not
enough** for Keepster, because a Snapshot must know which file changed to avoid re-hashing the tree —
though see §3: knowing only the directory is *sufficient for correctness* if the response is a rescan of that
directory.

#### F-M3. `MustScanSubDirs` — *announced loss, the macOS equivalent of `IN_Q_OVERFLOW`*

> "If an event in a directory occurs at about the same time as one or more events in a subdirectory of that
> directory, the events may be coalesced into a single event. In this case, you will receive an event with
> the `kFSEventStreamEventFlagMustScanSubDirs` flag set. When you receive such an event, you must recursively
> rescan the path listed in the event."
>
> "If a communication error occurs between the kernel and the user-space daemon, you may receive an event
> with either the `kFSEventStreamEventFlagKernelDropped` or `kFSEventStreamEventFlagUserDropped` flag set. In
> either case, you must do a full scan of any directories that you are monitoring because there is no way to
> determine what may have changed."
>
> "When an event is dropped, the `kFSEventStreamEventFlagMustScanSubDirs` flag is also set. Thus, it is not
> necessary to explicitly check for the dropped event flags when determining whether to perform a full
> rescan of a path." — Using the FSEvents Framework

Unlike `IN_Q_OVERFLOW`, this one *carries a path*: the rescan can be scoped to the subtree named by the
event. `notify` maps it to `Flag::Rescan` and preserves the hint in `info`
(`"rescan: user dropped"` / `"rescan: kernel dropped"`) — `notify/src/fsevent.rs`.

#### F-M4. Root changed, event-ID wrap, mounts

> "If the root directory that you are watching is deleted, moved, or renamed (or if any of its parent
> directories are moved or renamed), the directory may cease to exist. If you care about this, you should
> pass the flag `kFSEventStreamCreateFlagWatchRoot` […] you will receive an event with the flag
> `kFSEventStreamEventFlagRootChanged` and an event ID of zero (0). In this case, you must rescan the entire
> directory because it may not exist."
>
> "If the number of events approaches 2^64, the event identifier will wrap around. When this happens, you
> will receive an event with the flag `kFSEventStreamEventFlagEventIdsWrapped`." — Using the FSEvents Framework

`RootChanged` matters directly for the map's *"identity is a surrogate id; the path is a mutable attribute"*
decision: moving a Project directory is a first-class, detectable event on macOS.

#### F-M5. Renames cannot be paired; hardlinks are not reported

FSEvents supplies `kFSEventStreamEventFlagItemRenamed` but no cookie: as `notify` puts it in
`fsevent.rs`, *"FSEvents provides no mechanism to associate the old and new sides of a rename event."* A
rename therefore arrives as two unrelated location notifications.

Hardlinks: `notify` issue #374, "Mac: `FSEvent` ignores hard links"
(<https://github.com/notify-rs/notify/issues/374>) — creating a hard link to a watched file produces no
callback at all. The flags `ItemIsHardlink` / `ItemIsLastHardlink` exist but do not restore per-link
notification.

#### F-M6. Permission filtering — *silent loss for files the daemon cannot read*

> "To prevent this potential security leak, users do not receive any events unless the user can reach the
> modified directory through standard file system permissions."
>
> "As a side effect, event IDs presented to a file system events client will not necessarily be consecutive
> even if the user is monitoring all events on all directories beginning at the root. Only applications
> running as the root user can be guaranteed to receive all events." — File System Event Security

`notify` documents the practical consequence and recommends `PollWatcher` as the workaround
(<https://docs.rs/notify/latest/notify/#macos-fsevents-and-unowned-files>).

#### F-M7. Historical replay exists — but the abstraction throws it away

FSEvents is the **only** one of the three platforms that can tell you what happened while you were not
running:

> "To work with persistent events, your application should regularly store the last event ID that it
> processes. Then, when it needs to go back and see what files have changed, it only needs to look at events
> that occurred after the last known event. To obtain all events since a particular event in the past, you
> pass the event ID in the `sinceWhen` argument to `FSEventStreamCreate`."
>
> "One of the most powerful features of file system events is their persistence across reboots. This means
> that your application can easily find out what happened since a particular time or a particular event in
> the distant past."

Apple nevertheless calls the replay advisory:

> "Because disks can be modified by computers running earlier versions of OS X (or potentially other
> operating systems), you should treat the events list as advisory rather than a definitive list of all
> changes to the volume […] backup software should still periodically perform a full sweep of any volume to
> ensure that no changes fall through the cracks." — Using the FSEvents Framework

And `notify` does not expose it at all — it hardcodes `since_when: fs::kFSEventStreamEventIdSinceNow`
(`notify/src/fsevent.rs`). So with `notify`, **replay is not available on any platform**, and macOS behaves
like the others: downtime is a black hole.

#### F-M8. APFS clone and `std::fs::copy` noise

> "On APFS, `std::fs::copy` may use copy-on-write cloning (`fclonefileat`/`clonefile`). This can update inode
> metadata on the source file, and FSEvents may report a metadata change for the source path."
> — <https://docs.rs/notify/latest/notify/> (issues
> <https://github.com/notify-rs/notify/issues/259>, <https://github.com/notify-rs/notify/issues/465>)

A false positive, not a loss — but a Trigger that fires on metadata-only events will take Snapshots that
contain no change.

---

### 1.3 Windows — `ReadDirectoryChangesW`

**Sources.** <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw>,
<https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-file_notify_information>.

#### F-W1. Buffer overflow — *announced loss, with zero detail*

> "When you first call **ReadDirectoryChangesW**, the system allocates a buffer to store change information.
> This buffer is associated with the directory handle until it is closed and its size does not change during
> its lifetime. Directory changes that occur between calls to this function are added to the buffer and then
> returned with the next call. If the buffer overflows, **ReadDirectoryChangesW** will still return **true**,
> but the entire contents of the buffer are discarded and the *lpBytesReturned* parameter will be zero, which
> indicates that your buffer was too small to hold all of the changes that occurred."
>
> "If the number of bytes transferred is zero, the buffer was either too large for the system to allocate or
> too small to provide detailed information on all the changes that occurred in the directory or subtree. In
> this case, you should compute the changes by enumerating the directory or subtree."
>
> "**ReadDirectoryChangesW** fails with **ERROR_NOTIFY_ENUM_DIR** when the system was unable to record all
> the changes to the directory. In this case, you should compute the changes by enumerating the directory or
> subtree."

Two crucial properties. First, the failure is **silent unless you look for it**: the call *succeeds*, and a
naive implementation parses a zero-length buffer as "no changes". Second, **the entire buffer is discarded** —
not just the overflowing tail. Changes that had already been recorded are lost too, so an overflow forfeits
an unbounded prefix of history for the whole watched subtree.

Also documented:

> "**ReadDirectoryChangesW** fails with **ERROR_INVALID_PARAMETER** when the buffer length is greater than
> 64 KB and the application is monitoring a directory over the network. This is due to a packet size
> limitation with the underlying file sharing protocols."

so the buffer cannot simply be made huge on network paths. `notify` uses a fixed `const BUF_SIZE: u32 =
16384;` per watch (`notify/src/windows.rs`). Each record is a 12-byte `FILE_NOTIFY_INFORMATION` header plus
the UTF-16 path *relative to the watched directory*, so a subtree with ~40-character relative paths fits on
the order of **175 events** before the buffer is discarded. An AI agent rewriting a `node_modules` or a
`git checkout` will exceed that.

#### F-W2. `notify` did not report the overflow until very recently — *silent loss in shipped versions*

Issue #963, "[Bug]: Windows `ReadDirectoryChangesW` overflow does not emit a rescan event" (reported against
`notify = "8.2.0"`) — <https://github.com/notify-rs/notify/issues/963>:

> "The Windows completion callback receives the number of transferred bytes as `_bytes_written`, but does not
> inspect it. `ERROR_SUCCESS` always falls through to the `FILE_NOTIFY_INFO…`"

Fixed by PR #968, "windows: Report rescans on `ReadDirectoryChangesW` buffer overflows"
(<https://github.com/notify-rs/notify/pull/968>). Current `main` reads:

```rust
// Windows reports discarded notification details as either a zero-byte successful completion or
// ERROR_NOTIFY_ENUM_DIR.
fn completion_rescan_event(error_code: u32, bytes_written: u32) -> Option<Event> {
    (error_code == ERROR_NOTIFY_ENUM_DIR || (error_code == ERROR_SUCCESS && bytes_written == 0))
        .then(|| Event::new(EventKind::Other).set_flag(Flag::Rescan))
}
```
— `notify/src/windows.rs`

**Requirement:** the daemon must depend on a `notify` version that contains this fix (9.x / current `main`),
and must have a test that asserts the rescan path, because this exact hole shipped for years.

#### F-W3. Rename shape and short names

`FILE_ACTION_RENAMED_OLD_NAME` (0x4) and `FILE_ACTION_RENAMED_NEW_NAME` (0x5) are separate records with no
correlation token; ordering within a buffer is the only link, and an overflow can split a pair.
`FILE_NOTIFY_INFORMATION` also warns:

> "If there is both a short and long name for the file, the function will return one of these names, but it
> is unspecified which one."

so an 8.3 short name (`PROGRA~1`) can appear where the daemon expects a long path. Paths must be canonicalised
before being matched against Include Rules.

#### F-W4. Write-through-cache latency

> "**FILE_NOTIFY_CHANGE_SIZE** […] The operating system detects a change in file size only when the file is
> written to the disk. For operating systems that use extensive caching, detection occurs only when the cache
> is sufficiently flushed." (same for `FILE_NOTIFY_CHANGE_LAST_WRITE`)

A change can be arbitrarily late, so a debounce window is not a bound on staleness.

#### F-W5. Symlinked subdirectories are not traversed by `bWatchSubtree`

`notify` issue #629, "[Windows] No events reported from symlinked subdirectories when watching recursively"
(open) — <https://github.com/notify-rs/notify/issues/629>. The reporter notes this does not occur on Linux.
So symlink behaviour is *inconsistent across platforms*: Linux `notify` follows them by default, Windows does
not.

#### F-W6. Deletion of the watch root is not reported

`notify` issue #403 (<https://github.com/notify-rs/notify/issues/403>): deleting the watched directory itself
produces an event on Linux but not on Windows. `notify` documents the workaround — "If you want to receive an
event for a deletion of folder `b` for the path `/a/b/..`, you will have to watch its parent `/a`."

---

### 1.4 The `notify` crate as the abstraction

**Sources.** <https://docs.rs/notify/latest/notify/>, <https://docs.rs/notify/latest/notify/event/enum.Flag.html>,
<https://github.com/notify-rs/notify>.

#### What it gives us

- One unified loss signal. `Flag::Rescan` is the single cross-platform escape hatch, documented as:

  > "Rescan notices are emitted by some platforms (and may also be emitted by Notify itself). They indicate
  > either a lapse in the events or a change in the filesystem such that events received so far can no longer
  > be relied on to represent the state of the filesystem now.
  >
  > An application that simply reacts to file changes may not care about this. An application that keeps an
  > in-memory representation of the filesystem will need to care, and will need to refresh that
  > representation directly from the filesystem."
  > — <https://docs.rs/notify/latest/notify/event/enum.Flag.html>

  Keepster is emphatically the second kind of application.

- It maps all three platform signals onto it: `EventMask::Q_OVERFLOW` →
  `Event::new(EventKind::Other).set_flag(Flag::Rescan)` (`inotify.rs`); `MUST_SCAN_SUBDIRS` → same, with
  `info` carrying `"rescan: user dropped"` / `"rescan: kernel dropped"` (`fsevent.rs`); zero-byte completion
  or `ERROR_NOTIFY_ENUM_DIR` → same (`windows.rs`).
- A `PollWatcher` fallback with `compare_contents`, for filesystems where native watching does not work at
  all (network mounts, `/proc`, containers).

#### Documented caveats that bite this design

| Caveat | Source |
| --- | --- |
| "Network mounted filesystems like NFS may not emit any events for notify to listen to. This applies especially to WSL programs watching windows paths." | <https://docs.rs/notify/latest/notify/#network-filesystems>, issue [#254](https://github.com/notify-rs/notify/issues/254) |
| "When watching a very large amount of files, notify may fail to receive all events. For example the linux backend is documented to not be a 100% reliable source." | <https://docs.rs/notify/latest/notify/#watching-large-directories>, issue [#412](https://github.com/notify-rs/notify/issues/412) |
| "If you rely on precise events (Write/Delete/Create..), you will notice that the actual events can differ a lot between file editors. Some truncate the file on save, some create a new one and replace the old one." | <https://docs.rs/notify/latest/notify/#editor-behaviour>, issues [#247](https://github.com/notify-rs/notify/issues/247), [#113](https://github.com/notify-rs/notify/issues/113) |
| "Some filesystems like `/proc` and `/sys` on *nix do not emit change events or use correct file change dates." | <https://docs.rs/notify/latest/notify/#pseudo-filesystems-like-proc-sys> |
| Docker on macOS M1 throws `Function not implemented (os error 38)` | issue [#423](https://github.com/notify-rs/notify/issues/423) |
| "Rescan flag is correctness hazard" — the flag is buried several levels deep in `Event::attrs`, so almost every caller that needs it fails to find it | issue [#434](https://github.com/notify-rs/notify/issues/434) (open) |
| macOS: watcher observed to stop delivering callbacks after the first batch (9.0.0-rc.4, CI) | issue [#937](https://github.com/notify-rs/notify/issues/937) (open) |

Issue #434 deserves emphasis: the maintainers' own tracker records that the loss signal is easy to miss and
that "almost every caller of `notify` needs to handle rescan events, to be correct." The daemon must make
`Event::need_rescan()` a first-class, tested input, not an afterthought.

`notify` has **no documented liveness guarantee**: nothing tells you the watcher has silently stopped
(#937). A watcher that dies quietly looks exactly like a directory nobody is editing.

---

### 1.5 How editors actually save — event shapes

`notify` states the problem plainly: "Some truncate the file on save, some create a new one and replace the
old one." Measured locally with `inotifywait -r -m` on a watched directory:

**Atomic save (temp file + `rename(2)`)** — the pattern used by Vim (`backupcopy=no`), most GUI editors,
`git`, and by Keepster's own Restore (per the map's decision that Restore writes "atomically (temp file
+ rename)"):

```
CREATE          /proj/.file.txt.tmp
OPEN            /proj/.file.txt.tmp
MODIFY          /proj/.file.txt.tmp
CLOSE_WRITE     /proj/.file.txt.tmp
MOVED_FROM      /proj/.file.txt.tmp
MOVED_TO        /proj/file.txt
```

Four facts follow. (a) **There is no `IN_MODIFY` on `file.txt` at all** — the new content arrives as a
*rename*. A Trigger keyed on `EventKind::Modify(Data)` for a known path misses every atomic save. (b) The
old inode is replaced with no event of its own, so any inode-keyed cache is now stale. (c) The temp file
(`.file.txt.tmp`, `file.txt~`, `4913`, `.#file.txt`, `file.txt.swp`) transiently appears in the Working Tree
and, unless Include Rules exclude it, gets captured in a Snapshot. (d) The write and the rename are separate
events separated by an arbitrary interval, which is exactly what a debounce window must span — debouncing on
the *directory* rather than the path is the safer unit.

**Truncate-in-place save** (Vim with `backupcopy=yes`, many CLI tools, `>` redirection):

```
OPEN            /proj/file.txt
MODIFY          /proj/file.txt
CLOSE_WRITE     /proj/file.txt
```

Here the file is momentarily **empty or partially written**. A Snapshot taken between `MODIFY` and
`CLOSE_WRITE` captures a truncated file — and Keepster exists precisely so the user can trust that what
it captured was real. `IN_CLOSE_WRITE` ("File opened for writing was closed") is the correct Linux settle
signal; macOS and Windows have no equivalent, which is why a debounce is mandatory everywhere, not an
optimisation.

Note that `notify` deliberately does **not** put `CLOSE_WRITE` in the mask for `MODIFY_DATA` — it is
classified as `Access(Close(Write))` and requires opting into access events
(`EventKindMask`, `notify/src/inotify.rs`).

**Move out of the tree:**

```
MOVED_FROM      /proj/file.txt      <- and nothing else
```

Indistinguishable from a delete without cross-referencing the cookie against a `MOVED_TO` that will never
come. `notify` documents the same for `EventKind::Remove`: "This may also be triggered for renames/moves that
move files *out of the watched subpath*."

**Move into the tree:** covered in F-L3 — only the top-level `MOVED_TO,ISDIR`; the contents are invisible.

---

## 2. Detection and recovery, condensed

| # | Failure mode | Platform | Detected by | Recovery required |
| --- | --- | --- | --- | --- |
| F-L1 | inotify queue overflow | Linux | `IN_Q_OVERFLOW` → `Flag::Rescan`, no path | Full Reconciliation of **every** Project on that watcher instance |
| F-L2 | `max_user_watches` exhausted | Linux | `ENOSPC` at `watch()` | Refuse to claim the Project is watched; fall back to periodic Reconciliation and tell the user |
| F-L3 | Subtree created/moved in | Linux (and all) | `Create`/`Modify(Name(To))` with `is_dir` | Scan that subtree immediately after arming its watch; treat every entry as new |
| F-L4 | Rename pair split or unmatched | Linux | Heuristic only; `MOVED_FROM` with no partner | Treat as delete + unknown; Reconcile the containing directories |
| F-L5 | Hardlinked sibling changed | all | **Not detectable from events** | Only Reconciliation, or resolving links during the snapshot walk |
| F-L6 | Write through a symlink out of the tree / watch escapes through a symlink into `$HOME` | Linux (follows), Windows (does not) | Not detectable | Set `follow_symlinks(false)`; decide the Project boundary explicitly |
| F-L7 | Network mount, pseudo-fs, `mmap` writer, overmount | all | **Not detectable** | `PollWatcher` / periodic Reconciliation; refuse or warn on unsupported filesystems |
| F-M3 | FSEvents coalesced or dropped | macOS | `MustScanSubDirs` (± `UserDropped`/`KernelDropped`) → `Flag::Rescan`, **with a path** | Recursive rescan of the path in the event |
| F-M4 | Watch root moved/deleted | macOS | `RootChanged` (needs `WatchRoot`) | Re-resolve the Project's path in the Registry; full Reconciliation |
| F-M6 | Unreadable-by-user files | macOS | Not detectable | Only Reconciliation (and it will fail to read them too — report, don't silently skip) |
| F-M7 | Downtime | all (replay discarded by `notify`) | Daemon start | Full Reconciliation at startup |
| F-W1 | `ReadDirectoryChangesW` buffer discarded | Windows | zero `bytes_written` **or** `ERROR_NOTIFY_ENUM_DIR` → `Flag::Rescan` | Enumerate the watched directory or subtree |
| F-W2 | `notify` < 9 swallowed F-W1 | Windows | Nothing — silent | Pin a fixed `notify`; regression-test the rescan path |
| F-W6 | Watch root deleted | Windows | Nothing | Watch the parent, or poll for the root's existence |
| — | Watcher silently stops | all | Nothing (#937) | Liveness check: periodic self-test or unconditional periodic Reconciliation |

**The pattern:** exactly two of these give a *scoped* recovery (`MustScanSubDirs` on macOS, per-watch
overflow on Windows). Everything else is either "rescan everything" or "you were never told".

---

## 3. Requirements the daemon design must satisfy

Written in the vocabulary of `CONTEXT.md` (Project, Working Tree, Snapshot, Include Rules, Trigger,
Reconciliation, Registry).

### R1 — Reconciliation is the correctness mechanism; the watcher is a latency optimisation

The map lists three Triggers (watcher-with-debounce, periodic floor Snapshot, full Reconciliation scan) and
says *"the user chooses which are active."* This research says one of them cannot be optional:
**Reconciliation must be non-disableable at least at daemon start and after every rescan signal.** The user
may tune its *frequency*, but a configuration in which the watcher is the only thing standing between the
user and data loss is not a configuration Keepster can honestly offer, because every platform documents
conditions under which the watcher loses changes without saying so (F-L5, F-L6, F-L7, F-M6, F-W6, #937).

Suggested phrasing for the spec: watcher-driven Snapshots are "best-effort, low-latency"; Reconciliation is
"the guarantee". The user-facing knob is *how stale history may get*, not *whether it may be wrong*.

### R2 — Startup after downtime of unknown length: always a full Reconciliation

No platform gives us replay through `notify` (F-M7: `since_when` is hardcoded to `SinceNow`). Therefore, for
every Project in the Registry, at daemon start:

1. Re-resolve the Project's path from the Registry (it may have moved while the daemon was down; identity is
   the surrogate id, so a moved directory keeps its history).
2. If the path no longer exists or is not a directory, mark the Project unavailable — do **not** interpret it
   as "every file deleted" and take a Snapshot recording that.
3. **Arm the watcher first, then walk.** Reversing this loses every change that happens during the walk — and
   the walk takes seconds on a 100k-file tree (F-L9). Events arriving during the walk are queued and applied
   after it.
4. Perform a full Reconciliation: walk the Working Tree under the Include Rules, compare against the latest
   Snapshot, and take a Snapshot if anything differs.
5. Record the completion of the Reconciliation durably, so a crash mid-walk is not mistaken for a completed
   one.

The startup Snapshot must be attributed honestly. It represents "the state found at startup", not "a change
that happened at this instant" — the delta may span weeks. If the metadata store records a cause per
Snapshot, `reconciliation` must be distinguishable from `watcher`.

### R3 — Treat `Event::need_rescan()` as a first-class input, and scope the rescan correctly

- Never drop it, never coalesce it away with ordinary events, never let it be filtered by Include Rules
  (a rescan event has no meaningful path on Linux and Windows).
- Scope: if the event carries a path (macOS `MustScanSubDirs`), Reconcile that subtree; **otherwise Reconcile
  every Project served by that watcher** — an `IN_Q_OVERFLOW` carries `wd == -1` and belongs to the whole
  inotify instance, and a Windows overflow discards the entire buffer for that watch.
- Rate-limit rescans, but never coalesce a rescan *away*: under sustained load the loss signal repeats, and
  the correct response is one Reconciliation after the storm, not zero.
- Regression-test each platform's rescan path (F-W2 shipped broken for years).

### R4 — Do not build a model of the Working Tree out of events

Events are hints about *locations that deserve a look*, never a description of what happened:

- Never trust a path in an event; `stat` it. "By the time an application processes an inotify event, the
  filename may already have been deleted or renamed" (`inotify(7)`).
- Never trust the event *kind*: an atomic save arrives as a rename, a truncating save as a modify, and
  editors differ (§1.5). Normalise both to "this path may have changed".
- Never infer "unchanged" from the absence of an event (F-L5 hardlinks, F-L7 mmap/network, F-M6 permissions).
- The unit of interest is a **directory**, not a path: it is the only granularity every platform agrees on,
  and it is what a rescan operates over.

### R5 — Rescan any directory that appears in the tree

On `Create`/`Modify(Name(To))` with `is_dir`, arm the watch **and then walk the new subtree**, treating every
entry as newly arrived. `inotify(7)` requires this explicitly, `notify` does not do it for us
(§F-L3, verified locally), and it is the single easiest way for a whole moved-in directory of the user's work
to be silently absent from history.

### R6 — Debounce, and settle on quiescence rather than on a fixed delay

The user's rate limit is a floor, not a substitute. A Snapshot must not be taken mid-write (§1.5, truncate
save). Practical rules:

- Debounce per directory, so the write→rename pair of an atomic save falls inside one window.
- Prefer the platform's settle signal when there is one (Linux `IN_CLOSE_WRITE`, which `notify` exposes only
  if access events are enabled), and fall back to a quiet period elsewhere.
- Do not treat the debounce window as a bound on staleness — Windows may report a change only after the cache
  is flushed (F-W4).
- Consider `notify-debouncer-full`, which the crate points at for exactly this, rather than rolling our own.

### R7 — Draw the Project boundary explicitly and stop symlinks from crossing it

- Set `Config::with_follow_symlinks(false)`. The default is `true` and it lets a recursive watch escape into
  `$HOME` (F-L6); Windows does not follow symlinks anyway (F-W5), so following them on Linux also makes the
  platforms disagree about what a Project contains.
- The snapshot walker and the watcher must apply the **same** boundary rule, or the Snapshot contains files
  no watcher is watching.
- Record symlinks as symlinks (link target as content), not as their targets.
- This interacts directly with the secrets denylist: a symlink is a trivially easy way for `~/.ssh` to end up
  inside a Project.

### R8 — Assume hardlinks are invisible

No platform reliably reports that link *A* changed when the write went through link *B* (F-L5 measured on
Linux; F-M5 / #374 on macOS). Options, to be decided:
(a) accept it and rely on Reconciliation; (b) during the snapshot walk, group entries by `(dev, ino)` so one
read covers all links. Option (b) is cheap because the walk already `stat`s everything, and it also removes
duplicated blob work. Whatever is chosen must be written down — silently capturing a stale copy of a
hardlinked file is exactly the failure Keepster exists to prevent.

### R9 — Detect and surface the conditions under which watching does not work

- `ENOSPC` from `watch()` (F-L2): the Project must be visibly degraded to Reconciliation-only, with a
  diagnostic naming `fs.inotify.max_user_watches`. Never fail silently, and never let one large Project
  exhaust the budget for the others.
- Network and removable filesystems (F-L7, and an open item in `map.md` §"Not yet specified"): detect the
  filesystem type at registration; use `PollWatcher` or Reconciliation-only there.
- Watcher liveness (#937): a watcher that stops delivering is indistinguishable from an idle directory unless
  the daemon checks. Either run an unconditional periodic Reconciliation, or probe (touch a file inside the
  History Store and confirm the event arrives) — the former is simpler and is required by R1 anyway.
- The Registry should record, per Project, *when* the last successful Reconciliation completed and whether
  the watcher is currently healthy. This is the answer to the map's open question "what the user sees when
  snapshots silently stop working".

### R10 — Budget for the cost of watching

Measured (F-L9): ~10k watches, ~13 MB kernel memory and 1–4 s of arming for a 100k-file tree, against a
per-*user* budget of 133264 watches on this machine. Consequences:

- Include Rules must be applied **before** installing watches, not only before hashing — never watch
  `node_modules`, `target`, `.venv`, `build`. This is the single largest lever on both watch count and event
  volume, and it also reduces the chance of hitting F-L1/F-W1.
- Registering many large Projects has a real cost; the daemon should report the watch count per Project.
- Watch arming is slow enough that it must not block the CLI or the HTTP surface; startup should report
  Projects as "arming" rather than appearing hung.

### R11 — Exclude our own churn

Restore writes with temp file + rename, and the History Store lives on disk. `inotify(7)`: "there is no easy
way for a process that is monitoring events via inotify to distinguish events that it triggers itself from
those that are triggered by other processes." FSEvents offers `kFSEventStreamEventFlagOwnEvent`, but only
there. The daemon must exclude the History Store by path and must expect to see its own Restore as a
write-plus-rename — which is correct and wanted (the Safety Snapshot precedes it), but must not cause a
feedback loop.

### R12 — Ignore transient editor artefacts, but only by name

Temp files (`.file.txt.tmp`, `file.txt~`, `4913`, `.#file`, `*.swp`) really do exist in the Working Tree
during a save (§1.5). Exclude them via Include Rules so they never enter a Snapshot — but do **not** try to
suppress the *events* they generate: the `MOVED_FROM`/`MOVED_TO` pair those events carry is precisely how the
daemon learns the real file changed.

---

## 4. Source index

**Linux**
- `inotify(7)` — <https://man7.org/linux/man-pages/man7/inotify.7.html> (also read locally via `man 7 inotify`)
- `fs/notify/inotify/inotify_user.c` — <https://github.com/torvalds/linux/blob/master/fs/notify/inotify/inotify_user.c>
- Local limits: `/proc/sys/fs/inotify/{max_queued_events,max_user_instances,max_user_watches}` = 16384 / 1024 / 133264

**macOS**
- File System Events Programming Guide — Technology Overview: <https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/TechnologyOverview/TechnologyOverview.html>
- Using the FSEvents Framework: <https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html>
- File System Event Security: <https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/FileSystemEventSecurity/FileSystemEventSecurity.html>

**Windows**
- `ReadDirectoryChangesW`: <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw>
- `FILE_NOTIFY_INFORMATION`: <https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-file_notify_information>
- Change journals (the documented alternative for whole-volume change tracking): <https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals>

**`notify` crate**
- Crate docs incl. "Known Problems": <https://docs.rs/notify/latest/notify/>
- `Flag::Rescan`: <https://docs.rs/notify/latest/notify/event/enum.Flag.html>
- `EventKind`: <https://docs.rs/notify/latest/notify/event/enum.EventKind.html>
- Source read: `notify/src/lib.rs`, `inotify.rs`, `fsevent.rs`, `windows.rs`, `config.rs` on `main` (9.0.0-rc.5) — <https://github.com/notify-rs/notify>
- Issues: [#113](https://github.com/notify-rs/notify/issues/113), [#247](https://github.com/notify-rs/notify/issues/247), [#254](https://github.com/notify-rs/notify/issues/254), [#374](https://github.com/notify-rs/notify/issues/374), [#403](https://github.com/notify-rs/notify/issues/403), [#412](https://github.com/notify-rs/notify/issues/412), [#423](https://github.com/notify-rs/notify/issues/423), [#434](https://github.com/notify-rs/notify/issues/434), [#478](https://github.com/notify-rs/notify/issues/478), [#629](https://github.com/notify-rs/notify/issues/629), [#937](https://github.com/notify-rs/notify/issues/937), [#963](https://github.com/notify-rs/notify/issues/963), [PR #968](https://github.com/notify-rs/notify/pull/968)

**Local measurements** (this machine, 2026-09-08): recursive `inotifywait` watch counts and timings on
`/usr/lib` and `/usr/share`; `inotifywait -r -m` traces of atomic save, truncating save, move out of tree,
populated directory moved into tree, hardlink creation and write, and symlink-to-outside.
