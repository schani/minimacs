use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use ropey::Rope;

use crate::syntax::{BackgroundEdit, Language, SyntaxCompletion, SyntaxState};

/// A complete immutable snapshot of the work needed for one syntax state.
/// Rope clones share their backing chunks, so submitting a snapshot is O(1).
pub(crate) struct SyntaxJob {
    pub(crate) key: usize,
    pub(crate) language: Language,
    pub(crate) base_generation: Option<usize>,
    pub(crate) generation: usize,
    pub(crate) source: Rope,
    pub(crate) edits: Vec<BackgroundEdit>,
    pub(crate) requested: Range<usize>,
}

#[derive(Default)]
struct PendingJobs {
    jobs: HashMap<usize, SyntaxJob>,
    order: VecDeque<usize>,
}

impl PendingJobs {
    fn push(&mut self, job: SyntaxJob) {
        let key = job.key;
        if self.jobs.insert(key, job).is_none() {
            self.order.push_back(key);
        }
    }

    fn pop(&mut self) -> Option<SyntaxJob> {
        while let Some(key) = self.order.pop_front() {
            if let Some(job) = self.jobs.remove(&key) {
                return Some(job);
            }
        }
        None
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.jobs.len()
    }
}

#[derive(Default)]
struct SharedState {
    pending: PendingJobs,
    completions: HashMap<usize, SyntaxCompletion>,
    shutdown: bool,
}

type Shared = Arc<(Mutex<SharedState>, Condvar)>;

struct WorkerDocument {
    language: Language,
    generation: usize,
    syntax: SyntaxState,
}

/// A lazily started, single-thread syntax executor. The pending and completed
/// maps are each bounded to one entry per syntax-state key.
pub(crate) struct SyntaxWorker {
    shared: Shared,
    thread: Mutex<Option<JoinHandle<()>>>,
    #[cfg(test)]
    thread_starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl SyntaxWorker {
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new((Mutex::new(SharedState::default()), Condvar::new())),
            thread: Mutex::new(None),
            #[cfg(test)]
            thread_starts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub(crate) fn submit(&self, job: SyntaxJob) {
        self.ensure_started();
        let (state, ready) = &*self.shared;
        let mut state = state.lock().expect("syntax worker state poisoned");
        state.pending.push(job);
        ready.notify_one();
    }

    pub(crate) fn take_completions(&self) -> Vec<SyntaxCompletion> {
        self.shared
            .0
            .lock()
            .expect("syntax worker state poisoned")
            .completions
            .drain()
            .map(|(_, completion)| completion)
            .collect()
    }

    fn ensure_started(&self) {
        let mut handle = self.thread.lock().expect("syntax worker handle poisoned");
        if handle.is_some() {
            return;
        }

        let shared = Arc::clone(&self.shared);
        #[cfg(test)]
        let thread_starts = Arc::clone(&self.thread_starts);
        *handle = Some(
            thread::Builder::new()
                .name("minimacs-syntax".to_string())
                .spawn(move || {
                    #[cfg(test)]
                    thread_starts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    worker_loop(shared);
                })
                .expect("failed to start syntax worker"),
        );
    }

    #[cfg(test)]
    fn thread_start_count(&self) -> usize {
        self.thread_starts
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for SyntaxWorker {
    fn drop(&mut self) {
        {
            let (state, ready) = &*self.shared;
            let mut state = state.lock().expect("syntax worker state poisoned");
            state.shutdown = true;
            ready.notify_one();
        }
        if let Some(handle) = self
            .thread
            .get_mut()
            .expect("syntax worker handle poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn worker_loop(shared: Shared) {
    let mut documents = HashMap::<usize, WorkerDocument>::new();
    loop {
        let job = {
            let (state, ready) = &*shared;
            let mut state = state.lock().expect("syntax worker state poisoned");
            while state.pending.jobs.is_empty() && !state.shutdown {
                state = ready.wait(state).expect("syntax worker state poisoned");
            }
            if state.shutdown {
                return;
            }
            state.pending.pop().expect("pending job disappeared")
        };

        let completion = process_job(&mut documents, job);
        let mut state = shared.0.lock().expect("syntax worker state poisoned");
        let should_publish = state
            .completions
            .get(&completion.key)
            .is_none_or(|current| current.generation <= completion.generation);
        if should_publish {
            state.completions.insert(completion.key, completion);
        }
    }
}

fn process_job(documents: &mut HashMap<usize, WorkerDocument>, job: SyntaxJob) -> SyntaxCompletion {
    let can_update = documents.get(&job.key).is_some_and(|document| {
        let relevant = job
            .edits
            .iter()
            .filter(|edit| edit.generation > document.generation)
            .collect::<Vec<_>>();
        document.language == job.language
            && job
                .base_generation
                .is_some_and(|base| base <= document.generation)
            && document.generation <= job.generation
            && (document.generation == job.generation
                || (relevant
                    .first()
                    .is_some_and(|edit| edit.generation == document.generation + 1)
                    && relevant
                        .last()
                        .is_some_and(|edit| edit.generation == job.generation)))
    });

    if !can_update {
        documents.remove(&job.key);
        if let Some(syntax) = SyntaxState::new(job.language) {
            documents.insert(
                job.key,
                WorkerDocument {
                    language: job.language,
                    generation: job.generation,
                    syntax,
                },
            );
        }
    } else if let Some(document) = documents.get_mut(&job.key) {
        let edits = job
            .edits
            .iter()
            .filter(|edit| edit.generation > document.generation)
            .map(|edit| edit.edit)
            .collect::<Vec<_>>();
        document.syntax.apply_edits(job.source.slice(..), &edits);
        document.generation = job.generation;
    }

    let (completed_range, spans) = documents
        .get(&job.key)
        .map(|document| {
            document.syntax.highlight_rope_window(
                job.source.slice(..),
                job.requested.clone(),
                job.generation,
            )
        })
        .unwrap_or((job.requested.clone(), Vec::new()));

    SyntaxCompletion {
        key: job.key,
        generation: job.generation,
        requested: completed_range,
        spans,
        disabled: documents
            .get(&job.key)
            .is_some_and(|document| document.syntax.is_disabled()),
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::time::{Duration, Instant};

    use ratatui::style::{Color, Style};
    use ropey::Rope;
    use tree_house::tree_sitter::{InputEdit, Point};

    use super::*;
    use crate::syntax::{Language, StyledSpan};

    fn job(key: usize, generation: usize, source: &str, requested: Range<usize>) -> SyntaxJob {
        SyntaxJob {
            key,
            language: Language::Rust,
            base_generation: None,
            generation,
            source: Rope::from_str(source),
            edits: Vec::new(),
            requested,
        }
    }

    #[test]
    fn pending_jobs_are_bounded_to_one_latest_snapshot_per_syntax_state() {
        let mut pending = PendingJobs::default();
        pending.push(job(7, 1, "fn one() {}", 0..11));
        pending.push(job(7, 2, "fn two() {}", 0..11));
        pending.push(job(9, 4, "fn other() {}", 0..13));

        assert_eq!(pending.len(), 2);
        let first = pending.pop().expect("first pending job");
        let second = pending.pop().expect("second pending job");
        assert_eq!((first.key, first.generation), (7, 2));
        assert_eq!((second.key, second.generation), (9, 4));
        assert!(pending.pop().is_none());
    }

    #[test]
    fn worker_starts_one_thread_and_publishes_only_the_latest_completion() {
        let worker = SyntaxWorker::new();
        worker.submit(job(3, 0, "fn zero() {}", 0..12));
        worker.submit(job(3, 1, "fn one() {}", 0..11));
        worker.submit(job(3, 2, "fn two() {}", 0..11));

        let deadline = Instant::now() + Duration::from_secs(5);
        let completion = loop {
            if let Some(completion) = worker
                .take_completions()
                .into_iter()
                .find(|completion| completion.key == 3)
            {
                if completion.generation == 2 {
                    break completion;
                }
            }
            assert!(Instant::now() < deadline, "worker completion timed out");
            std::thread::yield_now();
        };

        assert_eq!(completion.key, 3);
        assert_eq!(completion.generation, 2);
        assert!(!completion.spans.is_empty());
        assert_eq!(worker.thread_start_count(), 1);
    }

    #[test]
    fn syntax_state_accepts_only_current_versioned_completions() {
        let syntax = SyntaxState::new(Language::Rust).unwrap();
        let key = syntax.background_key();
        assert!(syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 0,
                requested: 0..2,
                spans: Vec::new(),
                disabled: false,
            },
            0,
        ));

        syntax.note_background_edit(
            1,
            InputEdit {
                start_byte: 0,
                old_end_byte: 0,
                new_end_byte: 1,
                start_point: Point { row: 0, col: 0 },
                old_end_point: Point { row: 0, col: 0 },
                new_end_point: Point { row: 0, col: 1 },
            },
        );
        let (base_generation, edits) = syntax.background_update_for(1);
        assert_eq!(base_generation, Some(0));
        assert_eq!(edits.len(), 1);

        assert!(!syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 0,
                requested: 0..2,
                spans: Vec::new(),
                disabled: false,
            },
            1,
        ));

        let style = Style::default().fg(Color::Blue);
        assert!(syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 1,
                requested: 0..2,
                spans: vec![StyledSpan {
                    start: 0,
                    end: 2,
                    style,
                }],
                disabled: true,
            },
            1,
        ));
        let current = syntax.background_spans(0..2, 1);
        assert!(current.exact);
        assert_eq!(current.spans[0].style, style);
        assert!(syntax.background_spans(0..2, 0).spans.is_empty());
        assert!(syntax.take_disabled_message());
        assert!(!syntax.take_disabled_message());
    }

    #[test]
    fn worker_reuses_an_intermediate_generation_from_a_coalesced_edit_batch() {
        use crate::syntax::BackgroundEdit;

        fn insertion(byte: u32) -> InputEdit {
            InputEdit {
                start_byte: byte,
                old_end_byte: byte,
                new_end_byte: byte + 1,
                start_point: Point { row: 0, col: byte },
                old_end_point: Point { row: 0, col: byte },
                new_end_point: Point {
                    row: 0,
                    col: byte + 1,
                },
            }
        }

        let mut documents = HashMap::new();
        process_job(&mut documents, job(41, 0, "fn main() {}", 0..12));
        process_job(
            &mut documents,
            SyntaxJob {
                key: 41,
                language: Language::Rust,
                base_generation: Some(0),
                generation: 1,
                source: Rope::from_str("fn xmain() {}"),
                edits: vec![BackgroundEdit {
                    generation: 1,
                    edit: insertion(3),
                }],
                requested: 0..13,
            },
        );
        process_job(
            &mut documents,
            SyntaxJob {
                key: 41,
                language: Language::Rust,
                base_generation: Some(0),
                generation: 2,
                source: Rope::from_str("fn xymain() {}"),
                edits: vec![
                    BackgroundEdit {
                        generation: 1,
                        edit: insertion(3),
                    },
                    BackgroundEdit {
                        generation: 2,
                        edit: insertion(4),
                    },
                ],
                requested: 0..14,
            },
        );

        let document = documents.get(&41).unwrap();
        assert_eq!(document.syntax.full_parse_count(), 1);
        assert_eq!(document.syntax.incremental_update_count(), 2);
    }

    #[test]
    fn disjoint_viewports_for_one_generation_do_not_evict_each_other() {
        let syntax = SyntaxState::new(Language::Rust).unwrap();
        let key = syntax.background_key();
        for requested in [0..10, 100..110] {
            assert!(syntax.accept_background_completion(
                SyntaxCompletion {
                    key,
                    generation: 0,
                    requested,
                    spans: Vec::new(),
                    disabled: false,
                },
                0,
            ));
        }

        assert!(syntax.background_spans(0..10, 0).exact);
        assert!(syntax.background_spans(100..110, 0).exact);
    }

    #[test]
    fn scrolling_keeps_overlapping_cached_spans_while_requesting_the_remainder() {
        let syntax = SyntaxState::new(Language::Rust).unwrap();
        let key = syntax.background_key();
        let style = Style::default().fg(Color::Blue);
        assert!(syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 0,
                requested: 0..100,
                spans: vec![StyledSpan {
                    start: 60,
                    end: 70,
                    style,
                }],
                disabled: false,
            },
            0,
        ));

        let cached = syntax.background_spans(50..150, 0);

        assert!(!cached.exact);
        assert_eq!(cached.spans.len(), 1);
        assert_eq!((cached.spans[0].start, cached.spans[0].end), (60, 70));
    }

    #[test]
    fn edits_rebase_unaffected_spans_and_keep_them_provisional() {
        let syntax = SyntaxState::new(Language::Rust).unwrap();
        let key = syntax.background_key();
        let before = Style::default().fg(Color::Blue);
        let replaced = Style::default().fg(Color::Red);
        let after = Style::default().fg(Color::Green);
        assert!(syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 0,
                requested: 0..10,
                spans: vec![
                    StyledSpan {
                        start: 0,
                        end: 2,
                        style: before,
                    },
                    StyledSpan {
                        start: 3,
                        end: 6,
                        style: replaced,
                    },
                    StyledSpan {
                        start: 7,
                        end: 10,
                        style: after,
                    },
                ],
                disabled: false,
            },
            0,
        ));

        syntax.note_background_edit(
            1,
            InputEdit {
                start_byte: 3,
                old_end_byte: 6,
                new_end_byte: 5,
                start_point: Point { row: 0, col: 3 },
                old_end_point: Point { row: 0, col: 6 },
                new_end_point: Point { row: 0, col: 5 },
            },
        );
        let cached = syntax.background_spans(0..9, 1);

        assert!(!cached.exact);
        assert_eq!(cached.spans.len(), 2);
        assert_eq!(
            cached
                .spans
                .iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>(),
            vec![(0, 2, before), (6, 9, after)]
        );
    }

    #[test]
    fn editing_inside_a_long_capture_preserves_both_unaffected_sides() {
        let syntax = SyntaxState::new(Language::Rust).unwrap();
        let key = syntax.background_key();
        let style = Style::default().fg(Color::Green);
        assert!(syntax.accept_background_completion(
            SyntaxCompletion {
                key,
                generation: 0,
                requested: 0..100,
                spans: vec![StyledSpan {
                    start: 0,
                    end: 100,
                    style,
                }],
                disabled: false,
            },
            0,
        ));

        syntax.note_background_edit(
            1,
            InputEdit {
                start_byte: 50,
                old_end_byte: 50,
                new_end_byte: 51,
                start_point: Point { row: 0, col: 50 },
                old_end_point: Point { row: 0, col: 50 },
                new_end_point: Point { row: 0, col: 51 },
            },
        );
        let cached = syntax.background_spans(0..101, 1);

        assert_eq!(
            cached
                .spans
                .iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>(),
            vec![(0, 50, style), (51, 101, style)]
        );
    }

    #[test]
    fn worker_completion_publishes_its_padded_highlight_window() {
        let source = " ".repeat(100_000);
        let visible = 50_000..50_100;
        let mut documents = HashMap::new();

        let completion = process_job(&mut documents, job(91, 0, &source, visible.clone()));

        assert!(completion.requested.start < visible.start);
        assert!(completion.requested.end > visible.end);
        assert!(
            completion.requested.end - completion.requested.start <= visible.len() + 2 * 8 * 1024
        );
    }
}
