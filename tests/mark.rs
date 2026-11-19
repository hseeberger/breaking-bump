mod common;

use crate::common::Run;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
};
use tempfile::TempDir;

const BRANCH: &str = "dependabot/cargo/dep-2.0.0";

#[test]
fn marks_commit_title_and_branch() {
    let forge = Forge::new();
    let (api, requests) = serve(vec![
        Response::new(200, forge.pull_request("dependabot[bot]")),
        Response::new(200, forge.commit()),
        Response::new(201, json!({ "sha": forge.marked })),
        Response::new(200, json!({})),
    ]);
    let run = forge.mark(&api, Some("token"));
    assert_eq!(run.code, 0, "{run}");
    assert!(
        run.stdout
            .contains(&format!("marked: {BRANCH} now at {}", forge.marked)),
        "{run}"
    );
    assert_eq!(forge.branch(), forge.marked);
    let requests = requests.try_iter().collect::<Vec<_>>();
    assert_eq!(
        requests[2],
        (
            "POST /repos/owner/repo/git/commits".to_owned(),
            json!({
                "message": "build(deps)!: bump dep",
                "tree": forge.tree,
                "parents": [forge.parent],
            })
        )
    );
    assert_eq!(
        requests[3],
        (
            "PATCH /repos/owner/repo/pulls/7".to_owned(),
            json!({ "title": "build(deps)!: bump dep" })
        )
    );
}

#[test]
fn branch_that_moves_after_the_title_edit_is_left_alone() {
    let forge = Forge::new();
    let bare = forge.bare();
    let newer = forge.newer.clone();
    let (api, requests) = serve(vec![
        Response::new(200, forge.pull_request("dependabot[bot]")),
        Response::new(200, forge.commit()),
        Response::new(201, json!({ "sha": forge.marked })),
        Response::new(200, json!({})).before_response(move || {
            git(
                &bare,
                &["update-ref", &format!("refs/heads/{BRANCH}"), &newer],
            );
        }),
    ]);
    let run = forge.mark(&api, Some("token"));
    assert_eq!(run.code, 0, "{run}");
    assert!(
        run.stdout.contains(&format!(
            "not marking: the pull request head is no longer {}",
            forge.examined
        )),
        "{run}"
    );
    assert!(
        run.stdout
            .contains("check whether the `!` still belongs in the pull request title"),
        "{run}"
    );
    assert_eq!(forge.branch(), forge.newer);
    assert_eq!(requests.try_iter().count(), 4);
}

#[test]
fn pull_request_from_someone_else_is_refused() {
    let forge = Forge::new();
    let (api, requests) = serve(vec![Response::new(200, forge.pull_request("someone"))]);
    let run = forge.mark(&api, Some("token"));
    assert_eq!(run.code, 1, "{run}");
    assert!(
        run.stdout
            .contains("not marking: the pull request is from someone"),
        "{run}"
    );
    assert_eq!(forge.branch(), forge.examined);
    assert_eq!(requests.try_iter().count(), 1);
}

#[test]
fn missing_token_is_an_error() {
    let forge = Forge::new();
    let run = forge.mark("http://127.0.0.1:9", None);
    assert_eq!(run.code, 3, "{run}");
    assert!(
        run.stderr
            .contains("neither GH_TOKEN nor GITHUB_TOKEN is set"),
        "{run}"
    );
    assert_eq!(forge.branch(), forge.examined);
}

struct Forge {
    root: TempDir,
    tree: String,
    parent: String,
    examined: String,
    marked: String,
    newer: String,
}

impl Forge {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let bare = root.path().join("owner").join("repo.git");
        fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--quiet", "--bare"]);
        git(&bare, &["config", "user.email", "test@example.com"]);
        git(&bare, &["config", "user.name", "test"]);
        git(&bare, &["config", "commit.gpgsign", "false"]);
        git(&bare, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        let tree = git(&bare, &["mktree"]);
        let parent = git(&bare, &["commit-tree", &tree, "-m", "feat: initial"]);
        let commit = |parent: &str, message: &str| {
            git(&bare, &["commit-tree", &tree, "-p", parent, "-m", message])
        };
        let examined = commit(&parent, "build(deps): bump dep");
        let marked = commit(&parent, "build(deps)!: bump dep");
        let newer = commit(&examined, "build(deps): bump dep again");
        git(
            &bare,
            &["update-ref", &format!("refs/heads/{BRANCH}"), &examined],
        );
        Self {
            root,
            tree,
            parent,
            examined,
            marked,
            newer,
        }
    }

    fn bare(&self) -> PathBuf {
        self.root.path().join("owner").join("repo.git")
    }

    fn branch(&self) -> String {
        git(
            &self.bare(),
            &["rev-parse", &format!("refs/heads/{BRANCH}")],
        )
    }

    fn pull_request(&self, author: &str) -> Value {
        json!({
            "user": { "login": author },
            "title": "build(deps): bump dep",
            "head": {
                "sha": self.examined,
                "ref": BRANCH,
                "repo": { "full_name": "owner/repo" },
            },
            "base": { "repo": { "full_name": "owner/repo" } },
        })
    }

    fn commit(&self) -> Value {
        json!({
            "sha": self.examined,
            "commit": { "message": "build(deps): bump dep", "tree": { "sha": self.tree } },
            "parents": [{ "sha": self.parent }],
            "files": [{ "filename": "Cargo.toml" }],
        })
    }

    fn mark(&self, api: &str, token: Option<&str>) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-breaking-bump"));
        command
            .args(["breaking-bump", "mark", "--repository", "owner/repo"])
            .args(["--pr", "7", "--head", &self.examined])
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GITHUB_ACTIONS")
            .env("GITHUB_API_URL", api)
            .env("GITHUB_SERVER_URL", self.root.path());
        if let Some(token) = token {
            command.env("GH_TOKEN", token);
        }
        let output = command.output().unwrap();
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
            outputs: BTreeMap::new(),
        }
    }
}

struct Response {
    status: u16,
    body: Value,
    before_response: Option<Box<dyn FnOnce() + Send>>,
}

impl Response {
    fn new(status: u16, body: Value) -> Self {
        Self {
            status,
            body,
            before_response: None,
        }
    }

    fn before_response<F>(self, before_response: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        Self {
            before_response: Some(Box::new(before_response)),
            ..self
        }
    }
}

fn serve(responses: Vec<Response>) -> (String, Receiver<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (name, value) = line.split_once(": ").unwrap();
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.parse().unwrap();
                }
            }
            let mut request_body = vec![0; length];
            reader.read_exact(&mut request_body).unwrap();
            let request_body = serde_json::from_slice(&request_body).unwrap_or(Value::Null);
            let request = request_line.rsplit_once(' ').unwrap().0.to_owned();
            sender.send((request, request_body)).unwrap();
            if let Some(before_response) = response.before_response {
                before_response();
            }
            let body = response.body.to_string();
            write!(
                stream,
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                response.status,
                body.len()
            )
            .unwrap();
        }
    });
    (api, receiver)
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
