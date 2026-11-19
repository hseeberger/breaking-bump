use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub(crate) type ApiItems = BTreeMap<Reference, BTreeSet<DefiningPackage>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Reference {
    pub(crate) site: String,
    pub(crate) position: String,
    pub(crate) path: String,
    pub(crate) kind: String,
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` in `{}`", self.path, self.site)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DefiningPackage {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: Version,
}
