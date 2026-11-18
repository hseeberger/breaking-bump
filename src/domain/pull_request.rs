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
pub(crate) enum PullRequestState {
    Ready,
    Moved,
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marking {
    pub(crate) message: String,
    pub(crate) parent: String,
    pub(crate) title: Option<String>,
}

pub(crate) fn pull_request_state<B>(
    bot: &B,
    pull_request: &PullRequest,
    examined: &str,
) -> PullRequestState
where
    B: Bot,
{
    if !bot.is_author(&pull_request.author) {
        return PullRequestState::Refused(Refusal::NotFromBot(pull_request.author.clone()));
    }
    if pull_request.head_repository.as_deref() != Some(pull_request.base_repository.as_str()) {
        return PullRequestState::Refused(Refusal::FromFork);
    }
    if pull_request.head != examined {
        return PullRequestState::Moved;
    }
    PullRequestState::Ready
}

pub(crate) fn marking<B>(
    bot: &B,
    pull_request: &PullRequest,
    commit: &Commit,
) -> Result<Marking, Refusal>
where
    B: Bot,
{
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
    Ok(Marking {
        message: bot.marked_message(&commit.message)?,
        parent: parent.clone(),
        title: bot.marked_title(&pull_request.title)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bot::Dependabot,
        pull_request::{
            Commit, Marking, PullRequest, PullRequestState, Refusal, marking, pull_request_state,
        },
    };

    #[test]
    fn ready_and_marked() {
        assert_eq!(
            pull_request_state(&Dependabot, &pull_request(), "head"),
            PullRequestState::Ready
        );
        assert_eq!(
            marking(&Dependabot, &pull_request(), &commit()),
            Ok(Marking {
                message: "build(deps)!: bump dep".to_owned(),
                parent: "parent".to_owned(),
                title: Some("build(deps)!: bump dep".to_owned()),
            })
        );
    }

    #[test]
    fn moved() {
        assert_eq!(
            pull_request_state(&Dependabot, &pull_request(), "older"),
            PullRequestState::Moved
        );
    }

    #[test]
    fn refused_pull_requests() {
        let cases = [
            (
                PullRequest {
                    author: "someone".to_owned(),
                    ..pull_request()
                },
                Refusal::NotFromBot("someone".to_owned()),
            ),
            (
                PullRequest {
                    head_repository: Some("fork/repo".to_owned()),
                    ..pull_request()
                },
                Refusal::FromFork,
            ),
            (
                PullRequest {
                    head_repository: None,
                    ..pull_request()
                },
                Refusal::FromFork,
            ),
        ];
        for (pull_request, refusal) in cases {
            assert_eq!(
                pull_request_state(&Dependabot, &pull_request, "head"),
                PullRequestState::Refused(refusal)
            );
        }
    }

    #[test]
    fn refused_commits() {
        let cases = [
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
            assert_eq!(marking(&Dependabot, &pull_request, &commit), Err(refusal));
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
