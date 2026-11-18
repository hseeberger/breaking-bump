#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConventionalSubject<'a> {
    pub(crate) prefix: &'a str,
    pub(crate) marked: bool,
    pub(crate) rest: &'a str,
}

impl<'a> ConventionalSubject<'a> {
    pub(crate) fn parse(subject: &'a str) -> Option<Self> {
        let mut end = subject
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(subject.len());
        if end == 0 {
            return None;
        }
        if subject[end..].starts_with('(') {
            end += subject[end..].find(')')? + 1;
        }
        let prefix = &subject[..end];
        let marked = subject[end..].starts_with('!');
        if marked {
            end += 1;
        }
        let rest = subject[end..].strip_prefix(':')?;
        Some(Self {
            prefix,
            marked,
            rest,
        })
    }

    pub(crate) fn with_marker(&self) -> String {
        format!("{}!:{}", self.prefix, self.rest)
    }
}

pub(crate) fn is_marked(message: &str) -> bool {
    let subject = message.lines().next().unwrap_or_default();
    let marked_subject = ConventionalSubject::parse(subject).is_some_and(|subject| subject.marked);
    marked_subject
        || message.lines().any(|line| {
            line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:")
        })
}

#[cfg(test)]
mod tests {
    use crate::domain::marker::{ConventionalSubject, is_marked};

    #[test]
    fn marked_messages() {
        assert!(is_marked("build(deps)!: bump utoipa"));
        assert!(is_marked("feat!: drop x"));
        assert!(is_marked("build(deps): bump\n\nBREAKING CHANGE: utoipa 6"));
        assert!(is_marked("build(deps): bump\n\nBREAKING-CHANGE: utoipa 6"));
        assert!(!is_marked("build(deps): bump utoipa from 5.5.0 to 6.0.0"));
        assert!(!is_marked("Merge abc into def"));
        assert!(!is_marked(""));
    }

    #[test]
    fn conventional_subjects() {
        let subject = ConventionalSubject::parse("build(deps): bump utoipa").unwrap();
        assert_eq!(subject.prefix, "build(deps)");
        assert!(!subject.marked);
        assert_eq!(subject.with_marker(), "build(deps)!: bump utoipa");

        let subject = ConventionalSubject::parse("fix!: x").unwrap();
        assert_eq!(subject.prefix, "fix");
        assert!(subject.marked);

        for subject in [
            "Bump utoipa",
            "build(deps: x",
            "(deps): x",
            ": x",
            "build deps: x",
        ] {
            assert_eq!(ConventionalSubject::parse(subject), None, "{subject}");
        }
    }
}
