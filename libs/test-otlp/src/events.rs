//! Capture the `tracing` events a piece of code emits, for asserting on what it logged.

use std::fmt::Debug;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::Field;
use tracing::subscriber::with_default;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

/// One captured event: its level and its formatted message.
pub type CapturedEvent = (Level, String);

/// Run `body` with a subscriber that records every event it emits on this thread, and return them in order.
///
/// The subscriber is the thread's default only for the call, so events from other threads, or from code `body`
/// hands to another thread, are not captured.
pub fn capture_events<F>(body: F) -> Vec<CapturedEvent>
where
    F: FnOnce(),
{
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = Registry::default().with(Recorder(Arc::clone(&events)));
    with_default(subscriber, body);
    let captured = events.lock().unwrap_or_else(PoisonError::into_inner);
    captured.clone()
}

/// The layer that appends each event's level and `message` field to the shared list.
struct Recorder(Arc<Mutex<Vec<CapturedEvent>>>);

impl<S: Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut message = String::default();
        event.record(&mut |field: &Field, value: &dyn Debug| {
            if field.name() == "message" {
                message = format!("{value:?}");
            }
        });
        let level = *event.metadata().level();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((level, message));
    }
}
