use crate::{
    application::check::{Repository, Worktree},
    domain::package::is_manifest,
};
use anyhow::{Context, bail};
use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};
use tempfile::TempDir;

pub(crate) struct GitRepository {
    directory: PathBuf,
    prefix: PathBuf,
}

impl GitRepository {
    pub(crate) fn open(directory: &Path) -> anyhow::Result<Self> {
        let prefix = git(directory, &["rev-parse", "--show-prefix"], &[])?;
        Ok(Self {
            directory: directory.to_path_buf(),
            prefix: PathBuf::from(prefix),
        })
    }
}

impl Repository for GitRepository {
    type Worktree = GitWorktree;

    fn commit(&self, revision: &str) -> anyhow::Result<String> {
        let revision = format!("{revision}^{{commit}}");
        Ok(git(
            &self.directory,
            &["rev-parse", "--verify", &revision],
            &[],
        )?)
    }

    fn default_base(&self) -> anyhow::Result<Option<String>> {
        let base = git(
            &self.directory,
            &["rev-parse", "--verify", "--quiet", "origin/HEAD^{commit}"],
            &[],
        );
        match base {
            Ok(base) => Ok(Some(base)),
            Err(error) if error.is_silent() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn merge_base(&self, base: &str, head: &str) -> anyhow::Result<String> {
        Ok(git(&self.directory, &["merge-base", base, head], &[])?)
    }

    fn changed_paths(&self, from: &str, to: &str) -> anyhow::Result<Vec<String>> {
        let paths = git(&self.directory, &["diff", "--name-only", from, to], &[])?;
        Ok(paths.lines().map(str::to_owned).collect())
    }

    fn dirty_manifests(&self) -> anyhow::Result<Vec<String>> {
        let status = git(
            &self.directory,
            &["status", "--porcelain", "--untracked-files=all"],
            &[],
        )?;
        Ok(status
            .lines()
            .filter_map(|line| line.get(3..))
            .map(|path| path.rsplit(" -> ").next().unwrap_or(path))
            .filter(|path| is_manifest(Path::new(path)))
            .map(str::to_owned)
            .collect())
    }

    fn worktree(&self, commit: &str) -> anyhow::Result<GitWorktree> {
        let directory = tempfile::Builder::new()
            .prefix("breaking-bump-")
            .tempdir()
            .context("cannot create a temporary directory")?;
        let path = directory
            .path()
            .to_str()
            .context("temporary path is no UTF-8")?;
        git(
            &self.directory,
            &["worktree", "add", "--quiet", "--detach", path, commit],
            &[],
        )?;
        let root = fs::canonicalize(directory.path())?;
        Ok(GitWorktree {
            repository: self.directory.clone(),
            workspace: root.join(&self.prefix),
            directory,
        })
    }

    fn messages(&self, from: &str, to: &str, path: &Path) -> anyhow::Result<Vec<String>> {
        let range = format!("{from}..{to}");
        let path = path.to_str().context("manifest path is no UTF-8")?;
        let log = git(
            &self.directory,
            &[
                "log",
                "--full-history",
                "--format=%B%x00",
                &range,
                "--",
                path,
            ],
            &[],
        )?;
        Ok(log
            .split('\0')
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
            .collect())
    }
}

pub(crate) struct GitWorktree {
    repository: PathBuf,
    workspace: PathBuf,
    directory: TempDir,
}

impl Worktree for GitWorktree {
    fn workspace(&self) -> &Path {
        &self.workspace
    }
}

impl Drop for GitWorktree {
    fn drop(&mut self) {
        if let Some(path) = self.directory.path().to_str()
            && let Err(error) = git(
                &self.repository,
                &["worktree", "remove", "--force", path],
                &[],
            )
        {
            eprintln!("warning: cannot remove the worktree {path}: {error}");
        }
    }
}

#[derive(Debug)]
pub(crate) struct GitError {
    command: String,
    status: Option<ExitStatus>,
    stderr: String,
}

impl GitError {
    fn is_silent(&self) -> bool {
        self.status.and_then(|status| status.code()) == Some(1) && self.stderr.is_empty()
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git {} failed", self.command)?;
        if let Some(status) = self.status {
            write!(f, " ({status})")?;
        }
        if !self.stderr.is_empty() {
            write!(f, ": {}", self.stderr)?;
        }
        Ok(())
    }
}

impl Error for GitError {}

pub(crate) fn git(
    directory: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .output();
    let command = args.first().copied().unwrap_or_default().to_owned();
    match output {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()),
        Ok(output) => Err(GitError {
            command,
            status: Some(output.status),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
        Err(error) => Err(GitError {
            command,
            status: None,
            stderr: error.to_string(),
        }),
    }
}

pub(crate) fn authorized_env(header: &str) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        ("GIT_CONFIG_KEY_0".to_owned(), "http.extraHeader".to_owned()),
        ("GIT_CONFIG_VALUE_0".to_owned(), header.to_owned()),
    ]
}

pub(crate) fn push_with_lease(
    git_dir: &Path,
    url: &str,
    branch: &str,
    expected: &str,
    new: &str,
    env: &[(String, String)],
) -> anyhow::Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(["push", "--porcelain"])
        .arg(format!("--force-with-lease=refs/heads/{branch}:{expected}"))
        .arg(url)
        .arg(format!("{new}:refs/heads/{branch}"))
        .envs(env.iter().map(|(key, value)| (key, value)))
        .output()
        .context("cannot run git push")?;
    if output.status.success() {
        return Ok(true);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("(stale info)") || stderr.contains("(stale info)") {
        return Ok(false);
    }
    bail!("git push failed: {}{}", stdout.trim(), stderr.trim())
}

#[cfg(test)]
mod tests {
    use crate::{
        application::check::Repository,
        infrastructure::git::{GitRepository, authorized_env, git, push_with_lease},
    };
    use std::{
        fs,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    struct Lease {
        remote: TempDir,
        work: TempDir,
        examined: String,
        marked: String,
    }

    impl Lease {
        fn new() -> Self {
            let remote = TempDir::new().unwrap();
            git(remote.path(), &["init", "--quiet", "--bare"], &[]).unwrap();
            let work = TempDir::new().unwrap();
            let path = work.path();
            git(
                path,
                &["init", "--quiet", "--initial-branch", "branch"],
                &[],
            )
            .unwrap();
            git(path, &["config", "user.email", "test@example.com"], &[]).unwrap();
            git(path, &["config", "user.name", "test"], &[]).unwrap();
            git(path, &["config", "commit.gpgsign", "false"], &[]).unwrap();
            let parent = commit(path, "feat: initial");
            let examined = commit(path, "build(deps): bump x");
            push(path, remote.path());
            let marked = git(
                path,
                &[
                    "commit-tree",
                    "HEAD^{tree}",
                    "-p",
                    &parent,
                    "-m",
                    "build(deps)!: bump x",
                ],
                &[],
            )
            .unwrap();
            Self {
                remote,
                work,
                examined,
                marked,
            }
        }

        fn url(&self) -> &str {
            self.remote.path().to_str().unwrap()
        }

        fn remote_branch(&self) -> String {
            git(self.remote.path(), &["rev-parse", "refs/heads/branch"], &[]).unwrap()
        }
    }

    #[test]
    fn default_base_is_none_without_origin_head() {
        let directory = repository();
        let repository = GitRepository::open(directory.path()).unwrap();
        assert_eq!(repository.default_base().unwrap(), None);
    }

    #[test]
    fn default_base_is_origin_head() {
        let directory = repository();
        let head = git(directory.path(), &["rev-parse", "HEAD"], &[]).unwrap();
        git(
            directory.path(),
            &["update-ref", "refs/remotes/origin/HEAD", &head],
            &[],
        )
        .unwrap();
        let repository = GitRepository::open(directory.path()).unwrap();
        assert_eq!(repository.default_base().unwrap(), Some(head));
    }

    #[test]
    fn default_base_is_an_error_when_origin_head_is_no_commit() {
        let directory = repository();
        let tree = git(directory.path(), &["rev-parse", "HEAD^{tree}"], &[]).unwrap();
        git(
            directory.path(),
            &["update-ref", "refs/remotes/origin/HEAD", &tree],
            &[],
        )
        .unwrap();
        let repository = GitRepository::open(directory.path()).unwrap();
        let error = repository.default_base().unwrap_err().to_string();
        assert!(error.contains("git rev-parse failed"), "{error}");
    }

    #[test]
    fn default_base_is_an_error_outside_a_repository() {
        let directory = TempDir::new().unwrap();
        let repository = GitRepository {
            directory: directory.path().to_path_buf(),
            prefix: PathBuf::new(),
        };
        let error = repository.default_base().unwrap_err().to_string();
        assert!(error.contains("exit status: 128"), "{error}");
    }

    #[test]
    fn push_when_branch_unchanged() {
        let lease = Lease::new();
        let pushed = push_with_lease(
            lease.work.path(),
            lease.url(),
            "branch",
            &lease.examined,
            &lease.marked,
            &[],
        )
        .unwrap();
        assert!(pushed);
        assert_eq!(lease.remote_branch(), lease.marked);
    }

    #[test]
    fn push_rejected_when_branch_moved() {
        let lease = Lease::new();
        let newer = commit(lease.work.path(), "build(deps): bump x again");
        push(lease.work.path(), lease.remote.path());
        let pushed = push_with_lease(
            lease.work.path(),
            lease.url(),
            "branch",
            &lease.examined,
            &lease.marked,
            &[],
        )
        .unwrap();
        assert!(!pushed);
        assert_eq!(lease.remote_branch(), newer);
    }

    #[test]
    fn push_fails_when_remote_refuses() {
        let lease = Lease::new();
        let hook = lease.remote.path().join("hooks").join("pre-receive");
        fs::write(&hook, "#!/bin/sh\necho policy >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::{fs::Permissions, os::unix::fs::PermissionsExt};
            fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
        }
        let result = push_with_lease(
            lease.work.path(),
            lease.url(),
            "branch",
            &lease.examined,
            &lease.marked,
            &[],
        );
        assert!(result.is_err());
        assert_eq!(lease.remote_branch(), lease.examined);
    }

    #[test]
    fn git_error_does_not_contain_credentials() {
        let directory = TempDir::new().unwrap();
        git(directory.path(), &["init", "--quiet", "--bare"], &[]).unwrap();
        let env = authorized_env("Authorization: Basic c2VjcmV0LXRva2Vu");
        let missing = directory.path().join("missing.git");
        let error = git(
            directory.path(),
            &["fetch", "--quiet", missing.to_str().unwrap(), "main"],
            &env,
        )
        .unwrap_err();
        assert!(!error.to_string().contains("c2VjcmV0LXRva2Vu"));
    }

    fn repository() -> TempDir {
        let directory = TempDir::new().unwrap();
        let path = directory.path();
        git(path, &["init", "--quiet", "--initial-branch", "main"], &[]).unwrap();
        git(path, &["config", "user.email", "test@example.com"], &[]).unwrap();
        git(path, &["config", "user.name", "test"], &[]).unwrap();
        git(path, &["config", "commit.gpgsign", "false"], &[]).unwrap();
        commit(path, "feat: initial");
        directory
    }

    fn commit(path: &Path, message: &str) -> String {
        git(
            path,
            &["commit", "--quiet", "--allow-empty", "--message", message],
            &[],
        )
        .unwrap();
        git(path, &["rev-parse", "HEAD"], &[]).unwrap()
    }

    fn push(path: &Path, remote: &Path) {
        git(
            path,
            &["push", "--quiet", remote.to_str().unwrap(), "branch"],
            &[],
        )
        .unwrap();
    }
}
