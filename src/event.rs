use crossterm::event::{self, Event};
use std::collections::VecDeque;
use std::time::Duration;

pub trait EventSource {
    fn next_event(&mut self) -> Option<Event>;
}

/// Production event source: reads from the terminal via crossterm.
pub struct TerminalEventSource;

impl EventSource for TerminalEventSource {
    fn next_event(&mut self) -> Option<Event> {
        if event::poll(Duration::from_millis(100)).ok()? {
            event::read().ok()
        } else {
            None
        }
    }
}

/// Test event source: replays a queue of events.
#[allow(dead_code)]
pub struct TestEventSource {
    events: VecDeque<Event>,
}

#[allow(dead_code)]
impl TestEventSource {
    pub fn new(events: Vec<Event>) -> Self {
        Self {
            events: events.into(),
        }
    }
}

impl EventSource for TestEventSource {
    fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
}
