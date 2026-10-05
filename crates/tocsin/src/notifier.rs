//! Sending one notification to many services.

use crate::{
    AsyncTransport, Notification, ParseError, Plan, Response, Service, Transport, TransportError,
};

/// A set of services that all receive the same notification.
#[derive(Clone, Debug, Default)]
pub struct Notifier {
    services: Vec<Service>,
}

impl Notifier {
    /// An empty notifier.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse `url` and add the service.
    ///
    /// # Errors
    ///
    /// See [`Service::parse`].
    pub fn add(&mut self, url: &str) -> Result<&mut Self, ParseError> {
        self.services.push(Service::parse(url)?);
        Ok(self)
    }

    /// Add an already parsed service.
    pub fn add_service(&mut self, service: Service) -> &mut Self {
        self.services.push(service);
        self
    }

    /// The services, in the order they were added.
    #[must_use]
    pub fn services(&self) -> &[Service] {
        &self.services
    }

    /// Send `notification` to every service with `transport`.
    ///
    /// Every service is attempted even when an earlier one failed, one after
    /// the other. Nothing is retried.
    pub fn send<T: Transport>(&self, notification: &Notification, transport: &mut T) -> Report {
        let mut receipts = Vec::new();
        for service in &self.services {
            let mut plan = service.plan(notification);
            while let Some(request) = plan.next_request() {
                plan.report(transport.send(&request));
            }
            receipts.extend(receipts_of(service, plan));
        }
        Report { receipts }
    }

    /// [`send`](Self::send) with a transport that does not block.
    ///
    /// The services are still taken one after the other. To send to several at
    /// once, give each service its own [`Notifier`] and join the futures.
    pub async fn send_async<T: AsyncTransport>(
        &self,
        notification: &Notification,
        transport: &mut T,
    ) -> Report {
        let mut receipts = Vec::new();
        for service in &self.services {
            let mut plan = service.plan(notification);
            while let Some(request) = plan.next_request() {
                plan.report(transport.send(&request).await);
            }
            receipts.extend(receipts_of(service, plan));
        }
        Report { receipts }
    }
}

fn receipts_of(service: &Service, plan: Plan) -> impl Iterator<Item = Receipt> {
    let name = service.name();
    plan.finish().into_iter().map(move |outcome| Receipt {
        service: name,
        outcome,
    })
}

/// How one request ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The service accepted the request.
    Delivered(Response),
    /// The request failed.
    Failed(TransportError),
    /// The URL is valid but produced no request, for example a Telegram URL
    /// without a chat id.
    NothingToSend,
}

/// The result for one request of one service.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Receipt {
    /// The service name, such as `"telegram"`.
    pub service: &'static str,
    /// What happened.
    pub outcome: Outcome,
}

/// The results of [`Notifier::send`], in request order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    receipts: Vec<Receipt>,
}

impl Report {
    /// Every receipt.
    #[must_use]
    pub fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    /// Whether there is at least one receipt and all of them are deliveries.
    #[must_use]
    pub fn is_success(&self) -> bool {
        !self.receipts.is_empty()
            && self
                .receipts
                .iter()
                .all(|r| matches!(r.outcome, Outcome::Delivered(_)))
    }

    /// The receipts that are not deliveries.
    pub fn failures(&self) -> impl Iterator<Item = &Receipt> {
        self.receipts
            .iter()
            .filter(|r| !matches!(r.outcome, Outcome::Delivered(_)))
    }
}
