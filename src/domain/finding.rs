use crate::domain::{bump::Bump, package::Location};
use semver::Version;
use std::{collections::BTreeSet, fmt, path::PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct Finding {
    pub(crate) bump: Bump,
    pub(crate) package: String,
    pub(crate) old_versions: BTreeSet<Version>,
    pub(crate) new_versions: BTreeSet<Version>,
    pub(crate) items: Vec<String>,
    pub(crate) location: Location,
    pub(crate) manifest: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Violation {
    Inherited,
    MissingMarker,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::Inherited => f.write_str("inherited"),
            Violation::MissingMarker => f.write_str("missing-marker"),
        }
    }
}
