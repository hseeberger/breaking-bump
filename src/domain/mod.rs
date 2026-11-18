pub(crate) mod bot;
pub(crate) mod bump;
pub(crate) mod changes;
pub(crate) mod compatibility;
pub(crate) mod finding;
pub(crate) mod marker;
pub(crate) mod package;
pub(crate) mod pull_request;
pub(crate) mod reference;
pub(crate) mod rules;
pub(crate) mod settings;

use thiserror::Error;

#[derive(Debug, Error)]
#[error("{0}")]
pub(crate) struct Undecided(pub(crate) String);
