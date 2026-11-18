mod application;
mod domain;
mod infrastructure;

use crate::{
    application::{
        check::{self, Judged, Outcome, Report},
        exposed::{self, Audit, Verdict},
        mark::{self, Marking},
        preflight,
    },
    domain::{bot::Dependabot, finding::Violation},
    infrastructure::{
        cargo::CargoCli, git::GitRepository, github::GitHub, rustdoc, toolchain::RustupToolchain,
    },
};
use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use semver::Version;
use std::{
    collections::BTreeSet,
    env,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    let Cli::BreakingBump(breaking_bump) = Cli::parse();
    match breaking_bump.command {
        Command::Check(args) => run_check(args),
        Command::Mark(args) => run_mark(args),
        Command::Exposed(args) => run_exposed(args),
    }
}

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cli {
    #[command(
        name = "breaking-bump",
        version,
        about = "Detect semver-incompatible bumps of dependencies in the public API of Rust crates"
    )]
    BreakingBump(BreakingBump),
}

#[derive(Args)]
struct BreakingBump {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Check the head commit against the merge base of base and head")]
    Check(CheckArgs),

    #[command(about = "Add the breaking marker to a bot pull request")]
    Mark(MarkArgs),

    #[command(about = "List the dependencies in the public API of the published crates")]
    Exposed(ExposedArgs),
}

#[derive(Args)]
struct CheckArgs {
    #[arg(long, help = "Base commit, by default origin/HEAD")]
    base: Option<String>,

    #[arg(long, default_value = "HEAD", help = "Head commit")]
    head: String,
}

#[derive(Args)]
struct MarkArgs {
    #[arg(long, env = "GITHUB_REPOSITORY", help = "Repository as owner/name")]
    repository: String,

    #[arg(long, help = "Pull request number")]
    pr: u64,

    #[arg(long, help = "Head commit that check examined")]
    head: String,
}

#[derive(Args)]
struct ExposedArgs {
    #[arg(
        short,
        long = "package",
        help = "Restrict the audit to this crate, repeatable"
    )]
    packages: Vec<String>,
}

fn run_check(args: CheckArgs) -> ExitCode {
    let report = check(&args);
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    print_report(&report);
    if let Err(error) = write_outputs(&report) {
        eprintln!("error: {error:#}");
        return ExitCode::from(3);
    }
    match &report.outcome {
        Outcome::Compatible(_) => ExitCode::SUCCESS,
        Outcome::Breaking(_) if report.violations().is_empty() => ExitCode::SUCCESS,
        Outcome::Breaking(_) => ExitCode::from(1),
        Outcome::Undecided(_) => ExitCode::from(2),
        Outcome::Error(_) => ExitCode::from(3),
    }
}

fn check(args: &CheckArgs) -> Report {
    let prepared = preflight::preflight(&RustupToolchain, rustdoc::FORMAT_VERSION)
        .and_then(|()| current_dir())
        .and_then(|directory| {
            let repository = GitRepository::open(&directory)?;
            Ok((repository, target_dir(&directory)))
        });
    match prepared {
        Ok((repository, target_dir)) => check::check(
            &repository,
            &CargoCli::new(target_dir),
            args.base.as_deref(),
            &args.head,
        ),
        Err(error) => Report::error(error),
    }
}

fn current_dir() -> anyhow::Result<PathBuf> {
    env::current_dir().context("cannot determine the current directory")
}

fn target_dir(directory: &Path) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(target_dir) => directory.join(target_dir),
        None => directory.join("target").join("breaking-bump"),
    }
}

fn print_report(report: &Report) {
    match &report.outcome {
        Outcome::Compatible(reason) => println!("{reason}"),
        Outcome::Breaking(findings) => {
            for judged in findings {
                print_finding(judged);
            }
        }
        Outcome::Undecided(reason) => println!("cannot decide: {reason}"),
        Outcome::Error(error) => println!("error: {error:#}"),
    }
    println!("result: {}", report.outcome.name());
}

fn print_finding(judged: &Judged) {
    let finding = &judged.finding;
    let bump = &finding.bump;
    let shown = finding
        .items
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = match finding.items.len() {
        count if count > 3 => format!(" and {} more", count - 3),
        _ => String::new(),
    };
    println!(
        "{}: bumping {} from {} to {} ({}) changes {} from {} to {} in the public API: {shown}{more}",
        bump.package,
        bump.dependency,
        bump.old,
        bump.new,
        finding.location,
        finding.package,
        versions(&finding.old_versions),
        versions(&finding.new_versions),
    );
    let manifest = finding.manifest.display();
    for violation in &judged.violations {
        let fix = match violation {
            Violation::MissingMarker => format!(
                "no commit since the merge base that changes {manifest} is marked; mark one with \
                 `!` (`type!:` or `type(scope)!:`) or a `BREAKING CHANGE:` footer"
            ),
            Violation::Inherited => format!(
                "{} is declared with `workspace = true`; declare it in {manifest} itself, so \
                 that the bump changes that file",
                bump.dependency
            ),
        };
        println!("  {violation}: {fix}");
    }
}

fn versions(versions: &BTreeSet<Version>) -> String {
    versions
        .iter()
        .map(Version::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

fn write_outputs(report: &Report) -> anyhow::Result<()> {
    let Some(path) = env::var_os("GITHUB_OUTPUT") else {
        return Ok(());
    };
    let violations = report
        .violations()
        .iter()
        .map(Violation::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("cannot open GITHUB_OUTPUT")?;
    writeln!(file, "result={}", report.outcome.name())?;
    writeln!(file, "violations={violations}")?;
    writeln!(file, "head={}", report.head.as_deref().unwrap_or_default())?;
    Ok(())
}

fn run_mark(args: MarkArgs) -> ExitCode {
    let Some(token) = env::var("GH_TOKEN")
        .or_else(|_| env::var("GITHUB_TOKEN"))
        .ok()
    else {
        println!("error: neither GH_TOKEN nor GITHUB_TOKEN is set");
        return ExitCode::from(3);
    };
    let api = env::var("GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".to_owned());
    let server = env::var("GITHUB_SERVER_URL").unwrap_or_else(|_| "https://github.com".to_owned());
    let forge = GitHub::new(api, server, &args.repository, token);
    match mark::mark(&forge, &Dependabot, args.pr, &args.head) {
        Ok(Marking::Marked { branch, commit }) => {
            println!("marked: {branch} now at {commit}");
            ExitCode::SUCCESS
        }
        Ok(Marking::Moved { title_marked }) => {
            println!(
                "not marking: the pull request head is no longer {}",
                args.head
            );
            if title_marked {
                println!("the pull request title already has the `!`");
            }
            ExitCode::SUCCESS
        }
        Ok(Marking::Refused(refusal)) => {
            println!("not marking: {refusal}");
            ExitCode::from(1)
        }
        Err(error) => {
            println!("error: {error:#}");
            ExitCode::from(3)
        }
    }
}

fn run_exposed(args: ExposedArgs) -> ExitCode {
    match exposed(&args) {
        Ok(audit) => {
            print_audit(&audit);
            if audit.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
        Err(error) => {
            println!("error: {error:#}");
            ExitCode::from(3)
        }
    }
}

fn exposed(args: &ExposedArgs) -> anyhow::Result<Audit> {
    preflight::preflight(&RustupToolchain, rustdoc::FORMAT_VERSION)?;
    let directory = current_dir()?;
    let cargo = CargoCli::new(target_dir(&directory));
    exposed::exposed(&cargo, &directory, &args.packages)
}

fn print_audit(audit: &Audit) {
    for library in &audit.libraries {
        println!("{}", library.name);
        match &library.verdict {
            Verdict::Exposed(exposures) => {
                for exposure in exposures {
                    let through = match exposure.through.as_slice() {
                        [] => String::new(),
                        through => format!(" through {}", through.join(", ")),
                    };
                    let more = match exposure.more {
                        0 => String::new(),
                        more => format!(" and {more} more"),
                    };
                    println!(
                        "  {} {} ({}){through}: `{}`{more}",
                        exposure.dependency,
                        exposure.requirements.join(", "),
                        exposure.location,
                        exposure.example,
                    );
                }
            }
            Verdict::Undecided(reason) => println!("  undecided: {reason}"),
        }
    }
}
