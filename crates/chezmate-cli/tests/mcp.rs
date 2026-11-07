//! Drives `chezmate mcp` over stdio against a sandboxed real chezmoi, the
//! way an MCP client does.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Session {
    _root: TempDir,
    home: PathBuf,
    source: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    modern: bool,
}

impl Session {
    fn start(extra: &[&str], modern: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let (home, source, config) = (base.join("home"), base.join("source"), base.join("config"));
        for dir in [&home, &source, &config] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(config.join("chezmoi.toml"), "").unwrap();
        fs::write(source.join("dot_hello"), "hello\n").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_chezmate"))
            .arg("--source")
            .arg(&source)
            .arg("--destination")
            .arg(&home)
            .arg("--config")
            .arg(config.join("chezmoi.toml"))
            .arg("--persistent-state")
            .arg(config.join("state.boltdb"))
            .arg("mcp")
            .args(extra)
            .env("HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            _root: root,
            home,
            source,
            child,
            stdin,
            stdout,
            next_id: 1,
            modern,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, mut params: Value) -> Value {
        if self.modern {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let response: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"));
        assert_eq!(response["id"], id);
        response
    }

    /// Calls a tool and returns (is_error, structuredContent).
    fn call(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &response["result"];
        assert!(result.is_object(), "{response}");
        if self.modern {
            assert_eq!(result["resultType"], "complete");
        }
        assert_eq!(result["content"][0]["type"], "text");
        (
            result["isError"].as_bool().unwrap(),
            result["structuredContent"].clone(),
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workflow(s: &mut Session) {
    let (err, status) = s.call("chezmoi_status", json!({}));
    assert!(!err, "{status}");
    assert_eq!(status["entries"][0]["path"], ".hello");
    assert_eq!(status["up_to_date"], false);

    let (_, diff) = s.call("chezmoi_diff", json!({ "targets": [".hello"] }));
    assert!(diff["diff"].as_str().unwrap().contains("+hello"));

    // Apply is a dry run unless asked otherwise.
    let (err, dry) = s.call("chezmoi_apply", json!({ "targets": [".hello"] }));
    assert!(!err);
    assert_eq!(dry["dry_run"], true);
    assert!(!s.home.join(".hello").exists());

    let (err, _) = s.call(
        "chezmoi_apply",
        json!({ "targets": [".hello"], "dry_run": false }),
    );
    assert!(!err);
    assert_eq!(
        fs::read_to_string(s.home.join(".hello")).unwrap(),
        "hello\n"
    );
    assert_eq!(s.call("chezmoi_status", json!({})).1["up_to_date"], true);

    // A local edit plus a source change is a conflict the agent can resolve.
    fs::write(s.home.join(".hello"), "mine\n").unwrap();
    fs::write(s.source.join("dot_hello"), "theirs\n").unwrap();
    let (err, conflict) = s.call("chezmoi_apply", json!({ "dry_run": false }));
    assert!(err);
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["paths"][0], ".hello");
    let (err, readd) = s.call("chezmoi_re_add", json!({}));
    assert!(!err, "{readd}");
    assert_eq!(readd["updated"][0], ".hello");
    assert_eq!(
        fs::read_to_string(s.source.join("dot_hello")).unwrap(),
        "mine\n"
    );

    // Bad arguments come back as a tool error the model can read.
    let (err, bad) = s.call("chezmoi_apply", json!({ "dryrun": false }));
    assert!(err);
    assert_eq!(bad["kind"], "invalid_request");
    let (err, outside) = s.call("chezmoi_diff", json!({ "targets": ["../etc/passwd"] }));
    assert!(err);
    assert_eq!(outside["kind"], "invalid_request");

    // The simplified model: one state per file, and sync as a dry run first.
    fs::write(s.home.join(".hello"), "edited again\n").unwrap();
    let (err, overview) = s.call("sync_overview", json!({}));
    assert!(!err, "{overview}");
    assert_eq!(overview["files"][0]["target"], ".hello");
    assert_eq!(overview["files"][0]["state"], "changed_here");
    let (err, dry) = s.call("sync", json!({}));
    assert!(!err, "{dry}");
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["plan"]["save"][0], ".hello");
    assert_eq!(
        fs::read_to_string(s.source.join("dot_hello")).unwrap(),
        "mine\n"
    );
    let (err, done) = s.call("sync", json!({ "dry_run": false }));
    assert!(!err, "{done}");
    assert_eq!(done["outcome"]["saved"][0], ".hello");
    assert_eq!(
        fs::read_to_string(s.source.join("dot_hello")).unwrap(),
        "edited again\n"
    );
    let (_, overview) = s.call("sync_overview", json!({}));
    assert_eq!(overview["files"], json!([]));
}

#[test]
fn legacy_client_session() {
    let mut s = Session::start(&[], false);
    let init = s.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    s.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let tools = s.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 17);
    workflow(&mut s);
}

#[test]
fn modern_client_session() {
    let mut s = Session::start(&[], true);
    let discover = s.request("server/discover", json!({}));
    assert_eq!(discover["result"]["supportedVersions"][0], "2026-07-28");
    workflow(&mut s);
}

#[test]
fn read_only_server_cannot_write() {
    let mut s = Session::start(&["--read-only"], true);
    let tools = s.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"chezmoi_diff"));
    assert!(names.contains(&"sync_overview"));
    assert!(!names.contains(&"chezmoi_apply"));
    assert!(!names.contains(&"sync"));
    let denied = s.request(
        "tools/call",
        json!({ "name": "chezmoi_apply", "arguments": { "dry_run": false } }),
    );
    assert_eq!(denied["error"]["code"], -32602);
    assert!(!s.home.join(".hello").exists());
}
