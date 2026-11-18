use thiserror::Error;

pub(crate) trait Toolchain {
    fn release(&self) -> anyhow::Result<String>;

    fn format_version(&self) -> anyhow::Result<u32>;
}

#[derive(Debug, Error)]
pub(crate) enum PreflightError {
    #[error(
        "breaking-bump needs a nightly toolchain, but runs on {0}; run it as \
         `cargo +nightly-YYYY-MM-DD breaking-bump ...`"
    )]
    NotNightly(String),

    #[error(
        "the toolchain emits rustdoc JSON format {actual}, but this version of breaking-bump \
         supports format {supported}"
    )]
    Format { actual: u32, supported: u32 },
}

pub(crate) fn preflight<T>(toolchain: &T, supported: u32) -> anyhow::Result<()>
where
    T: Toolchain,
{
    let release = toolchain.release()?;
    if !release.contains("-nightly") {
        return Err(PreflightError::NotNightly(release).into());
    }
    let actual = toolchain.format_version()?;
    if actual != supported {
        return Err(PreflightError::Format { actual, supported }.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::application::preflight::{PreflightError, Toolchain, preflight};

    struct Fixed(&'static str, u32);

    impl Toolchain for Fixed {
        fn release(&self) -> anyhow::Result<String> {
            Ok(self.0.to_owned())
        }

        fn format_version(&self) -> anyhow::Result<u32> {
            Ok(self.1)
        }
    }

    #[test]
    fn nightly_with_the_supported_format() {
        assert!(preflight(&Fixed("1.101.0-nightly", 61), 61).is_ok());
    }

    #[test]
    fn stable_is_rejected() {
        let error = preflight(&Fixed("1.100.0", 61), 61).unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(PreflightError::NotNightly(_))
        ));
    }

    #[test]
    fn another_format_is_rejected() {
        let error = preflight(&Fixed("1.101.0-nightly", 62), 61).unwrap_err();
        assert!(error.to_string().contains("62"));
        assert!(error.to_string().contains("61"));
    }
}
