use crate::{application::preflight::Toolchain, infrastructure::rustdoc};
use anyhow::{Context, bail};
use std::{fs, process::Command};

pub(crate) struct ActiveToolchain;

impl Toolchain for ActiveToolchain {
    fn release(&self) -> anyhow::Result<String> {
        let output = Command::new("rustc")
            .arg("-vV")
            .output()
            .context("cannot run rustc")?;
        if !output.status.success() {
            bail!(
                "rustc -vV failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let version = String::from_utf8(output.stdout)?;
        version
            .lines()
            .find_map(|line| line.strip_prefix("release: "))
            .map(str::to_owned)
            .context("rustc -vV reports no release")
    }

    fn format_version(&self) -> anyhow::Result<u32> {
        let directory = tempfile::Builder::new()
            .prefix("breaking-bump-probe-")
            .tempdir()?;
        let source = directory.path().join("lib.rs");
        fs::write(&source, "pub struct Probe;\n")?;
        let output = Command::new("rustdoc")
            .args(["-Zunstable-options", "--output-format", "json"])
            .args([
                "--crate-name",
                "probe",
                "--crate-type",
                "lib",
                "--edition",
                "2021",
            ])
            .arg("-o")
            .arg(directory.path())
            .arg(&source)
            .output()
            .context("cannot run rustdoc")?;
        if !output.status.success() {
            bail!(
                "rustdoc cannot generate JSON: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        rustdoc::format_version(&directory.path().join("probe.json"))
    }
}
