use semver::{Version, VersionReq};
use std::{fmt, path::PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub(crate) members: Vec<Package>,
    pub(crate) metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub(crate) struct Package {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) library: Option<String>,
    pub(crate) published: bool,
    pub(crate) manifest: PathBuf,
    pub(crate) dependencies: Vec<Dependency>,
}

impl Package {
    pub(crate) fn is_published_library(&self) -> bool {
        self.published && self.library.is_some()
    }

    pub(crate) fn declarations(&self, name: &str) -> impl Iterator<Item = &Dependency> {
        self.dependencies
            .iter()
            .filter(move |dependency| dependency.name == name)
    }

    pub(crate) fn location(&self, name: &str) -> Location {
        if self
            .declarations(name)
            .any(|dependency| dependency.location == Location::Inherited)
        {
            Location::Inherited
        } else {
            Location::Own
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Dependency {
    pub(crate) name: String,
    pub(crate) requirement: VersionReq,
    pub(crate) location: Location,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Location {
    Own,
    Inherited,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Own => f.write_str("own"),
            Location::Inherited => f.write_str("inherited"),
        }
    }
}
