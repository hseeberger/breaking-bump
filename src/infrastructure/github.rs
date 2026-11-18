use crate::{
    application::mark::Forge,
    domain::pull_request::{Commit, PullRequest},
    infrastructure::git::{authorized_env, git, push_with_lease},
};
use anyhow::{Context, bail};
use base64::{Engine, prelude::BASE64_STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{env, time::Duration};
use ureq::{Agent, Body, RequestBuilder, http, typestate::WithBody};

pub(crate) struct GitHub {
    agent: Agent,
    api: String,
    server: String,
    repository: String,
    token: String,
}

impl GitHub {
    pub(crate) fn new(api: String, server: String, repository: &str, token: String) -> Self {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            agent,
            api,
            server,
            repository: repository.to_owned(),
            token,
        }
    }

    fn get<T>(&self, path: &str) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .agent
            .get(self.url(path))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .with_context(|| format!("GET {path} failed"))?;
        json_body(&format!("GET {path}"), response)
    }

    fn post<T>(&self, path: &str, body: Value) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        self.send(
            self.agent.post(self.url(path)),
            &format!("POST {path}"),
            body,
        )
    }

    fn patch<T>(&self, path: &str, body: Value) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        self.send(
            self.agent.patch(self.url(path)),
            &format!("PATCH {path}"),
            body,
        )
    }

    fn send<T>(
        &self,
        request: RequestBuilder<WithBody>,
        description: &str,
        body: Value,
    ) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let response = request
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send_json(body)
            .with_context(|| format!("{description} failed"))?;
        json_body(description, response)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/repos/{}{path}", self.api, self.repository)
    }
}

impl Forge for GitHub {
    fn pull_request(&self, number: u64) -> anyhow::Result<PullRequest> {
        let pull_request = self.get::<ApiPullRequest>(&format!("/pulls/{number}"))?;
        Ok(PullRequest {
            author: pull_request.user.login,
            title: pull_request.title,
            branch: pull_request.head.branch,
            head: pull_request.head.sha,
            head_repository: pull_request.head.repo.map(|repo| repo.full_name),
            base_repository: pull_request
                .base
                .repo
                .map(|repo| repo.full_name)
                .unwrap_or_default(),
        })
    }

    fn commit(&self, sha: &str) -> anyhow::Result<Commit> {
        let commit = self.get::<ApiCommit>(&format!("/commits/{sha}"))?;
        Ok(Commit {
            message: commit.commit.message,
            tree: commit.commit.tree.sha,
            parents: commit
                .parents
                .into_iter()
                .map(|parent| parent.sha)
                .collect(),
            files: commit.files.into_iter().map(|file| file.filename).collect(),
        })
    }

    fn create_commit(&self, message: &str, tree: &str, parent: &str) -> anyhow::Result<String> {
        let body = json!({ "message": message, "tree": tree, "parents": [parent] });
        let commit = self.post::<Sha>("/git/commits", body)?;
        Ok(commit.sha)
    }

    fn set_title(&self, number: u64, title: &str) -> anyhow::Result<()> {
        let body = json!({ "title": title });
        self.patch::<Value>(&format!("/pulls/{number}"), body)?;
        Ok(())
    }

    fn move_branch(&self, branch: &str, expected: &str, new: &str) -> anyhow::Result<bool> {
        let url = format!("{}/{}.git", self.server, self.repository);
        let auth = BASE64_STANDARD.encode(format!("x-access-token:{}", self.token));
        if env::var("GITHUB_ACTIONS").is_ok_and(|value| value == "true") {
            println!("::add-mask::{auth}");
        }
        let env = authorized_env(&format!("Authorization: Basic {auth}"));
        let git_dir = tempfile::Builder::new()
            .prefix("breaking-bump-push-")
            .tempdir()?;
        git(git_dir.path(), &["init", "--quiet", "--bare"], &[])?;
        git(git_dir.path(), &["fetch", "--quiet", &url, new], &env)?;
        push_with_lease(git_dir.path(), &url, branch, expected, new, &env)
    }
}

#[derive(Deserialize)]
struct ApiPullRequest {
    user: User,
    title: String,
    head: Head,
    base: Base,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct Head {
    sha: String,
    #[serde(rename = "ref")]
    branch: String,
    repo: Option<Repo>,
}

#[derive(Deserialize)]
struct Base {
    repo: Option<Repo>,
}

#[derive(Deserialize)]
struct Repo {
    full_name: String,
}

#[derive(Deserialize)]
struct ApiCommit {
    commit: CommitDetail,
    parents: Vec<Sha>,
    #[serde(default)]
    files: Vec<File>,
}

#[derive(Deserialize)]
struct CommitDetail {
    message: String,
    tree: Sha,
}

#[derive(Deserialize)]
struct Sha {
    sha: String,
}

#[derive(Deserialize)]
struct File {
    filename: String,
}

fn json_body<T>(description: &str, mut response: http::Response<Body>) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if !status.is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        bail!("{description} failed with {status}: {}", body.trim());
    }
    response
        .body_mut()
        .read_json()
        .with_context(|| format!("{description} returned unexpected JSON"))
}

#[cfg(test)]
mod tests {
    use crate::{application::mark::Forge, infrastructure::github::GitHub};
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    #[test]
    fn talks_to_the_rest_api() {
        let (api, requests) = serve(vec![
            (200, pull_request()),
            (200, commit()),
            (201, json!({ "sha": "new" })),
            (200, json!({})),
            (
                403,
                json!({ "message": "Resource not accessible by integration" }),
            ),
        ]);
        let github = GitHub::new(api, "unused".to_owned(), "owner/repo", "token".to_owned());

        let pull_request = github.pull_request(7).unwrap();
        assert_eq!(pull_request.author, "dependabot[bot]");
        assert_eq!(pull_request.branch, "dependabot/cargo/dep-2.0.0");
        assert_eq!(pull_request.head, "head");
        assert_eq!(pull_request.head_repository.as_deref(), Some("owner/repo"));
        assert_eq!(pull_request.base_repository, "owner/repo");

        let commit = github.commit("head").unwrap();
        assert_eq!(commit.tree, "tree");
        assert_eq!(commit.parents, ["parent"]);
        assert_eq!(commit.files, ["Cargo.toml"]);

        let sha = github
            .create_commit("build(deps)!: bump dep", "tree", "parent")
            .unwrap();
        assert_eq!(sha, "new");
        github.set_title(7, "build(deps)!: bump dep").unwrap();
        let error = github.set_title(7, "x").unwrap_err().to_string();
        assert!(error.contains("403"), "{error}");
        assert!(error.contains("Resource not accessible"), "{error}");

        let requests = requests.iter().collect::<Vec<_>>();
        assert_eq!(requests[0].0, "GET /repos/owner/repo/pulls/7");
        assert_eq!(requests[0].1, "Bearer token");
        assert_eq!(requests[1].0, "GET /repos/owner/repo/commits/head");
        assert_eq!(requests[2].0, "POST /repos/owner/repo/git/commits");
        assert_eq!(
            requests[2].2,
            json!({ "message": "build(deps)!: bump dep", "tree": "tree", "parents": ["parent"] })
        );
        assert_eq!(requests[3].0, "PATCH /repos/owner/repo/pulls/7");
        assert_eq!(requests[3].2, json!({ "title": "build(deps)!: bump dep" }));
    }

    fn serve(responses: Vec<(u16, Value)>) -> (String, mpsc::Receiver<(String, String, Value)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut authorization = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(": ").unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => authorization = value.to_owned(),
                        "content-length" => length = value.parse().unwrap(),
                        _ => {}
                    }
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                let request_body = serde_json::from_slice(&request_body).unwrap_or(Value::Null);
                let request = request_line.rsplit_once(' ').unwrap().0.to_owned();
                sender.send((request, authorization, request_body)).unwrap();
                let body = body.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (api, receiver)
    }

    fn pull_request() -> Value {
        json!({
            "user": { "login": "dependabot[bot]" },
            "title": "build(deps): bump dep",
            "head": {
                "sha": "head",
                "ref": "dependabot/cargo/dep-2.0.0",
                "repo": { "full_name": "owner/repo" },
            },
            "base": { "repo": { "full_name": "owner/repo" } },
        })
    }

    fn commit() -> Value {
        json!({
            "sha": "head",
            "commit": { "message": "build(deps): bump dep", "tree": { "sha": "tree" } },
            "parents": [{ "sha": "parent" }],
            "files": [{ "filename": "Cargo.toml" }],
        })
    }
}
