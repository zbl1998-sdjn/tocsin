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

#[cfg(feature = "_lookups")]
use std::collections::HashMap;
use std::{collections::VecDeque, fmt};

use crate::{Outcome, PreparedRequest, Response, TransportError};

/// Reads the body of an answer that has no error status and says whether the
/// service accepted the notification. For services that answer `200` either way.
pub(crate) type Check = fn(&Response) -> bool;

/// What a request in a [`Sequence`] is for, which decides what its result means
/// for the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "a build with only some services leaves some of these unused"
)]
pub(crate) enum Role {
    /// Delivers the notification. Its result is an outcome of the plan.
    Delivery,
    /// Prepares the deliveries, such as a login or a lookup. Only a failure is
    /// an outcome.
    Setup,
    /// Tidies up afterwards, such as a logout. Its result is not reported.
    Cleanup,
}

/// A request of a [`Sequence`] and what it is for.
#[cfg_attr(not(feature = "_sequence"), allow(dead_code))]
pub(crate) struct Step {
    pub(crate) request: PreparedRequest,
    pub(crate) role: Role,
    pub(crate) check: Option<Check>,
}

#[allow(
    dead_code,
    reason = "a build with only some services leaves some of these unused"
)]
impl Step {
    pub(crate) fn delivery(request: PreparedRequest) -> Self {
        Self {
            request,
            role: Role::Delivery,
            check: None,
        }
    }

    pub(crate) fn setup(request: PreparedRequest) -> Self {
        Self {
            request,
            role: Role::Setup,
            check: None,
        }
    }

    pub(crate) fn cleanup(request: PreparedRequest) -> Self {
        Self {
            request,
            role: Role::Cleanup,
            check: None,
        }
    }

    /// Have the answer to this request checked.
    pub(crate) fn checked(mut self, check: Option<Check>) -> Self {
        self.check = check;
        self
    }
}

/// What a [`Sequence`] does after an answer.
#[cfg_attr(not(feature = "_sequence"), allow(dead_code))]
pub(crate) struct Next {
    /// A failure to report, whatever happens next.
    failure: Option<TransportError>,
    /// The request to go on with, or `None` to end the sequence.
    step: Option<Step>,
}

#[allow(
    dead_code,
    reason = "a build with only some services leaves some of these unused"
)]
impl Next {
    /// Go on with this request.
    pub(crate) fn go(step: Step) -> Self {
        Self {
            failure: None,
            step: Some(step),
        }
    }

    /// Nothing more to send.
    pub(crate) fn done() -> Self {
        Self {
            failure: None,
            step: None,
        }
    }

    /// The answer cannot be used: report the failure and stop.
    pub(crate) fn fail(error: TransportError) -> Self {
        Self {
            failure: Some(error),
            step: None,
        }
    }

    /// The answer cannot be used, but the others can still be: report the
    /// failure and go on with `step`, or stop when there is none.
    pub(crate) fn skip(error: TransportError, step: Option<Step>) -> Self {
        Self {
            failure: Some(error),
            step,
        }
    }
}

/// Requests in which each one may depend on the answer to the one before.
pub(crate) trait Sequence: Send {
    /// The request that starts the sequence.
    fn start(&mut self) -> Step;

    /// Read what became of the last request and decide what is next. Failures
    /// are shown too, so that one failed delivery does not keep the others
    /// from going out or the logout from being sent.
    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next;
}

/// What a name that was looked up stands for, by name.
#[cfg(feature = "_lookups")]
type Found = HashMap<String, String>;

/// Builds the deliveries that need the answers to the lookups.
#[cfg(feature = "_lookups")]
type Deliveries = Box<dyn FnOnce(&Found) -> Vec<Step> + Send>;

/// Looks names up, one request each, and then sends the deliveries that are
/// built from what was found. A name that cannot be resolved is a failure of its
/// own, and the deliveries that need no lookup still go out.
#[cfg(feature = "_lookups")]
pub(crate) struct Lookups {
    pending: VecDeque<String>,
    /// The name whose answer is awaited.
    asked: Option<String>,
    /// What each name that was resolved stands for.
    found: Found,
    request: Box<dyn Fn(&str) -> Step + Send>,
    read: fn(&Response) -> Option<String>,
    /// Builds the deliveries once every name was looked up.
    deliveries: Option<Deliveries>,
    queue: VecDeque<Step>,
}

#[cfg(feature = "_lookups")]
impl Lookups {
    /// `names` must not be empty: a plan without a lookup needs no sequence.
    pub(crate) fn new(
        names: Vec<String>,
        request: impl Fn(&str) -> Step + Send + 'static,
        read: fn(&Response) -> Option<String>,
        deliveries: impl FnOnce(&Found) -> Vec<Step> + Send + 'static,
    ) -> Self {
        Self {
            pending: names.into(),
            asked: None,
            found: HashMap::new(),
            request: Box::new(request),
            read,
            deliveries: Some(Box::new(deliveries)),
            queue: VecDeque::new(),
        }
    }

    /// The next lookup, or the next delivery once every name was looked up.
    fn advance(&mut self) -> Option<Step> {
        if let Some(name) = self.pending.pop_front() {
            let step = (self.request)(&name);
            self.asked = Some(name);
            return Some(step);
        }
        if let Some(build) = self.deliveries.take() {
            self.queue = build(&self.found).into();
        }
        self.queue.pop_front()
    }

    fn next(&mut self) -> Next {
        self.advance().map_or_else(Next::done, Next::go)
    }
}

#[cfg(feature = "_lookups")]
impl Sequence for Lookups {
    fn start(&mut self) -> Step {
        self.advance()
            .expect("a lookup is planned only when there is a name to look up")
    }

    fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
        let Some(name) = self.asked.take() else {
            // The answer to a delivery.
            return self.next();
        };
        match result.map(self.read) {
            Ok(Some(found)) => {
                self.found.insert(name, found);
                self.next()
            }
            // The answer says nothing usable: a failure the plan cannot see.
            Ok(None) => Next::skip(TransportError::InvalidResponse, self.advance()),
            // A failed request is already a failure of the plan.
            Err(_) => self.next(),
        }
    }
}

enum Job {
    Request(PreparedRequest, Option<Check>),
    /// Something that cannot be sent but that is a failure all the same.
    #[allow(
        dead_code,
        reason = "a build with only some services leaves this unused"
    )]
    Failure(TransportError),
    #[cfg_attr(not(feature = "_sequence"), allow(dead_code))]
    Sequence(Box<dyn Sequence>),
}

/// The requests that deliver a notification to one service, in order.
///
/// Ask for the next request with [`next_request`](Self::next_request), send it
/// with any client, and give the result to [`report`](Self::report). Repeat
/// until there is no next request, then read the outcomes with
/// [`finish`](Self::finish). A failed request does not stop the plan: the
/// requests that do not depend on it still go out.
pub struct Plan {
    jobs: VecDeque<Job>,
    /// The sequence that the request last handed out belongs to.
    current: Option<Box<dyn Sequence>>,
    /// The next step of `current`, once its last answer has been read.
    queued: Option<Step>,
    /// What the request that was handed out last is for.
    role: Role,
    /// How to read the answer to the request that was handed out last.
    check: Option<Check>,
    /// Whether a request was handed out and its result is still due.
    awaiting: bool,
    outcomes: Vec<Outcome>,
}

impl Plan {
    fn new(jobs: VecDeque<Job>) -> Self {
        Self {
            jobs,
            current: None,
            queued: None,
            role: Role::Delivery,
            check: None,
            awaiting: false,
            outcomes: Vec::new(),
        }
    }

    /// A plan of independent requests.
    pub(crate) fn requests(requests: Vec<PreparedRequest>) -> Self {
        Self::checked(requests, None)
    }

    /// A plan of independent requests whose answers are checked.
    pub(crate) fn checked(requests: Vec<PreparedRequest>, check: Option<Check>) -> Self {
        Self::new(
            requests
                .into_iter()
                .map(|request| Job::Request(request, check))
                .collect(),
        )
    }

    /// Add a failure after what is already planned, for a part of the
    /// notification that cannot be sent.
    #[must_use]
    #[allow(
        dead_code,
        reason = "a build with only some services leaves this unused"
    )]
    pub(crate) fn failed(mut self, error: TransportError) -> Self {
        self.jobs.push_back(Job::Failure(error));
        self
    }

    /// Add a sequence after what is already planned.
    #[must_use]
    #[cfg_attr(not(feature = "_sequence"), allow(dead_code))]
    pub(crate) fn then(mut self, sequence: impl Sequence + 'static) -> Self {
        self.jobs.push_back(Job::Sequence(Box::new(sequence)));
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
        let step = match self.queued.take() {
            Some(step) => step,
            None => loop {
                match self.jobs.pop_front()? {
                    Job::Request(request, check) => {
                        break Step::delivery(request).checked(check);
                    }
                    Job::Failure(error) => self.outcomes.push(Outcome::Failed(error)),
                    Job::Sequence(mut sequence) => {
                        let first = sequence.start();
                        self.current = Some(sequence);
                        break first;
                    }
                }
            },
        };
        self.role = step.role;
        self.check = step.check;
        self.awaiting = true;
        Some(step.request)
    }

    /// Say how the request that [`next_request`](Self::next_request) handed
    /// out went. Does nothing when no request is waiting for a result.
    pub fn report(&mut self, result: Result<Response, TransportError>) {
        if !self.awaiting {
            return;
        }
        self.awaiting = false;
        let result = match (result, self.check) {
            (Ok(response), Some(check)) if !check(&response) => Err(TransportError::Rejected),
            (result, _) => result,
        };
        let next = self
            .current
            .as_mut()
            .map(|sequence| sequence.answer(result.as_ref()));
        match (self.role, result) {
            (Role::Cleanup, _) | (Role::Setup, Ok(_)) => {}
            (_, Err(error)) => self.outcomes.push(Outcome::Failed(error)),
            (Role::Delivery, Ok(response)) => self.outcomes.push(Outcome::Delivered(response)),
        }
        if let Some(Next { failure, step }) = next {
            if let Some(error) = failure {
                self.outcomes.push(Outcome::Failed(error));
            }
            match step {
                Some(step) => self.queued = Some(step),
                None => self.current = None,
            }
        }
    }

    /// What happened to each delivery. A plan that delivered nothing and
    /// failed at nothing says [`Outcome::NothingToSend`].
    #[must_use]
    pub fn finish(mut self) -> Vec<Outcome> {
        if self.awaiting {
            self.report(Err(TransportError::Connection));
        }
        // A failure is known without sending anything, so it counts even when
        // nobody asked for the requests that come after it.
        for job in std::mem::take(&mut self.jobs) {
            if let Job::Failure(error) = job {
                self.outcomes.push(Outcome::Failed(error));
            }
        }
        if self.outcomes.is_empty() {
            vec![Outcome::NothingToSend]
        } else {
            self.outcomes
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

    /// Hands out its steps one after the other, whatever the answers are, and
    /// remembers whether each answer was a success. With `fail_after`, it
    /// gives up on the answer with that number.
    struct Script {
        steps: VecDeque<Step>,
        answers: Vec<bool>,
        fail_after: Option<usize>,
    }

    impl Script {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: steps.into(),
                answers: Vec::new(),
                fail_after: None,
            }
        }
    }

    impl Sequence for Script {
        fn start(&mut self) -> Step {
            self.steps.pop_front().expect("a first step")
        }

        fn answer(&mut self, result: Result<&Response, &TransportError>) -> Next {
            self.answers.push(result.is_ok());
            if self.fail_after == Some(self.answers.len()) {
                return Next::fail(TransportError::InvalidResponse);
            }
            // A failed login or lookup leaves nothing to go on with.
            if result.is_err() && self.answers.len() == 1 {
                return Next::done();
            }
            self.steps.pop_front().map_or_else(Next::done, Next::go)
        }
    }

    fn session() -> Vec<Step> {
        vec![
            Step::setup(request("https://example.com/login")),
            Step::delivery(request("https://example.com/post-1")),
            Step::delivery(request("https://example.com/post-2")),
            Step::cleanup(request("https://example.com/logout")),
        ]
    }

    /// Sends everything the plan asks for, answering from `results`, and
    /// returns the URLs and the outcomes.
    fn drive(
        mut plan: Plan,
        results: Vec<Result<Response, TransportError>>,
    ) -> (Vec<String>, Vec<Outcome>) {
        let mut results = VecDeque::from(results);
        let mut sent = Vec::new();
        while let Some(request) = plan.next_request() {
            sent.push(request.url.expose().to_owned());
            plan.report(
                results
                    .pop_front()
                    .unwrap_or_else(|| Ok(Response::new(200))),
            );
        }
        (sent, plan.finish())
    }

    #[allow(clippy::unnecessary_wraps, reason = "stands for a delivered request")]
    fn ok() -> Result<Response, TransportError> {
        Ok(Response::new(200))
    }

    fn rejected() -> Result<Response, TransportError> {
        Err(TransportError::HttpStatus(500))
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
        let (sent, outcomes) = drive(plan, vec![ok(), rejected()]);
        assert_eq!(sent, ["https://a.example", "https://b.example"]);
        assert_eq!(
            outcomes,
            [
                Outcome::Delivered(Response::new(200)),
                Outcome::Failed(TransportError::HttpStatus(500)),
            ]
        );
    }

    #[test]
    fn only_deliveries_of_a_sequence_are_outcomes() {
        let plan = Plan::requests(Vec::new()).then(Script::new(session()));
        let (sent, outcomes) = drive(plan, Vec::new());
        assert_eq!(
            sent,
            [
                "https://example.com/login",
                "https://example.com/post-1",
                "https://example.com/post-2",
                "https://example.com/logout",
            ]
        );
        assert_eq!(outcomes.len(), 2);
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Outcome::Delivered(_)))
        );
    }

    #[test]
    fn a_failed_delivery_does_not_keep_the_others_or_the_cleanup_from_going_out() {
        let plan = Plan::requests(Vec::new()).then(Script::new(session()));
        let (sent, outcomes) = drive(plan, vec![ok(), rejected(), ok(), ok()]);
        assert_eq!(sent.len(), 4);
        assert_eq!(
            outcomes,
            [
                Outcome::Failed(TransportError::HttpStatus(500)),
                Outcome::Delivered(Response::new(200)),
            ]
        );
    }

    #[test]
    fn a_failed_setup_ends_its_sequence_but_not_the_plan() {
        let plan =
            Plan::requests(vec![request("https://after.example")]).then(Script::new(session()));
        let (sent, outcomes) = drive(plan, vec![ok(), rejected()]);
        assert_eq!(sent, ["https://after.example", "https://example.com/login"]);
        assert_eq!(
            outcomes,
            [
                Outcome::Delivered(Response::new(200)),
                Outcome::Failed(TransportError::HttpStatus(500)),
            ]
        );
    }

    #[test]
    fn a_failed_cleanup_is_not_reported() {
        let plan = Plan::requests(Vec::new()).then(Script::new(session()));
        let (_, outcomes) = drive(plan, vec![ok(), ok(), ok(), rejected()]);
        assert_eq!(outcomes.len(), 2);
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Outcome::Delivered(_)))
        );
    }

    #[test]
    fn an_answer_that_cannot_be_used_is_a_failure() {
        let mut script = Script::new(session());
        script.fail_after = Some(1);
        let plan = Plan::requests(Vec::new()).then(script);
        let (sent, outcomes) = drive(plan, Vec::new());
        assert_eq!(sent, ["https://example.com/login"]);
        assert_eq!(outcomes, [Outcome::Failed(TransportError::InvalidResponse)]);
    }

    #[test]
    fn a_sequence_that_only_prepared_has_nothing_to_send() {
        let plan = Plan::requests(Vec::new()).then(Script::new(vec![Step::setup(request(
            "https://example.com/lookup",
        ))]));
        let (_, outcomes) = drive(plan, Vec::new());
        assert_eq!(outcomes, [Outcome::NothingToSend]);
    }

    fn says_ok(response: &Response) -> bool {
        response.body.expose() == b"ok"
    }

    #[test]
    fn a_check_turns_an_answer_into_a_rejection() {
        let plan = Plan::checked(
            vec![request("https://a.example"), request("https://b.example")],
            Some(says_ok),
        );
        let answers = vec![
            Ok(Response::with_body(200, "ok")),
            Ok(Response::with_body(200, "no")),
        ];
        let (_, outcomes) = drive(plan, answers);
        assert_eq!(
            outcomes,
            [
                Outcome::Delivered(Response::with_body(200, "ok")),
                Outcome::Failed(TransportError::Rejected),
            ]
        );
    }

    #[test]
    fn a_step_can_have_its_answer_checked() {
        let steps = vec![
            Step::setup(request("https://example.com/lookup")).checked(Some(says_ok)),
            Step::delivery(request("https://example.com/post")),
        ];
        let plan = Plan::requests(Vec::new()).then(Script::new(steps));
        let (sent, outcomes) = drive(plan, vec![Ok(Response::with_body(200, "no"))]);
        assert_eq!(sent, ["https://example.com/lookup"]);
        assert_eq!(outcomes, [Outcome::Failed(TransportError::Rejected)]);
    }

    #[test]
    fn a_planned_failure_is_an_outcome_in_its_place() {
        let plan = Plan::requests(vec![request("https://a.example")])
            .failed(TransportError::InvalidRequest);
        let (sent, outcomes) = drive(plan, Vec::new());
        assert_eq!(sent, ["https://a.example"]);
        assert_eq!(
            outcomes,
            [
                Outcome::Delivered(Response::new(200)),
                Outcome::Failed(TransportError::InvalidRequest),
            ]
        );
        let only_failure = Plan::requests(Vec::new()).failed(TransportError::InvalidRequest);
        assert_eq!(
            only_failure.finish(),
            [Outcome::Failed(TransportError::InvalidRequest)]
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
