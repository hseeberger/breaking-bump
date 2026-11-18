use crate::domain::{marker::Conventional, pull_request::Refusal};

pub(crate) trait Bot {
    fn is_author(&self, login: &str) -> bool;

    fn marked_message(&self, message: &str) -> Result<String, Refusal>;

    fn marked_title(&self, title: &str) -> Result<Option<String>, Refusal>;
}

pub(crate) struct Dependabot;

impl Bot for Dependabot {
    fn is_author(&self, login: &str) -> bool {
        login == "dependabot[bot]"
    }

    fn marked_message(&self, message: &str) -> Result<String, Refusal> {
        let (subject, body) = message.split_at(message.find('\n').unwrap_or(message.len()));
        let subject = Conventional::parse(subject).ok_or(Refusal::SubjectNotConventional)?;
        if subject.marked {
            return Err(Refusal::SubjectMarked);
        }
        Ok(format!("{}{body}", subject.with_marker()))
    }

    fn marked_title(&self, title: &str) -> Result<Option<String>, Refusal> {
        let title = Conventional::parse(title).ok_or(Refusal::TitleNotConventional)?;
        Ok((!title.marked).then(|| title.with_marker()))
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{
        bot::{Bot, Dependabot},
        pull_request::Refusal,
    };

    #[test]
    fn marked_message_keeps_the_body() {
        assert_eq!(
            Dependabot.marked_message("build(deps): bump utoipa\n\nBody text\n"),
            Ok("build(deps)!: bump utoipa\n\nBody text\n".to_owned())
        );
        assert_eq!(
            Dependabot.marked_message("build(deps)!: bump utoipa"),
            Err(Refusal::SubjectMarked)
        );
        assert_eq!(
            Dependabot.marked_message("Bump utoipa"),
            Err(Refusal::SubjectNotConventional)
        );
    }

    #[test]
    fn marked_title_accepts_a_marked_title() {
        assert_eq!(
            Dependabot.marked_title("build(deps): bump utoipa"),
            Ok(Some("build(deps)!: bump utoipa".to_owned()))
        );
        assert_eq!(
            Dependabot.marked_title("build(deps)!: bump utoipa"),
            Ok(None)
        );
        assert_eq!(
            Dependabot.marked_title("Bump utoipa"),
            Err(Refusal::TitleNotConventional)
        );
    }

    #[test]
    fn recognizes_the_author() {
        assert!(Dependabot.is_author("dependabot[bot]"));
        assert!(!Dependabot.is_author("renovate[bot]"));
    }
}
