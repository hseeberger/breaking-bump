use crate::domain::bot::Bot;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone)]
pub(crate) struct PullRequest {
    pub(crate) author: String,
    pub(crate) title: String,
    pub(crate) branch: String,
    pub(crate) head: String,
    pub(crate) head_repository: Option<String>,
    pub(crate) base_repository: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Commit {
    pub(crate) message: String,
    pub(crate) tree: String,
    pub(crate) parents: Vec<String>,
    pub(crate) files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum Refusal {
    #[error("the pull request is from {0}, not from a supported bot")]
    NotFromBot(String),

    #[error("the pull request comes from a fork")]
    FromFork,

    #[error("the examined commit has {0} parents, not one")]
    NotSingleParent(usize),

    #[error("the examined commit changes no Cargo.toml")]
    NoManifest,

    #[error("the commit subject is not conventional")]
    SubjectNotConventional,

    #[error("the commit subject already has a `!`")]
    SubjectMarked,

    #[error("the pull request title is not conventional")]
    TitleNotConventional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Preconditions {
    Met {
        message: String,
        parent: String,
        title: Option<String>,
    },
    Moved,
    Refused(Refusal),
}

pub(crate) fn preconditions<B>(
    bot: &B,
    pull_request: &PullRequest,
    commit: &Commit,
    examined: &str,
) -> Preconditions
where
    B: Bot,
{
    check(bot, pull_request, commit, examined).unwrap_or_else(Preconditions::Refused)
}

fn check<B>(
    bot: &B,
    pull_request: &PullRequest,
    commit: &Commit,
    examined: &str,
) -> Result<Preconditions, Refusal>
where
    B: Bot,
{
    if !bot.is_author(&pull_request.author) {
        return Err(Refusal::NotFromBot(pull_request.author.clone()));
    }
    if pull_request.head_repository.as_deref() != Some(pull_request.base_repository.as_str()) {
        return Err(Refusal::FromFork);
    }
    if pull_request.head != examined {
        return Ok(Preconditions::Moved);
    }
    let [parent] = commit.parents.as_slice() else {
        return Err(Refusal::NotSingleParent(commit.parents.len()));
    };
    let changes_manifest = commit.files.iter().any(|file| {
        Path::new(file)
            .file_name()
            .is_some_and(|name| name == "Cargo.toml")
    });
    if !changes_manifest {
        return Err(Refusal::NoManifest);
    }
    Ok(Preconditions::Met {
        message: bot.marked_message(&commit.message)?,
        parent: parent.clone(),
        title: bot.marked_title(&pull_request.title)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bot::Dependabot,
        pull_request::{Commit, Preconditions, PullRequest, Refusal, preconditions},
    };

    #[test]
    fn met() {
        assert_eq!(
            preconditions(&Dependabot, &pull_request(), &commit(), "head"),
            Preconditions::Met {
                message: "build(deps)!: bump dep".to_owned(),
                parent: "parent".to_owned(),
                title: Some("build(deps)!: bump dep".to_owned()),
            }
        );
    }

    #[test]
    fn moved() {
        assert_eq!(
            preconditions(&Dependabot, &pull_request(), &commit(), "older"),
            Preconditions::Moved
        );
    }

    #[test]
    fn refused() {
        let cases = [
            (
                PullRequest {
                    author: "someone".to_owned(),
                    ..pull_request()
                },
                commit(),
                Refusal::NotFromBot("someone".to_owned()),
            ),
            (
                PullRequest {
                    head_repository: Some("fork/repo".to_owned()),
                    ..pull_request()
                },
                commit(),
                Refusal::FromFork,
            ),
            (
                PullRequest {
                    head_repository: None,
                    ..pull_request()
                },
                commit(),
                Refusal::FromFork,
            ),
            (
                pull_request(),
                Commit {
                    parents: vec!["a".to_owned(), "b".to_owned()],
                    ..commit()
                },
                Refusal::NotSingleParent(2),
            ),
            (
                pull_request(),
                Commit {
                    files: vec!["README.md".to_owned()],
                    ..commit()
                },
                Refusal::NoManifest,
            ),
            (
                pull_request(),
                Commit {
                    message: "build(deps)!: bump dep".to_owned(),
                    ..commit()
                },
                Refusal::SubjectMarked,
            ),
            (
                PullRequest {
                    title: "Bump dep".to_owned(),
                    ..pull_request()
                },
                commit(),
                Refusal::TitleNotConventional,
            ),
        ];
        for (pull_request, commit, refusal) in cases {
            assert_eq!(
                preconditions(&Dependabot, &pull_request, &commit, "head"),
                Preconditions::Refused(refusal)
            );
        }
    }

    fn pull_request() -> PullRequest {
        PullRequest {
            author: "dependabot[bot]".to_owned(),
            title: "build(deps): bump dep".to_owned(),
            branch: "dependabot/cargo/dep-2.0.0".to_owned(),
            head: "head".to_owned(),
            head_repository: Some("owner/repo".to_owned()),
            base_repository: "owner/repo".to_owned(),
        }
    }

    fn commit() -> Commit {
        Commit {
            message: "build(deps): bump dep".to_owned(),
            tree: "tree".to_owned(),
            parents: vec!["parent".to_owned()],
            files: vec!["sub/Cargo.toml".to_owned(), "Cargo.lock".to_owned()],
        }
    }
}
