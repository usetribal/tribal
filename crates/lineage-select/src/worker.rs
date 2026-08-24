//! Running searches off the UI thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::search::{SearchError, SessionMatch, SessionSearch};

/// A finished search, tagged with the keystroke generation that asked for it.
pub struct Answer {
    pub generation: u64,
    pub query: String,
    pub result: Result<Vec<SessionMatch>, SearchError>,
}

/// Runs searches on a worker thread and drops answers to superseded queries.
///
/// Every request carries a generation. A user who keeps typing raises it, so an
/// answer that arrives after the query moved on is discarded rather than
/// overwriting a newer list — the failure mode is otherwise a list that flicks
/// back to stale results whenever an earlier search finishes last.
pub struct SearchWorker {
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    generation: Arc<AtomicU64>,
    latest: u64,
}

struct Request {
    generation: u64,
    query: String,
}

impl SearchWorker {
    pub fn spawn<S>(search: S) -> Self
    where
        S: SessionSearch + Send + 'static,
    {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (answer_tx, answer_rx) = mpsc::channel::<Answer>();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);

        thread::spawn(move || {
            for request in request_rx {
                // Skip work the user has already typed past. Checked again
                // before sending, because the newest generation can arrive
                // while this search runs.
                if request.generation < worker_generation.load(Ordering::Relaxed) {
                    continue;
                }
                let result = search.search(&request.query);
                let answer = Answer {
                    generation: request.generation,
                    query: request.query,
                    result,
                };
                if answer_tx.send(answer).is_err() {
                    break;
                }
            }
        });

        Self {
            requests: request_tx,
            answers: answer_rx,
            generation,
            latest: 0,
        }
    }

    /// Ask for a search. Returns the generation it was filed under.
    pub fn request(&mut self, query: &str) -> u64 {
        self.latest += 1;
        self.generation.store(self.latest, Ordering::Relaxed);
        let _ = self.requests.send(Request {
            generation: self.latest,
            query: query.to_string(),
        });
        self.latest
    }

    /// Take the newest answer that is still current, discarding stale ones.
    pub fn poll(&self) -> Option<Answer> {
        let mut current = None;
        while let Ok(answer) = self.answers.try_recv() {
            if answer.generation >= self.latest {
                current = Some(answer);
            }
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    struct Fixed(Vec<String>);

    impl SessionSearch for Fixed {
        fn search(&self, query: &str) -> Result<Vec<SessionMatch>, SearchError> {
            if query == "boom" {
                return Err(SearchError::new("index missing"));
            }
            Ok(self
                .0
                .iter()
                .map(|id| SessionMatch {
                    id: id.clone(),
                    passage: None,
                })
                .collect())
        }
    }

    /// Wait for an answer rather than sleeping a fixed span, so the test cannot
    /// flake on a slow machine.
    fn wait_for(worker: &SearchWorker) -> Answer {
        for _ in 0..200 {
            if let Some(answer) = worker.poll() {
                return answer;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("no answer arrived");
    }

    #[test]
    fn an_answer_carries_the_query_that_asked_for_it() {
        let mut worker = SearchWorker::spawn(Fixed(vec!["a".into()]));
        worker.request("auth");
        let answer = wait_for(&worker);
        assert_eq!(answer.query, "auth");
        assert_eq!(
            answer.result,
            Ok(vec![SessionMatch {
                id: "a".into(),
                passage: None
            }])
        );
    }

    #[test]
    fn a_failed_search_reaches_the_caller_as_an_error() {
        let mut worker = SearchWorker::spawn(Fixed(vec![]));
        worker.request("boom");
        let answer = wait_for(&worker);
        assert_eq!(answer.result, Err(SearchError::new("index missing")));
    }

    #[test]
    fn a_superseded_answer_is_dropped() {
        let mut worker = SearchWorker::spawn(Fixed(vec!["a".into()]));
        let stale = worker.request("au");
        let current = worker.request("auth");
        let answer = wait_for(&worker);
        assert_eq!(answer.generation, current);
        assert!(answer.generation > stale);
    }

    #[test]
    fn nothing_is_polled_before_a_request() {
        let worker = SearchWorker::spawn(Fixed(vec!["a".into()]));
        assert!(worker.poll().is_none());
    }

    #[test]
    fn the_worker_stops_when_the_selector_goes_away() {
        let (tx, rx) = mpsc::channel();
        struct Signal(Sender<()>);
        impl SessionSearch for Signal {
            fn search(&self, _query: &str) -> Result<Vec<SessionMatch>, SearchError> {
                let _ = self.0.send(());
                Ok(vec![])
            }
        }
        let mut worker = SearchWorker::spawn(Signal(tx));
        worker.request("x");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(()));
        drop(worker);
        // Disconnected, not Timeout: the thread ended and dropped the sender it
        // owned, which is what proves it stopped rather than merely going quiet.
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
