//! In-process domain event bus.
//!
//! A synchronous pub/sub seam for domain events: publishers push a value,
//! every live subscriber receives its own clone via a dedicated
//! `std::sync::mpsc` channel. The async broadcast bus arrives with the
//! Phase 3 service runtime (`docs/02_System_Architecture.md` §6) — this is
//! the synchronous seam services will use in the meantime.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

/// A synchronous, thread-safe, in-process pub/sub bus.
///
/// Each [`subscribe`](EventBus::subscribe) call gets its own channel;
/// [`publish`](EventBus::publish) clones the event once per subscriber and
/// prunes subscribers whose [`Subscriber`] has been dropped.
#[derive(Debug)]
pub struct EventBus<T: Clone + Send> {
    senders: Mutex<Vec<Sender<T>>>,
}

impl<T: Clone + Send> EventBus<T> {
    /// Creates a bus with no subscribers.
    pub fn new() -> Self {
        EventBus {
            senders: Mutex::new(Vec::new()),
        }
    }

    /// Registers a new subscriber with its own private channel.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic while
    /// publishing or subscribing).
    pub fn subscribe(&self) -> Subscriber<T> {
        let (tx, rx) = channel();
        self.senders
            .lock()
            .expect("event bus lock poisoned")
            .push(tx);
        Subscriber { receiver: rx }
    }

    /// Delivers a clone of `event` to every live subscriber, pruning
    /// disconnected ones. Returns the number of live subscribers after
    /// pruning.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned (a prior panic while
    /// publishing or subscribing).
    pub fn publish(&self, event: &T) -> usize {
        let mut senders = self.senders.lock().expect("event bus lock poisoned");
        senders.retain(|tx| tx.send(event.clone()).is_ok());
        senders.len()
    }
}

impl<T: Clone + Send> Default for EventBus<T> {
    fn default() -> Self {
        EventBus::new()
    }
}

/// The receiving end of an [`EventBus`] subscription.
///
/// Dropping a subscriber disconnects it; the bus prunes it on the next
/// [`publish`](EventBus::publish).
#[derive(Debug)]
pub struct Subscriber<T> {
    receiver: Receiver<T>,
}

impl<T> Subscriber<T> {
    /// Returns the next pending event, or `None` if the queue is empty (or
    /// the bus has been dropped).
    pub fn try_recv(&self) -> Option<T> {
        self.receiver.try_recv().ok()
    }

    /// Drains all pending events in delivery order.
    pub fn drain(&self) -> Vec<T> {
        let mut events = Vec::new();
        while let Some(event) = self.try_recv() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_subscribers_both_receive() {
        let bus: EventBus<String> = EventBus::new();
        let a = bus.subscribe();
        let b = bus.subscribe();
        assert_eq!(bus.publish(&"hello".to_owned()), 2);
        assert_eq!(a.try_recv().as_deref(), Some("hello"));
        assert_eq!(b.try_recv().as_deref(), Some("hello"));
        assert_eq!(a.try_recv(), None);
    }

    #[test]
    fn drain_returns_all_pending_in_order() {
        let bus: EventBus<u32> = EventBus::default();
        let sub = bus.subscribe();
        bus.publish(&1);
        bus.publish(&2);
        bus.publish(&3);
        assert_eq!(sub.drain(), vec![1, 2, 3]);
        assert!(sub.drain().is_empty());
    }

    #[test]
    fn dropped_subscriber_is_pruned() {
        let bus: EventBus<u32> = EventBus::new();
        let keep = bus.subscribe();
        let dropped = bus.subscribe();
        assert_eq!(bus.publish(&1), 2);
        drop(dropped);
        assert_eq!(bus.publish(&2), 1);
        assert_eq!(keep.drain(), vec![1, 2]);
    }

    #[test]
    fn publish_with_no_subscribers_is_zero() {
        let bus: EventBus<u32> = EventBus::new();
        assert_eq!(bus.publish(&42), 0);
    }

    #[test]
    fn cross_thread_publish_and_receive() {
        let bus: EventBus<u32> = EventBus::new();
        let sub = bus.subscribe();
        std::thread::scope(|s| {
            s.spawn(|| {
                assert_eq!(bus.publish(&7), 1);
            });
        });
        assert_eq!(sub.try_recv(), Some(7));
    }
}
