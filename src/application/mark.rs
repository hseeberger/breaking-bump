use crate::domain::{
    bot::Bot,
    pull_request::{Commit, Preconditions, PullRequest, Refusal, preconditions},
};

pub(crate) trait Forge {
    fn pull_request(&self, number: u64) -> anyhow::Result<PullRequest>;

    fn commit(&self, sha: &str) -> anyhow::Result<Commit>;

    // Creates the commit without author and committer, so that the forge signs it.
    fn create_commit(&self, message: &str, tree: &str, parent: &str) -> anyhow::Result<String>;

    fn set_title(&self, number: u64, title: &str) -> anyhow::Result<()>;

    // Returns false when the branch no longer points at `expected`.
    fn move_branch(&self, branch: &str, expected: &str, new: &str) -> anyhow::Result<bool>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Marking {
    Marked { branch: String, commit: String },
    Moved { title_marked: bool },
    Refused(Refusal),
}

pub(crate) fn mark<F, B>(forge: &F, bot: &B, number: u64, examined: &str) -> anyhow::Result<Marking>
where
    F: Forge,
    B: Bot,
{
    let pull_request = forge.pull_request(number)?;
    let commit = forge.commit(examined)?;
    match preconditions(bot, &pull_request, &commit, examined) {
        Preconditions::Refused(refusal) => Ok(Marking::Refused(refusal)),
        Preconditions::Moved => Ok(Marking::Moved {
            title_marked: false,
        }),
        Preconditions::Met {
            message,
            parent,
            title,
        } => {
            let new = forge.create_commit(&message, &commit.tree, &parent)?;
            if let Some(title) = title {
                forge.set_title(number, &title)?;
            }
            if forge.move_branch(&pull_request.branch, examined, &new)? {
                Ok(Marking::Marked {
                    branch: pull_request.branch,
                    commit: new,
                })
            } else {
                Ok(Marking::Moved { title_marked: true })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::mark::{Forge, Marking, mark},
        domain::{
            bot::Dependabot,
            pull_request::{Commit, PullRequest, Refusal},
        },
    };
    use anyhow::bail;
    use std::cell::RefCell;

    #[test]
    fn marks_commit_and_title() {
        let forge = FakeForge::new("build(deps): bump dep");
        let marking = mark(&forge, &Dependabot, 7, "head").unwrap();
        assert_eq!(
            marking,
            Marking::Marked {
                branch: "dependabot/cargo/dep".to_owned(),
                commit: "new".to_owned()
            }
        );
        assert_eq!(
            *forge.calls.borrow(),
            [
                "create build(deps)!: bump dep\n\nbody tree parent",
                "title build(deps)!: bump dep",
                "move dependabot/cargo/dep head new",
            ]
        );
    }

    #[test]
    fn a_retry_with_a_marked_title_finishes_the_push() {
        let forge = FakeForge::new("build(deps)!: bump dep");
        let marking = mark(&forge, &Dependabot, 7, "head").unwrap();
        assert!(matches!(marking, Marking::Marked { .. }));
        assert_eq!(
            *forge.calls.borrow(),
            [
                "create build(deps)!: bump dep\n\nbody tree parent",
                "move dependabot/cargo/dep head new",
            ]
        );
    }

    #[test]
    fn a_branch_that_moves_after_the_title_edit_is_left_alone() {
        let mut forge = FakeForge::new("build(deps): bump dep");
        forge.branch_moved = true;
        let marking = mark(&forge, &Dependabot, 7, "head").unwrap();
        assert_eq!(marking, Marking::Moved { title_marked: true });
    }

    #[test]
    fn a_failed_title_edit_leaves_the_branch_alone() {
        let mut forge = FakeForge::new("build(deps): bump dep");
        forge.title_fails = true;
        assert!(mark(&forge, &Dependabot, 7, "head").is_err());
        assert!(
            !forge
                .calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("move"))
        );
    }

    #[test]
    fn a_moved_head_changes_nothing() {
        let forge = FakeForge::new("build(deps): bump dep");
        let marking = mark(&forge, &Dependabot, 7, "older").unwrap();
        assert_eq!(
            marking,
            Marking::Moved {
                title_marked: false
            }
        );
        assert!(forge.calls.borrow().is_empty());
    }

    #[test]
    fn a_refusal_changes_nothing() {
        let mut forge = FakeForge::new("build(deps): bump dep");
        forge.author = "someone";
        let marking = mark(&forge, &Dependabot, 7, "head").unwrap();
        assert_eq!(
            marking,
            Marking::Refused(Refusal::NotFromBot("someone".to_owned()))
        );
        assert!(forge.calls.borrow().is_empty());
    }

    struct FakeForge {
        author: &'static str,
        title: &'static str,
        branch_moved: bool,
        title_fails: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeForge {
        fn new(title: &'static str) -> Self {
            Self {
                author: "dependabot[bot]",
                title,
                branch_moved: false,
                title_fails: false,
                calls: RefCell::new(vec![]),
            }
        }
    }

    impl Forge for FakeForge {
        fn pull_request(&self, _number: u64) -> anyhow::Result<PullRequest> {
            Ok(PullRequest {
                author: self.author.to_owned(),
                title: self.title.to_owned(),
                branch: "dependabot/cargo/dep".to_owned(),
                head: "head".to_owned(),
                head_repository: Some("owner/repo".to_owned()),
                base_repository: "owner/repo".to_owned(),
            })
        }

        fn commit(&self, _sha: &str) -> anyhow::Result<Commit> {
            Ok(Commit {
                message: "build(deps): bump dep\n\nbody".to_owned(),
                tree: "tree".to_owned(),
                parents: vec!["parent".to_owned()],
                files: vec!["Cargo.toml".to_owned()],
            })
        }

        fn create_commit(&self, message: &str, tree: &str, parent: &str) -> anyhow::Result<String> {
            self.calls
                .borrow_mut()
                .push(format!("create {message} {tree} {parent}"));
            Ok("new".to_owned())
        }

        fn set_title(&self, _number: u64, title: &str) -> anyhow::Result<()> {
            if self.title_fails {
                bail!("403 Resource not accessible by integration");
            }
            self.calls.borrow_mut().push(format!("title {title}"));
            Ok(())
        }

        fn move_branch(&self, branch: &str, expected: &str, new: &str) -> anyhow::Result<bool> {
            self.calls
                .borrow_mut()
                .push(format!("move {branch} {expected} {new}"));
            Ok(!self.branch_moved)
        }
    }
}
