#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    env, fmt, fs,
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

const POSITION_DEPS: [(&str, &str); 6] = [
    ("dep_field", "pub struct S;"),
    ("dep_bound", "pub trait Tr {}"),
    ("dep_arg", "pub struct S;"),
    ("dep_from", "pub struct S;"),
    ("dep_variant", "pub struct S;"),
    ("dep_method", "pub struct S;"),
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

const POSITIONS: &str = r#"
pub struct Holder {
    pub field: dep_field::S,
}

pub fn bounded<T: dep_bound::Tr>(_value: T) {}

pub fn listed() -> Vec<dep_arg::S> {
    Vec::new()
}

impl From<dep_from::S> for Local {
    fn from(_value: dep_from::S) -> Self {
        Local
    }
}

pub enum Choice {
    Variant(dep_variant::S),
}

pub trait Api {
    fn method(&self) -> dep_method::S;
}
"#;

const OTHER: &str = r#"
pub struct Other;

pub fn other() -> dep_other::O {
    dep_other::O
}

pub fn local() -> subject::Local {
    subject::Local
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
    pub positions: bool,
    pub target_specific: bool,
    pub renamed: bool,
    pub unpublished: bool,
    // Needs `workspace`.
    pub second_member: bool,
    pub nested: bool,
}

// A git repository with a published crate `subject` and path dependencies under `deps/`.
pub struct Fixture {
    pub dir: TempDir,
    target: TempDir,
    pub versions: BTreeMap<&'static str, String>,
    pub requirements: BTreeMap<&'static str, String>,
    pub description: String,
    pub subject_version: String,
    pub dep_b_uses_dep_c_2: bool,
    pub dep_keep_uses_dep_c_3: bool,
    pub settings: Option<String>,
    pub options: Options,
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
            .chain(&POSITION_DEPS)
            .map(|(name, _)| (*name, "1.0.0".to_owned()))
            .collect::<BTreeMap<_, _>>();
        versions.insert("dep_derive", "1.0.0".to_owned());
        versions.insert("dep_other", "1.0.0".to_owned());
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
            requirements: BTreeMap::new(),
            description: "subject".to_owned(),
            subject_version: "0.1.0".to_owned(),
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

    pub fn workspace_dir(&self) -> PathBuf {
        if self.options.nested {
            self.path().join("nested")
        } else {
            self.path().to_path_buf()
        }
    }

    pub fn write(&self, path: &str, content: &str) {
        let path = self.workspace_dir().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content.trim_start()).unwrap();
    }

    pub fn write_all(&self) {
        let workspace = self.options.workspace;
        let up = if workspace { "../" } else { "" };
        let mut deps = DEPS.to_vec();
        if self.options.positions {
            deps.extend(POSITION_DEPS);
        }
        if self.options.second_member {
            deps.push(("dep_other", "pub struct O;"));
        }
        for (name, source) in deps {
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
        if self.options.positions {
            source.push_str(POSITIONS);
        }
        if self.options.renamed {
            source = source.replace("dep_sig::", "sig::");
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

        let dep_sig_key = if self.options.renamed {
            "sig"
        } else {
            "dep_sig"
        };
        let dep_sig_package = if self.options.renamed {
            "package = \"dep_sig\", "
        } else {
            ""
        };
        let dep_sig = if workspace {
            format!("{dep_sig_key} = {{ workspace = true }}")
        } else {
            format!(
                "{dep_sig_key} = {{ {dep_sig_package}path = \"deps/dep_sig\", version = \"{}\" }}",
                self.requirement("dep_sig")
            )
        };
        let mut dependencies = Vec::new();
        let mut target = String::new();
        if self.options.target_specific {
            target = format!("\n[target.'cfg(any(unix, windows))'.dependencies]\n{dep_sig}\n");
        } else {
            dependencies.push(dep_sig);
        }
        let mut names = vec!["dep_reexport", "dep_trait", "dep_private", "dep_marker"];
        if self.options.positions {
            names.extend(POSITION_DEPS.iter().map(|(name, _)| *name));
        }
        for name in names {
            dependencies.push(format!(
                "{name} = {{ path = \"{up}deps/{name}\", version = \"{}\" }}",
                self.requirement(name)
            ));
        }
        dependencies.push(format!(
            "dep_b = {{ path = \"{up}deps/dep_b\", version = \"{}\", optional = true }}",
            self.versions["dep_b"]
        ));
        let publish = if self.options.unpublished {
            "publish = false\n"
        } else {
            ""
        };
        let package = format!(
            "[package]\nname = \"subject\"\nversion = \"{}\"\nedition = \"2024\"\n\
             description = \"{}\"\n{publish}\n[dependencies]\n{}\n{extra}{target}\n\
             [dev-dependencies]\ndep_dev = {{ path = \"{up}deps/dep_dev\", version = \"{}\" }}\n\n\
             [features]\ntransitive = [\"dep:dep_b\"]\n",
            self.subject_version,
            self.description,
            dependencies.join("\n"),
            self.versions["dep_dev"],
        );
        let settings = match &self.settings {
            Some(settings) => format!("\n[workspace.metadata.breaking-bump]\n{settings}\n"),
            None => String::new(),
        };
        if workspace {
            let members = if self.options.second_member {
                "\"subject\", \"other\""
            } else {
                "\"subject\""
            };
            self.write(
                "Cargo.toml",
                &format!(
                    "[workspace]\nmembers = [{members}]\nexclude = [\"deps\"]\nresolver = \"3\"\n\n\
                     [workspace.dependencies]\n\
                     {dep_sig_key} = {{ {dep_sig_package}path = \"deps/dep_sig\", version = \"{}\" }}\n\
                     {settings}",
                    self.requirement("dep_sig")
                ),
            );
            self.write("subject/Cargo.toml", &package);
            self.write("subject/src/lib.rs", &source);
            if self.options.second_member {
                self.write(
                    "other/Cargo.toml",
                    &format!(
                        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
                         [dependencies]\n\
                         dep_other = {{ path = \"../deps/dep_other\", version = \"{}\" }}\n\
                         subject = {{ path = \"../subject\", version = \"{}\" }}\n",
                        self.versions["dep_other"], self.subject_version
                    ),
                );
                self.write("other/src/lib.rs", OTHER);
            }
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
            .current_dir(self.workspace_dir())
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
        self.run_in(&self.workspace_dir(), args)
    }

    pub fn run_in(&self, directory: &Path, args: &[&str]) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-breaking-bump"));
        command
            .arg("breaking-bump")
            .args(args)
            .current_dir(directory);
        self.execute(command, Some(nightly()))
    }

    pub fn execute(&self, mut command: Command, toolchain: Option<&str>) -> Run {
        let outputs = self.target.path().join("github-output");
        let _ = fs::remove_file(&outputs);
        match toolchain {
            Some(toolchain) => command.env("RUSTUP_TOOLCHAIN", toolchain),
            None => command.env_remove("RUSTUP_TOOLCHAIN"),
        };
        if command.get_current_dir().is_none() {
            command.current_dir(self.workspace_dir());
        }
        let output = command
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

    fn requirement(&self, name: &str) -> &str {
        self.requirements.get(name).unwrap_or(&self.versions[name])
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

impl fmt::Display for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

pub fn host() -> String {
    let output = Command::new("rustc").arg("-vV").output().unwrap();
    let version = String::from_utf8(output.stdout).unwrap();
    version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc -vV reports no host")
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
