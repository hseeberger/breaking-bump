pub(crate) mod check;
pub(crate) mod exposed;
pub(crate) mod mark;
pub(crate) mod preflight;

use crate::domain::{
    package::{Package, Workspace},
    reference::ApiItems,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(crate) trait Cargo {
    type Snapshot: Snapshot;

    fn workspace(&self, directory: &Path) -> anyhow::Result<Workspace>;

    fn snapshot(&self, workspace_root: &Path, target_dir: &Path) -> anyhow::Result<Self::Snapshot>;
}

pub(crate) trait Snapshot {
    fn libraries(&self) -> &[Package];

    // Fails with `Undecided` when a crate cannot be mapped to a package or site names collide.
    fn api_items(&self, library: &Package) -> anyhow::Result<ApiItems>;

    // Fails with `Undecided` when a crate cannot be mapped to a package.
    fn package_grouped_foreign_paths(
        &self,
        library: &Package,
    ) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>>;

    fn direct_dependencies(&self, package_id: &str) -> Vec<String>;

    // Includes the package itself at distance 0.
    fn distances(&self, package_id: &str) -> BTreeMap<String, usize>;

    fn package_name(&self, package_id: &str) -> &str;

    fn package_version(&self, package_id: &str) -> String;
}

pub(crate) fn default_target_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join("target").join("breaking-bump")
}
