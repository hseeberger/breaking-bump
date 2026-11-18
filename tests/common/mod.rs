#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
use tempfile::TempDir;

const DEPS: [(&str, &str); 9] = [
    ("dep_reexport", "pub struct Reexported;"),
    ("dep_sig", "pub struct S;"),
    ("dep_trait", "pub trait Tr {}"),
    ("dep_private", "pub struct P;"),
    ("dep_dev", "pub fn helper() {}"),
    ("dep_c", "pub struct Thing;"),
    (
        "dep_b",
        "pub use dep_c::Thing;\npub fn make() -> Thing { Thing }",
    ),
    (
        "dep_marker",
        "pub trait Marker {}\npub use dep_derive::Marker;",
    ),
    ("dep_keep", "pub use dep_c::Thing;"),
];

const DERIVE: &str = r#"
use proc_macro::{TokenStream, TokenTree};

#[proc_macro_derive(Marker)]
pub fn derive(input: TokenStream) -> TokenStream {
    let mut tokens = input.into_iter();
    while let Some(token) = tokens.next() {
        if let TokenTree::Ident(ident) = &token {
            if ident.to_string() == "struct" {
                if let Some(TokenTree::Ident(name)) = tokens.next() {
                    return format!("impl ::dep_marker::Marker for {name} {{}}").parse().unwrap();
                }
            }
        }
    }
    panic!("no struct")
}
"#;

const SUBJECT: &str = r#"
pub use dep_reexport::Reexported;

#[derive(dep_marker::Marker)]
pub struct Derived;

pub struct Local;

impl dep_trait::Tr for Local {}

pub fn signature() -> dep_sig::S {
    private();
    dep_sig::S
}

fn private() -> dep_private::P {
    dep_private::P
}

#[cfg(test)]
mod tests {
    #[test]
    fn helper() {
        dep_dev::helper();
    }
}
"#;

const TRANSITIVE: &str = r#"
#[cfg(feature = "transitive")]
pub fn transitive() -> dep_b::Thing {
    dep_b::make()
}
"#;

const SAME_NAMED_IMPLS: &str = r#"
#[cfg(feature = "transitive")]
pub mod first {
    pub struct S;
    impl S {
        pub fn get(&self) -> dep_b::Thing {
            dep_b::make()
        }
    }
}

pub mod second {
    pub struct S;
    impl S {
        pub fn get(&self) -> dep_c::Thing {
            dep_c::Thing
        }
    }
}

pub mod third {
    pub struct S;
    impl S {
        pub fn get(&self) -> dep_keep::Thing {
            dep_keep::Thing
        }
    }
}
"#;

#[derive(Default)]
pub struct Options {
    pub workspace: bool,
    pub private_dep_c: Option<&'static str>,
    pub public_dep_c: bool,
    pub keep_dep_c_1: bool,
    pub same_named_impls: bool,
    pub cargo_env: bool,
}

// A git repository with a published crate `subject` and path dependencies under `deps/`.
pub struct Fixture {
    pub dir: TempDir,
    target: TempDir,
    pub versions: BTreeMap<&'static str, String>,
    pub description: String,
    pub dep_b_uses_dep_c_2: bool,
    pub dep_keep_uses_dep_c_3: bool,
    pub settings: Option<String>,
    options: Options,
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub outputs: BTreeMap<String, String>,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with(Options::default())
    }

    pub fn with(options: Options) -> Self {
        let mut versions = DEPS
            .iter()
            .map(|(name, _)| (*name, "1.0.0".to_owned()))
            .collect::<BTreeMap<_, _>>();
        versions.insert("dep_derive", "1.0.0".to_owned());
        let fixture = Self {
            dir: tempfile::Builder::new()
                .prefix("breaking-bump-fixture-")
                .tempdir()
                .unwrap(),
            target: tempfile::Builder::new()
                .prefix("breaking-bump-target-")
                .tempdir()
                .unwrap(),
            versions,
            description: "subject".to_owned(),
            dep_b_uses_dep_c_2: false,
            dep_keep_uses_dep_c_3: false,
            settings: None,
            options,
        };
        fixture.git(&["init", "--quiet", "--initial-branch", "main"]);
        fixture.git(&["config", "user.email", "test@example.com"]);
        fixture.git(&["config", "user.name", "test"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture.write_all();
        fixture.commit("feat: initial");
        fixture
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn write(&self, path: &str, content: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content.trim_start()).unwrap();
    }

    pub fn write_all(&self) {
        let workspace = self.options.workspace;
        let up = if workspace { "../" } else { "" };
        for (name, source) in DEPS {
            let dependencies = match name {
                "dep_b" => {
                    let dep_c = if self.dep_b_uses_dep_c_2 {
                        "../dep_c2"
                    } else {
                        "../dep_c"
                    };
                    format!("[dependencies]\ndep_c = {{ path = \"{dep_c}\" }}\n")
                }
                "dep_keep" => {
                    let dep_c = if self.dep_keep_uses_dep_c_3 {
                        "../dep_c3"
                    } else {
                        "../dep_c"
                    };
                    format!("[dependencies]\ndep_c = {{ path = \"{dep_c}\" }}\n")
                }
                "dep_marker" => {
                    "[dependencies]\ndep_derive = { path = \"../dep_derive\" }\n".to_owned()
                }
                _ => String::new(),
            };
            self.write(
                &format!("deps/{name}/Cargo.toml"),
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"{}\"\nedition = \"2024\"\n\n{dependencies}",
                    self.versions[name]
                ),
            );
            self.write(&format!("deps/{name}/src/lib.rs"), source);
        }
        self.write(
            "deps/dep_derive/Cargo.toml",
            &format!(
                "[package]\nname = \"dep_derive\"\nversion = \"{}\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n",
                self.versions["dep_derive"]
            ),
        );
        self.write("deps/dep_derive/src/lib.rs", DERIVE);
        self.write(
            "deps/dep_c2/Cargo.toml",
            "[package]\nname = \"dep_c\"\nversion = \"2.0.0\"\nedition = \"2024\"\n",
        );
        self.write("deps/dep_c2/src/lib.rs", "pub struct Thing;");
        self.write(
            "deps/dep_c3/Cargo.toml",
            "[package]\nname = \"dep_c\"\nversion = \"3.0.0\"\nedition = \"2024\"\n",
        );
        self.write("deps/dep_c3/src/lib.rs", "pub struct Thing;");

        let mut source = SUBJECT.to_owned();
        if self.options.same_named_impls {
            source.push_str(SAME_NAMED_IMPLS);
        } else {
            source.push_str(TRANSITIVE);
        }
        let mut extra = String::new();
        if let Some(version) = self.options.private_dep_c {
            let visibility = if self.options.public_dep_c {
                "pub "
            } else {
                ""
            };
            source.push_str(&format!(
                "\n#[allow(dead_code)]\n{visibility}fn direct_c() -> dep_c::Thing {{\n    dep_c::Thing\n}}\n"
            ));
            let directory = if version == "2.0.0" {
                "dep_c2"
            } else {
                "dep_c"
            };
            extra.push_str(&format!(
                "dep_c = {{ path = \"{up}deps/{directory}\", version = \"{version}\" }}\n"
            ));
        }
        if self.options.keep_dep_c_1 {
            source.push_str("\npub fn kept() -> dep_keep::Thing {\n    dep_keep::Thing\n}\n");
            extra.push_str(&format!(
                "dep_keep = {{ path = \"{up}deps/dep_keep\", version = \"{}\" }}\n",
                self.versions["dep_keep"]
            ));
        }
        if self.options.cargo_env {
            source.push_str("\npub const VALUE: &str = env!(\"BREAKING_BUMP_FIXTURE\");\n");
            self.write(
                ".cargo/config.toml",
                "[env]\nBREAKING_BUMP_FIXTURE = \"set\"\n",
            );
        }

        let mut dependencies = Vec::new();
        for name in [
            "dep_reexport",
            "dep_sig",
            "dep_trait",
            "dep_private",
            "dep_marker",
        ] {
            if workspace && name == "dep_sig" {
                dependencies.push(format!("{name} = {{ workspace = true }}"));
            } else {
                dependencies.push(format!(
                    "{name} = {{ path = \"{up}deps/{name}\", version = \"{}\" }}",
                    self.versions[name]
                ));
            }
        }
        dependencies.push(format!(
            "dep_b = {{ path = \"{up}deps/dep_b\", version = \"{}\", optional = true }}",
            self.versions["dep_b"]
        ));
        let package = format!(
            "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
             description = \"{}\"\n\n[dependencies]\n{}\n{extra}\n\
             [dev-dependencies]\ndep_dev = {{ path = \"{up}deps/dep_dev\", version = \"{}\" }}\n\n\
             [features]\ntransitive = [\"dep:dep_b\"]\n",
            self.description,
            dependencies.join("\n"),
            self.versions["dep_dev"],
        );
        let settings = match &self.settings {
            Some(settings) => format!("\n[workspace.metadata.breaking-bump]\n{settings}\n"),
            None => String::new(),
        };
        if workspace {
            self.write(
                "Cargo.toml",
                &format!(
                    "[workspace]\nmembers = [\"subject\"]\nexclude = [\"deps\"]\nresolver = \"3\"\n\n\
                     [workspace.dependencies]\n\
                     dep_sig = {{ path = \"deps/dep_sig\", version = \"{}\" }}\n{settings}",
                    self.versions["dep_sig"]
                ),
            );
            self.write("subject/Cargo.toml", &package);
            self.write("subject/src/lib.rs", &source);
        } else {
            self.write(
                "Cargo.toml",
                &format!("{package}\n[workspace]\nexclude = [\"deps\"]\n{settings}"),
            );
            self.write("src/lib.rs", &source);
        }
    }

    pub fn commit(&self, message: &str) -> String {
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline", "--quiet"])
            .current_dir(self.path())
            .env_remove("CARGO_TARGET_DIR")
            .status()
            .unwrap();
        assert!(status.success());
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
        self.head()
    }

    pub fn bump(&mut self, name: &'static str, version: &str, message: &str) -> String {
        self.versions.insert(name, version.to_owned());
        self.write_all();
        self.commit(message)
    }

    pub fn check(&self, base: &str, head: &str) -> Run {
        self.run(&["check", "--base", base, "--head", head])
    }

    pub fn run(&self, args: &[&str]) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-breaking-bump"));
        command.arg("breaking-bump").args(args);
        self.execute(command, Some(nightly()))
    }

    pub fn execute(&self, mut command: Command, toolchain: Option<&str>) -> Run {
        let outputs = self.target.path().join("github-output");
        let _ = fs::remove_file(&outputs);
        match toolchain {
            Some(toolchain) => command.env("RUSTUP_TOOLCHAIN", toolchain),
            None => command.env_remove("RUSTUP_TOOLCHAIN"),
        };
        let output = command
            .current_dir(self.path())
            .env_remove("CARGO")
            .env("CARGO_TARGET_DIR", self.target.path())
            .env("GITHUB_OUTPUT", &outputs)
            .output()
            .unwrap();
        let outputs = fs::read_to_string(&outputs)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
            outputs,
        }
    }

    pub fn target(&self) -> &Path {
        self.target.path()
    }
}

impl Run {
    pub fn result(&self) -> &str {
        self.outputs
            .get("result")
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub fn violations(&self) -> &str {
        self.outputs
            .get("violations")
            .map(String::as_str)
            .unwrap_or_default()
    }

    #[track_caller]
    pub fn assert(&self, code: i32, result: &str, violations: &str) {
        assert_eq!(
            (self.code, self.result(), self.violations()),
            (code, result, violations),
            "{self}"
        );
    }
}

impl std::fmt::Display for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stdout:\n{}\nstderr:\n{}", self.stdout, self.stderr)
    }
}

pub fn stable() -> String {
    let output = Command::new("rustc").arg("--version").output().unwrap();
    let version = String::from_utf8(output.stdout).unwrap();
    version
        .split(' ')
        .nth(1)
        .expect("rustc --version has no version")
        .to_owned()
}

pub fn nightly() -> &'static str {
    static NIGHTLY: OnceLock<String> = OnceLock::new();
    NIGHTLY.get_or_init(|| {
        if let Ok(toolchain) = env::var("BREAKING_BUMP_TEST_TOOLCHAIN") {
            return toolchain;
        }
        let output = Command::new("rustc").arg("--version").output().unwrap();
        let version = String::from_utf8(output.stdout).unwrap();
        let date = version
            .split([' ', ')'])
            .find(|part| {
                part.len() == 10 && part.as_bytes()[4] == b'-' && part.as_bytes()[7] == b'-'
            })
            .expect("rustc --version contains no date");
        format!("nightly-{date}")
    })
}

pub fn bin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cargo-breaking-bump"))
        .parent()
        .unwrap()
        .to_path_buf()
}
