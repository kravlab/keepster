//! The live event stream (ticket 45): what happened, as it happens, and an honest `resync` when the
//! daemon cannot say what a client missed.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{self, KeepAlive, Sse};
use futures_util::Stream;
use keepster_api::{Event, RootState};
use keepster_app::config::{ConfigError, Resolved, Value};
use keepster_core::EventId;
use keepster_store::Registry;
use tokio::sync::broadcast;

use crate::DaemonState;

/// The header a reconnecting client names the last event it saw in — the one browsers send by themselves.
const LAST_EVENT_ID: &str = "last-event-id";

/// What happens in the daemon, as it happens.
#[utoipa::path(
    get,
    path = "/v1/events",
    operation_id = "stream_events",
    tag = "events",
    params(
        ("Last-Event-ID" = Option<String>, Header, description = "The identity of the last event this client saw. The events that followed it are replayed before live ones; when the daemon no longer holds it, the stream opens with `resync` instead."),
    ),
    responses(
        (status = 200, description = "A `text/event-stream` that stays open. Each event's `event:` field is its name, its `id:` a ULID, and its `data:` one JSON `Event` whose `event` property repeats the name.\n\nThe daemon keeps the last 1000 events, none older than an hour, for replay. A client that reconnects with `Last-Event-ID` gets every kept event after that one, in order, then live ones. When that identity is no longer kept — it was evicted, the daemon restarted, or it was never one of ours — or when the client reads too slowly to keep up, the daemon sends `resync`: what the client shows may be stale, and it refetches what it displays. A `resync` carries the newest identity still kept, when there is one, so the next reconnect resumes from there. A comment line is sent every 15 seconds while nothing happens, so a dead connection is noticed.\n\nA Root's state and its configuration are compared with what was announced last once a second, so `root.state_changed` and `config.*` follow the change within about a second; a Root's first state is the one `GET /v1/roots/{id}` shows after `root.registered`.\n\nEach `data:` is described by the `Event` schema in this document's components. It is not bound to the stream here: OpenAPI 3.1 can only describe a body as a whole.", content_type = "text/event-stream"),
    ),
)]
pub(crate) async fn stream(
    State(state): State<DaemonState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    let last = match headers.get(LAST_EVENT_ID).map(|value| value.to_str()) {
        // An empty identity is a client that saw nothing yet — a browser never sends one.
        None | Some(Ok("")) => None,
        Some(Ok(last)) => Some(last),
        // A header that is not text names no event we sent, which is what `resync` answers: it does not
        // parse as an identity.
        Some(Err(_)) => Some("\u{fffd}"),
    };
    let subscription = state.events.subscribe(last, Instant::now());

    let events = futures_util::stream::unfold(subscription, |subscription| async move {
        let mut subscription = subscription?;
        let next = subscription.next().await?;
        Some((Ok(wire(&next)), Some(subscription)))
    });

    Sse::new(events).keep_alive(KeepAlive::default())
}

/// What the stream last said about each Root, so that it announces what changed rather than repeating
/// what stayed.
///
/// A Root's state is computed whenever it is asked for, not stored, so a change is found by comparing:
/// every reload pass — once a second, and at once after the Registry changes — compares each Root with
/// what was announced last. A Root seen for the first time is announced by its registration, not here.
#[derive(Debug, Default)]
pub(crate) struct Announced {
    roots: HashMap<String, Seen>,
}

/// One Root as the stream last announced it.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    state: RootState,
    state_code: Option<String>,
    /// `None` for a Root whose record cannot be read, which has no configuration to speak of.
    config: Option<ConfigSeen>,
}

/// A configuration as far as an announcement cares: the values in force, without where they were set —
/// a comment added above a key moves its line, not its effect — and the files that are invalid.
#[derive(Debug, Clone, PartialEq)]
struct ConfigSeen {
    values: Option<Vec<(&'static str, Vec<Value>)>>,
    errors: Vec<ConfigError>,
}

impl ConfigSeen {
    fn of(resolved: &Resolved) -> Self {
        Self {
            values: resolved.effective.as_ref().map(|effective| {
                effective
                    .keys
                    .iter()
                    .map(|key| {
                        let values = key.setting.values().into_iter().cloned().collect();
                        (key.spec.name, values)
                    })
                    .collect()
            }),
            errors: resolved.errors.clone(),
        }
    }

    /// What changed from `before` to this, as the event that says so.
    fn change(&self, before: &Self, root: &str) -> Option<Event> {
        let root = root.to_owned();
        if self
            .errors
            .iter()
            .any(|error| !before.errors.contains(error))
        {
            Some(Event::ConfigInvalid { root })
        } else if self.values != before.values
            || (self.errors.is_empty() && !before.errors.is_empty())
        {
            Some(Event::ConfigReloaded { root })
        } else {
            None
        }
    }
}

/// Announce every Root whose configuration or state changed since the last pass.
///
/// Called with the Registry held, after every Root's configuration was resolved, so the order is the one
/// every holder keeps: the Registry, the configuration, the runtimes, then what was announced and the
/// event buffer.
pub(crate) fn announce_changes(state: &DaemonState, registry: &Registry) {
    // A Registry that cannot be read leaves nothing to compare; what was announced stands, and the next
    // pass that can read it announces whatever changed meanwhile.
    let (reported, records) = match (crate::roots::reported(state, registry), registry.readable()) {
        (Ok(reported), Ok(records)) => (reported, records),
        (Err(problem), _) => {
            tracing::warn!(problem = ?problem, "the Registry could not be read to announce changes");
            return;
        }
        (_, Err(error)) => {
            tracing::warn!(%error, "the Registry could not be read to announce changes");
            return;
        }
    };
    let configurations: HashMap<String, ConfigSeen> = records
        .into_iter()
        .map(|record| {
            let resolved = state.configuration(record.id, &record.path);
            (record.id.to_string(), ConfigSeen::of(&resolved))
        })
        .collect();
    let current: HashMap<String, Seen> = reported
        .iter()
        .cloned()
        .map(|root| {
            let config = configurations.get(&root.id).cloned();
            let seen = Seen {
                state: root.state,
                state_code: root.state_code,
                config,
            };
            (root.id, seen)
        })
        .collect();

    let mut announced = state
        .announced
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (root, now) in &current {
        let Some(before) = announced.roots.get(root) else {
            continue;
        };
        if let (Some(config), Some(was)) = (&now.config, &before.config) {
            if let Some(event) = config.change(was, root) {
                state.publish(event);
            }
        }
        if (now.state, &now.state_code) != (before.state, &before.state_code) {
            state.publish(Event::RootStateChanged {
                root: root.clone(),
                state: now.state,
                state_code: now.state_code.clone(),
            });
        }
    }
    announced.roots = current;
    drop(announced);
    state.alarms.observe_roots(&reported, Instant::now());
    crate::alarms::announce_due(state);
}

/// One event in the SSE framing.
fn wire(delivered: &Delivered) -> sse::Event {
    let framed = sse::Event::default()
        .event(delivered.event.name())
        // Serialising strings, flags and numbers cannot fail; were it ever to, the client would read an
        // empty object and refetch, which is what it does for an event it cannot use anyway.
        .data(serde_json::to_string(&*delivered.event).unwrap_or_else(|_| "{}".to_owned()));

    match delivered.id {
        Some(id) => framed.id(id.to_string()),
        None => framed,
    }
}

/// How much of the past a reconnecting client can be replayed: the last thousand events, none older than
/// an hour (ticket 07). Enough to cover a laptop lid closed over lunch on a busy Root; past it, a client
/// refetches, which is always correct and only slower.
pub(crate) const REPLAY_BOUND: Bound = Bound {
    events: 1_000,
    age: Duration::from_secs(3_600),
};

/// What the replay buffer keeps: whichever of the two runs out first evicts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bound {
    pub(crate) events: usize,
    pub(crate) age: Duration,
}

/// One event as a client receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delivered {
    /// What a client sends back as `Last-Event-ID`. A `resync` carries the newest identity the daemon
    /// still holds, if any, so the reconnect after it can be replayed from there.
    pub(crate) id: Option<EventId>,
    pub(crate) event: Arc<Event>,
}

/// An event the daemon kept for replay.
#[derive(Debug, Clone)]
struct Kept {
    id: EventId,
    at: Instant,
    event: Arc<Event>,
}

/// Every event, to every open stream, with the recent past kept for those that reconnect.
#[derive(Debug)]
pub(crate) struct EventBus {
    bound: Bound,
    inner: Mutex<Inner>,
}

/// What the bus's one lock guards.
#[derive(Debug)]
struct Inner {
    /// The recent past, oldest first, within the bound.
    kept: VecDeque<Kept>,
    /// `None` once the daemon is stopping: every open stream ends, so that a stop is not held up by a
    /// client that would listen forever.
    live: Option<broadcast::Sender<Kept>>,
}

impl EventBus {
    pub(crate) fn new(bound: Bound) -> Self {
        // A stream that falls further behind than the buffer holds has lost what a reconnect would lose
        // too, so the channel is as deep as the buffer.
        let (live, _) = broadcast::channel(bound.events.max(1));

        Self {
            bound,
            inner: Mutex::new(Inner {
                kept: VecDeque::new(),
                live: Some(live),
            }),
        }
    }

    /// Send `event` to every open stream and keep it for the ones that reconnect.
    pub(crate) fn publish(&self, id: EventId, at: Instant, event: Event) {
        let kept = Kept {
            id,
            at,
            event: Arc::new(event),
        };
        let mut inner = self.inner();
        let Some(live) = &inner.live else { return };
        // No stream open is not a failure: the event is kept all the same.
        let _ = live.send(kept.clone());
        inner.kept.push_back(kept);
        self.evict(&mut inner, at);
    }

    /// Send `event` to every open stream now, under an identity minted from the wall clock.
    pub(crate) fn publish_now(&self, event: Event) {
        self.publish(
            EventId::generate(crate::roots::now()),
            Instant::now(),
            event,
        );
    }

    /// A stream that starts after `last`, the identity the client saw last — or with what happens from
    /// now on when it saw none. `None` once the daemon is stopping.
    pub(crate) fn subscribe(&self, last: Option<&str>, at: Instant) -> Option<Subscription> {
        let mut inner = self.inner();
        self.evict(&mut inner, at);
        // Subscribed under the same lock `publish` sends under, so nothing falls between the replay and
        // the live events, and nothing is in both.
        let receiver = inner.live.as_ref()?.subscribe();
        let opening = match last {
            None => Vec::new(),
            Some(last) => {
                let found = last
                    .parse::<EventId>()
                    .ok()
                    .and_then(|last| inner.kept.iter().position(|kept| kept.id == last));
                match found {
                    Some(position) => inner
                        .kept
                        .iter()
                        .skip(position + 1)
                        .map(delivered)
                        .collect(),
                    None => vec![Delivered {
                        id: inner.kept.back().map(|kept| kept.id),
                        event: Arc::new(Event::Resync),
                    }],
                }
            }
        };

        Some(Subscription {
            opening: opening.into(),
            receiver,
        })
    }

    /// End every open stream and accept no new one: the daemon is stopping.
    pub(crate) fn close(&self) {
        self.inner().live = None;
    }

    fn evict(&self, inner: &mut Inner, now: Instant) {
        while inner.kept.len() > self.bound.events
            || inner
                .kept
                .front()
                .is_some_and(|oldest| now.saturating_duration_since(oldest.at) > self.bound.age)
        {
            inner.kept.pop_front();
        }
    }

    /// The buffer, held for the length of one publish or subscribe. A leaf in the lock order: nothing
    /// else is taken while it is held.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A kept event as a stream hands it out.
fn delivered(kept: &Kept) -> Delivered {
    Delivered {
        id: Some(kept.id),
        event: Arc::clone(&kept.event),
    }
}

/// One client's stream: the replay or the `resync` it opened with, then every event as it happens.
#[derive(Debug)]
pub(crate) struct Subscription {
    opening: VecDeque<Delivered>,
    receiver: broadcast::Receiver<Kept>,
}

impl Subscription {
    /// The next event, waiting for one; `None` once the daemon is stopping.
    pub(crate) async fn next(&mut self) -> Option<Delivered> {
        if let Some(opening) = self.opening.pop_front() {
            return Some(opening);
        }

        match self.receiver.recv().await {
            Ok(kept) => Some(delivered(&kept)),
            // Events were dropped before this stream read them, so what it shows is stale; saying so is
            // the same honesty a reconnect past the buffer gets.
            Err(broadcast::error::RecvError::Lagged(_)) => Some(Delivered {
                id: None,
                event: Arc::new(Event::Resync),
            }),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime};

    use keepster_api::Event;
    use keepster_core::EventId;

    use super::{Bound, Delivered, EventBus, Subscription};

    /// A fixed moment to mint identities from: their random part keeps them apart.
    const MOMENT: SystemTime = SystemTime::UNIX_EPOCH;

    const ROOMY: Bound = Bound {
        events: 100,
        age: Duration::from_secs(3_600),
    };

    fn moved(root: &str) -> Event {
        Event::RootMoved {
            root: root.to_owned(),
        }
    }

    /// Publish one event per name at `at`, returning their identities in order.
    fn publish(bus: &EventBus, at: Instant, names: &[&str]) -> Vec<EventId> {
        names
            .iter()
            .map(|name| {
                let id = EventId::generate(MOMENT);
                bus.publish(id, at, moved(name));
                id
            })
            .collect()
    }

    /// Whatever the stream has ready now, without waiting for more.
    async fn ready(subscription: &mut Subscription) -> Vec<Delivered> {
        let mut found = Vec::new();
        while let Ok(Some(next)) =
            tokio::time::timeout(Duration::from_millis(20), subscription.next()).await
        {
            found.push(next);
        }
        found
    }

    fn names(delivered: &[Delivered]) -> Vec<String> {
        delivered
            .iter()
            .map(|delivered| match &*delivered.event {
                Event::RootMoved { root } => root.clone(),
                other => other.name().to_owned(),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_reconnect_with_a_kept_identity_replays_what_followed_it() {
        let bus = EventBus::new(ROOMY);
        let now = Instant::now();
        let ids = publish(&bus, now, &["a", "b", "c"]);

        let mut stream = bus.subscribe(Some(&ids[0].to_string()), now).expect("open");

        let replayed = ready(&mut stream).await;
        assert_eq!(names(&replayed), ["b", "c"]);
        assert_eq!(replayed[1].id, Some(ids[2]));
    }

    #[tokio::test]
    async fn a_stream_without_an_identity_starts_with_what_happens_next() {
        let bus = EventBus::new(ROOMY);
        let now = Instant::now();
        publish(&bus, now, &["before"]);

        let mut stream = bus.subscribe(None, now).expect("open");
        publish(&bus, now, &["after"]);

        assert_eq!(names(&ready(&mut stream).await), ["after"]);
    }

    #[tokio::test]
    async fn an_identity_the_buffer_no_longer_holds_is_answered_with_resync_then_live_events() {
        let bus = EventBus::new(Bound { events: 2, ..ROOMY });
        let now = Instant::now();
        let ids = publish(&bus, now, &["a", "b", "c"]);

        let mut evicted = bus.subscribe(Some(&ids[0].to_string()), now).expect("open");
        publish(&bus, now, &["d"]);

        let delivered = ready(&mut evicted).await;
        assert_eq!(names(&delivered), ["resync", "d"]);
        assert_eq!(
            delivered[0].id,
            Some(ids[2]),
            "a reconnect after the resync replays from the newest event kept"
        );
        let resumed = delivered[0].id.expect("an identity").to_string();
        let mut kept = bus.subscribe(Some(&resumed), now).expect("open");
        assert_eq!(names(&ready(&mut kept).await), ["d"]);
    }

    #[tokio::test]
    async fn events_older_than_the_bound_are_not_replayed() {
        let bus = EventBus::new(Bound {
            age: Duration::from_secs(3_600),
            ..ROOMY
        });
        let start = Instant::now();
        let old = publish(&bus, start, &["old"]);
        let recent = publish(&bus, start + Duration::from_secs(1_800), &["recent"]);
        let later = start + Duration::from_secs(3_660);

        let mut expired = bus
            .subscribe(Some(&old[0].to_string()), later)
            .expect("open");
        assert_eq!(names(&ready(&mut expired).await), ["resync"]);
        let mut current = bus
            .subscribe(Some(&recent[0].to_string()), later)
            .expect("open");
        assert_eq!(names(&ready(&mut current).await), Vec::<String>::new());
    }

    #[tokio::test]
    async fn an_identity_from_another_daemon_or_none_at_all_is_answered_with_resync() {
        let bus = EventBus::new(ROOMY);
        let now = Instant::now();

        for last in [EventId::generate(MOMENT).to_string(), "garbage".to_owned()] {
            let mut stream = bus.subscribe(Some(&last), now).expect("open");
            let delivered = ready(&mut stream).await;
            assert_eq!(names(&delivered), ["resync"]);
            assert_eq!(delivered[0].id, None, "nothing is kept to resume from");
        }
    }

    #[tokio::test]
    async fn a_stream_that_falls_behind_is_told_to_resync() {
        let bus = EventBus::new(Bound { events: 2, ..ROOMY });
        let now = Instant::now();
        let mut slow = bus.subscribe(None, now).expect("open");

        publish(&bus, now, &["a", "b", "c", "d"]);

        assert_eq!(names(&ready(&mut slow).await), ["resync", "c", "d"]);
    }

    #[tokio::test]
    async fn closing_ends_every_stream_and_opens_no_new_one() {
        let bus = EventBus::new(ROOMY);
        let now = Instant::now();
        let mut open = bus.subscribe(None, now).expect("open");

        bus.close();

        assert_eq!(open.next().await, None);
        assert!(bus.subscribe(None, now).is_none());
    }
}
