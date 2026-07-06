use crossterm::event::{self, Event};
#[cfg(test)]
use std::collections::VecDeque;
use std::time::Duration;

/// Outcome of polling an event source. The three cases must stay distinct:
/// a `Timeout` is idle (the loop keeps polling), while `Closed` means the
/// source is gone for good and the loop must exit — collapsing them is how
/// a hung-up terminal used to busy-spin the editor forever.
pub enum Poll {
    /// An input event arrived.
    Event(Event),
    /// The poll timed out with no input; nothing can have changed.
    Timeout,
    /// The source is dead (terminal hangup, or a drained test queue).
    Closed,
}

pub trait EventSource {
    fn next_event(&mut self) -> Poll;
}

/// Production event source: reads from the terminal via crossterm.
pub struct TerminalEventSource;

impl EventSource for TerminalEventSource {
    fn next_event(&mut self) -> Poll {
        // Errors from poll/read mean the terminal is gone (e.g. tty
        // hangup); report Closed, never Timeout, or the caller would
        // spin on a dead terminal.
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(event) => Poll::Event(event),
                Err(_) => Poll::Closed,
            },
            Ok(false) => Poll::Timeout,
            Err(_) => Poll::Closed,
        }
    }
}

/// Test event source: replays a queue of events, then reports Closed.
#[cfg(test)]
pub struct TestEventSource {
    events: VecDeque<Event>,
}

#[cfg(test)]
impl TestEventSource {
    pub fn new(events: Vec<Event>) -> Self {
        Self {
            events: events.into(),
        }
    }
}

#[cfg(test)]
impl EventSource for TestEventSource {
    fn next_event(&mut self) -> Poll {
        match self.events.pop_front() {
            Some(event) => Poll::Event(event),
            None => Poll::Closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_event_source_yields_events_then_closes() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        let mut source = TestEventSource::new(vec![event.clone()]);
        assert!(matches!(source.next_event(), Poll::Event(e) if e == event));
        assert!(matches!(source.next_event(), Poll::Closed));
        // Closed is terminal: it must not flip back to Timeout.
        assert!(matches!(source.next_event(), Poll::Closed));
    }
}
