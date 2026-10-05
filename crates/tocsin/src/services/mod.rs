//! The supported notification services, one module each.
//!
//! To add a service, see `CONTRIBUTING.md`: a module with a `parse` function
//! and a `prepare` method, a scheme in [`Service::parse`](crate::Service::parse),
//! a feature in `Cargo.toml`, fixtures for the Apprise oracle, and golden
//! tests for the request.

#[cfg(feature = "bark")]
pub(crate) mod bark;
#[cfg(feature = "discord")]
pub(crate) mod discord;
#[cfg(feature = "form")]
pub(crate) mod form;
#[cfg(feature = "gchat")]
pub(crate) mod gchat;
#[cfg(feature = "gotify")]
pub(crate) mod gotify;
#[cfg(feature = "ifttt")]
pub(crate) mod ifttt;
#[cfg(feature = "json")]
pub(crate) mod json;
#[cfg(feature = "mattermost")]
pub(crate) mod mattermost;
#[cfg(feature = "ntfy")]
pub(crate) mod ntfy;
#[cfg(feature = "pagerduty")]
pub(crate) mod pagerduty;
#[cfg(feature = "prowl")]
pub(crate) mod prowl;
#[cfg(feature = "pushbullet")]
pub(crate) mod pushbullet;
#[cfg(feature = "pushover")]
pub(crate) mod pushover;
#[cfg(feature = "rocketchat")]
pub(crate) mod rocketchat;
#[cfg(feature = "slack")]
pub(crate) mod slack;
#[cfg(feature = "telegram")]
pub(crate) mod telegram;
#[cfg(feature = "_webhook")]
pub(crate) mod webhook;
#[cfg(feature = "workflows")]
pub(crate) mod workflows;
#[cfg(feature = "xml")]
pub(crate) mod xml;
#[cfg(feature = "zulip")]
pub(crate) mod zulip;
