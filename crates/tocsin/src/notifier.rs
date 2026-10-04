//! Sending one notification to many services.

use crate::{Notification, ParseError, Response, Service, Transport, TransportError};

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
    /// Every request is attempted even when an earlier one failed. Nothing is
    /// retried.
    pub fn send<T: Transport>(&self, notification: &Notification, transport: &mut T) -> Report {
        let mut receipts = Vec::new();
        for service in &self.services {
            let requests = service.prepare(notification);
            if requests.is_empty() {
                receipts.push(Receipt {
                    service: service.name(),
                    outcome: Outcome::NothingToSend,
                });
            }
            for request in &requests {
                receipts.push(Receipt {
                    service: service.name(),
                    outcome: match transport.send(request) {
                        Ok(response) => Outcome::Delivered(response),
                        Err(error) => Outcome::Failed(error),
                    },
                });
            }
        }
        Report { receipts }
    }
}

/// How one request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
