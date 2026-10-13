//! The tray process (ticket 67): an API client living in the user's session, which shows at a glance
//! whether the insurance holds and notifies when it stops.
//!
//! It is governed by one invariant (ADR 0008) — **a view, never a source**: everything it shows comes
//! from the status payload and the event stream, every notification answers an alarm the daemon raised,
//! and every menu item is something the API or the service manager already does. The one thing it
//! decides itself is that the daemon died, which a dead daemon cannot say.
//!
//! [`model`] is the whole of that logic, without a UI; the platform adapters only draw what it says.

pub mod log;
pub mod menu;
pub mod model;
pub mod platform;
pub mod run;
pub mod strings;
