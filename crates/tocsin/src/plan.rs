//! Everything it takes to deliver one notification to one service.
//!
//! Most services need a single request. Some need several, where a request
//! depends on the answer to the one before: a login whose token the next
//! request carries, a channel name that has to be looked up before it can be
//! posted to, a file that is uploaded before a message points at it. A [`Plan`]
//! covers both, and does not care what sends the requests.
//!
//! ```
//! # #[cfg(feature = "ntfy")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use tocsin::{MockTransport, Notification, Service, Transport};
//!
//! let service: Service = "ntfy://my-topic".parse()?;
//! let mut transport = MockTransport::default();
//! let mut plan = service.plan(&Notification::new("hello"));
//! while let Some(request) = plan.next_request() {
//!     // Send it with any client, then say how it went.
//!     plan.report(transport.send(&request));
//! }
//! assert_eq!(plan.finish().len(), 1);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "ntfy"))]
//! # fn main() {}
//! ```

use std::{collections::VecDeque, fmt};

use crate::{Outcome, PreparedRequest, Response, TransportError};

/// Requests in which each one may depend on the answer to the one before.
pub(crate) trait Sequence: Send {
    /// The request that starts the sequence.
    fn start(&mut self) -> PreparedRequest;

    /// Read the answer to the last request. `Ok(Some(request))` goes on with
    /// that request, `Ok(None)` ends the sequence successfully, and an error
    /// ends it as a failure.
    fn answer(&mut self, response: &Response) -> Result<Option<PreparedRequest>, TransportError>;
}

enum Job {
    Request(PreparedRequest),
    Sequence(Box<dyn Sequence>),
}

/// The requests that deliver a notification to one service, in order.
///
/// Ask for the next request with [`next_request`](Self::next_request), send it
/// with any client, and give the result to [`report`](Self::report). Repeat
/// until there is no next request, then read the outcomes with
/// [`finish`](Self::finish). A failed request does not stop the plan: the
/// sequence it belonged to ends, and the next independent request still goes
/// out.
pub struct Plan {
    jobs: VecDeque<Job>,
    /// The sequence that the request last handed out belongs to.
    current: Option<Box<dyn Sequence>>,
    /// The next step of `current`, once its last answer has been read.
    queued: Option<PreparedRequest>,
    /// Whether a request was handed out and its result is still due.
    awaiting: bool,
    had_jobs: bool,
    outcomes: Vec<Outcome>,
}

impl Plan {
    fn new(jobs: VecDeque<Job>) -> Self {
        Self {
            had_jobs: !jobs.is_empty(),
            jobs,
            current: None,
            queued: None,
            awaiting: false,
            outcomes: Vec::new(),
        }
    }

    /// A plan of independent requests.
    pub(crate) fn requests(requests: Vec<PreparedRequest>) -> Self {
        Self::new(requests.into_iter().map(Job::Request).collect())
    }

    /// Add a sequence after what is already planned.
    #[must_use]
    pub(crate) fn then(mut self, sequence: impl Sequence + 'static) -> Self {
        self.jobs.push_back(Job::Sequence(Box::new(sequence)));
        self.had_jobs = true;
        self
    }

    /// The next request to send, or `None` when the plan is done.
    ///
    /// Report the result of every request before asking for the next one. A
    /// request that is never reported counts as a failed connection.
    pub fn next_request(&mut self) -> Option<PreparedRequest> {
        if self.awaiting {
            self.report(Err(TransportError::Connection));
        }
        if let Some(request) = self.queued.take() {
            self.awaiting = true;
            return Some(request);
        }
        match self.jobs.pop_front()? {
            Job::Request(request) => {
                self.awaiting = true;
                Some(request)
            }
            Job::Sequence(mut sequence) => {
                let first = sequence.start();
                self.current = Some(sequence);
                self.awaiting = true;
                Some(first)
            }
        }
    }

    /// Say how the request that [`next_request`](Self::next_request) handed
    /// out went. Does nothing when no request is waiting for a result.
    pub fn report(&mut self, result: Result<Response, TransportError>) {
        if !self.awaiting {
            return;
        }
        self.awaiting = false;
        let sequence = self.current.take();
        match (result, sequence) {
            (Ok(response), None) => self.outcomes.push(Outcome::Delivered(response)),
            (Ok(response), Some(mut sequence)) => match sequence.answer(&response) {
                Ok(Some(next)) => {
                    self.current = Some(sequence);
                    self.queued = Some(next);
                }
                Ok(None) => self.outcomes.push(Outcome::Delivered(response)),
                Err(error) => self.outcomes.push(Outcome::Failed(error)),
            },
            (Err(error), _) => self.outcomes.push(Outcome::Failed(error)),
        }
    }

    /// What happened to each delivery. A plan that had nothing to send says
    /// [`Outcome::NothingToSend`].
    #[must_use]
    pub fn finish(mut self) -> Vec<Outcome> {
        if self.awaiting {
            self.report(Err(TransportError::Connection));
        }
        if self.had_jobs {
            self.outcomes
        } else {
            vec![Outcome::NothingToSend]
        }
    }
}

impl fmt::Debug for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plan")
            .field("waiting", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> PreparedRequest {
        PreparedRequest::json(url, &serde_json::json!({}))
    }

    /// Asks for `first`, then, if the answer says so, for `second`.
    struct Login {
        answers: Vec<u16>,
    }

    impl Sequence for Login {
        fn start(&mut self) -> PreparedRequest {
            request("https://example.com/login")
        }

        fn answer(
            &mut self,
            response: &Response,
        ) -> Result<Option<PreparedRequest>, TransportError> {
            self.answers.push(response.status);
            match self.answers.len() {
                1 => Ok(Some(request("https://example.com/post"))),
                2 => Ok(None),
                _ => Err(TransportError::InvalidRequest),
            }
        }
    }

    fn drive(mut plan: Plan, results: &mut Vec<Result<Response, TransportError>>) -> Vec<String> {
        let mut sent = Vec::new();
        while let Some(request) = plan.next_request() {
            sent.push(request.url.expose().to_owned());
            plan.report(results.remove(0));
        }
        results.clear();
        let outcomes = plan.finish();
        sent.push(format!("{} outcomes", outcomes.len()));
        sent
    }

    #[test]
    fn an_empty_plan_has_nothing_to_send() {
        let plan = Plan::requests(Vec::new());
        assert_eq!(plan.finish(), [Outcome::NothingToSend]);
    }

    #[test]
    fn independent_requests_each_get_an_outcome() {
        let plan = Plan::requests(vec![
            request("https://a.example"),
            request("https://b.example"),
        ]);
        let mut results = vec![Ok(Response::new(200)), Err(TransportError::HttpStatus(500))];
        assert_eq!(
            drive(plan, &mut results),
            ["https://a.example", "https://b.example", "2 outcomes"]
        );
    }

    #[test]
    fn a_sequence_goes_on_with_the_answers() {
        let plan = Plan::requests(Vec::new()).then(Login {
            answers: Vec::new(),
        });
        let mut results = vec![Ok(Response::new(200)), Ok(Response::new(201))];
        assert_eq!(
            drive(plan, &mut results),
            [
                "https://example.com/login",
                "https://example.com/post",
                "1 outcomes"
            ]
        );
    }

    #[test]
    fn a_failed_step_ends_its_sequence_but_not_the_plan() {
        let plan = Plan::requests(vec![request("https://after.example")]).then(Login {
            answers: Vec::new(),
        });
        // The plan runs the independent request first, then the sequence, whose
        // first step fails, so its second step is never sent.
        let mut results = vec![Ok(Response::new(200)), Err(TransportError::HttpStatus(401))];
        let sent = drive(plan, &mut results);
        assert_eq!(
            sent,
            [
                "https://after.example",
                "https://example.com/login",
                "2 outcomes"
            ]
        );
    }

    #[test]
    fn an_unreported_request_counts_as_a_failure() {
        let mut plan = Plan::requests(vec![request("https://a.example")]);
        assert!(plan.next_request().is_some());
        assert!(plan.next_request().is_none());
        assert_eq!(plan.finish(), [Outcome::Failed(TransportError::Connection)]);
    }
}
