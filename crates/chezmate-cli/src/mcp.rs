//! `chezmate mcp`: a Model Context Protocol server over stdio.
//!
//! Dual-era, as the 2026-07-28 revision allows: a client that opens with
//! `initialize` gets the handshake-based protocol (2025-11-25 and earlier);
//! a request carrying `_meta["io.modelcontextprotocol/protocolVersion"]` is
//! served statelessly under 2026-07-28. Messages are newline-delimited JSON.

use std::io::{self, BufRead, Write};

use chezmate_core::{
    AddOptions, ApplyOptions, Chezmoi, DiffOptions, EntryType, Error, ManagedEntry,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

const INSTRUCTIONS: &str = "Chezmate manages dotfiles with chezmoi. \
Target paths are relative to the destination (home) directory, exactly as chezmoi_status reports them; absolute paths and ~/ also work. \
Typical flow: chezmoi_status, then chezmoi_diff for the targets, then chezmoi_apply. \
chezmoi_apply is a dry run unless dry_run is false; a target edited outside chezmoi fails with kind \"conflict\" unless force is true, and chezmoi_re_add keeps the local edit instead. \
Diffs leave out encrypted files unless include_encrypted is true; rendered templates can still contain secrets. \
git_pull only updates the source directory: review chezmoi_status and chezmoi_diff, then apply. \
Errors carry a stable \"kind\".";

fn server_info() -> Value {
    json!({ "name": "chezmate", "title": "Chezmate", "version": env!("CARGO_PKG_VERSION") })
}

/// Which protocol a request is served under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Era {
    Legacy,
    Modern,
}

pub struct Server {
    chezmoi: Result<Chezmoi, String>,
    read_only: bool,
}

impl Server {
    pub fn new(chezmoi: Result<Chezmoi, Error>, read_only: bool) -> Self {
        Self {
            chezmoi: chezmoi.map_err(|err| err.to_string()),
            read_only,
        }
    }

    /// Handles one line of input; returns the response line, if any.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => {
                return Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    &err.to_string(),
                    None,
                ));
            }
        };
        match message {
            // JSON-RPC batches (2025-03-26 and earlier).
            Value::Array(messages) => {
                let responses: Vec<Value> = messages
                    .into_iter()
                    .filter_map(|m| self.handle(m))
                    .collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            message => self.handle(message),
        }
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "expected an object",
                None,
            ));
        };
        let method = object.get("method").and_then(Value::as_str);
        let id = object.get("id").cloned();
        let (Some(method), Some(id)) = (method, id) else {
            // Notifications (initialized, cancelled, …) and stray responses.
            return None;
        };
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));

        let requested = params
            .get("_meta")
            .and_then(|meta| meta.get(PROTOCOL_VERSION_KEY))
            .and_then(Value::as_str);
        let era = match (method, requested) {
            ("initialize", _) => Era::Legacy,
            (_, Some(version)) if MODERN_VERSIONS.contains(&version) => Era::Modern,
            (_, Some(version)) => {
                return Some(error_response(
                    id,
                    UNSUPPORTED_PROTOCOL_VERSION,
                    "Unsupported protocol version",
                    Some(json!({ "supported": supported_versions(), "requested": version })),
                ));
            }
            ("server/discover", None) => Era::Modern,
            (_, None) => Era::Legacy,
        };

        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "server/discover" => Ok(self.discover()),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.list_tools(era)),
            "tools/call" => self.call_tool(&params),
            _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": finish(result, era),
            }),
            Err((code, message)) => error_response(id, code, &message, None),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested
            .filter(|v| LEGACY_VERSIONS.contains(v))
            .unwrap_or(LEGACY_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn discover(&self) -> Value {
        json!({
            "supportedVersions": supported_versions(),
            "capabilities": { "tools": { "listChanged": false } },
            "instructions": INSTRUCTIONS,
            "ttlMs": 3_600_000,
            "cacheScope": "public",
        })
    }

    fn list_tools(&self, era: Era) -> Value {
        let tools: Vec<Value> = TOOLS
            .iter()
            .filter(|tool| !self.read_only || tool.read_only)
            .map(|tool| tool.describe())
            .collect();
        let mut result = json!({ "tools": tools });
        if era == Era::Modern {
            // The list depends only on this binary and its flags.
            result["ttlMs"] = json!(3_600_000);
            result["cacheScope"] = json!("public");
        }
        result
    }

    fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == name)
            .filter(|tool| !self.read_only || tool.read_only)
            .ok_or_else(|| (INVALID_PARAMS, format!("unknown tool: {name}")))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let outcome = match &self.chezmoi {
            Ok(chezmoi) => (tool.run)(chezmoi, arguments),
            Err(message) => Err(ToolError::Message {
                kind: "chezmoi_not_found",
                message: message.clone(),
            }),
        };
        Ok(match outcome {
            Ok(output) => json!({
                "content": [{ "type": "text", "text": output.text }],
                "structuredContent": output.data,
                "isError": false,
            }),
            Err(err) => {
                let data = err.to_json();
                json!({
                    "content": [{ "type": "text", "text": data.to_string() }],
                    "structuredContent": data,
                    "isError": true,
                })
            }
        })
    }
}

fn supported_versions() -> Vec<&'static str> {
    MODERN_VERSIONS
        .iter()
        .chain(LEGACY_VERSIONS)
        .copied()
        .collect()
}

/// Adds the fields every modern result carries.
fn finish(mut result: Value, era: Era) -> Value {
    if era == Era::Modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ SERVER_INFO_KEY: server_info() });
    }
    result
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// Runs the server until stdin closes.
pub fn serve(chezmoi: Result<Chezmoi, Error>, read_only: bool) -> io::Result<()> {
    let mut server = Server::new(chezmoi, read_only);
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tools

struct ToolOutput {
    text: String,
    data: Value,
}

impl ToolOutput {
    fn json(data: Value) -> Self {
        Self {
            text: serde_json::to_string_pretty(&data).unwrap_or_default(),
            data,
        }
    }
}

enum ToolError {
    Core(Error),
    Message { kind: &'static str, message: String },
}

impl From<Error> for ToolError {
    fn from(err: Error) -> Self {
        ToolError::Core(err)
    }
}

impl ToolError {
    fn invalid(message: impl Into<String>) -> Self {
        ToolError::Message {
            kind: "invalid_request",
            message: message.into(),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            ToolError::Core(err) => {
                let mut value = json!({ "kind": err.kind(), "message": err.to_string() });
                if let Some(stderr) = err.stderr() {
                    value["stderr"] = json!(stderr);
                }
                if let Error::Conflict { paths, .. } = err {
                    value["paths"] = json!(paths);
                }
                value
            }
            ToolError::Message { kind, message } => json!({ "kind": kind, "message": message }),
        }
    }
}

type ToolResult = Result<ToolOutput, ToolError>;

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// JSON Schema properties and required list for the arguments.
    properties: fn() -> (Value, &'static [&'static str]),
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
    run: fn(&Chezmoi, Value) -> ToolResult,
}

impl Tool {
    fn describe(&self) -> Value {
        let (properties, required) = (self.properties)();
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
            "annotations": {
                "title": self.title,
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            },
        })
    }
}

fn args<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments)
        .map_err(|err| ToolError::invalid(format!("invalid arguments: {err}")))
}

fn no_properties() -> (Value, &'static [&'static str]) {
    (json!({}), &[])
}

fn targets_property(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": description })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TargetsArgs {
    #[serde(default)]
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedArgs {
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffArgs {
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    include_encrypted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyArgs {
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default = "yes")]
    dry_run: bool,
    #[serde(default)]
    force: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    paths: Vec<String>,
    #[serde(default)]
    template: bool,
    #[serde(default)]
    encrypt: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequiredTargets {
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogArgs {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitArgs {
    message: String,
}

fn managed_json(entries: &[ManagedEntry]) -> Value {
    json!({ "count": entries.len(), "entries": entries })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "chezmoi_info",
        title: "chezmoi installation",
        description: "chezmoi version, executable, source, destination and config paths, and whether the source is a git repository.",
        properties: no_properties,
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            Ok(ToolOutput::json(json!(chezmoi.info()?)))
        },
    },
    Tool {
        name: "chezmoi_status",
        title: "Pending changes",
        description: "Targets that differ from the source state. Each entry has `local` (how the file on disk changed since chezmoi last wrote it; anything but \"none\" means it was edited outside chezmoi) and `apply` (what chezmoi_apply would do: added, modified, deleted, or run for scripts). Empty means up to date.",
        properties: || {
            (
                json!({ "targets": targets_property("Limit to these targets. Default: all.") }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: TargetsArgs = args(arguments)?;
            let status = chezmoi.status(&a.targets)?;
            Ok(ToolOutput::json(
                json!({ "up_to_date": status.is_empty(), "entries": status }),
            ))
        },
    },
    Tool {
        name: "chezmoi_managed",
        title: "Managed files",
        description: "Targets chezmoi manages, with source paths, entry kind and attributes decoded from the source name (template, encrypted, private, …).",
        properties: || {
            (
                json!({ "filter": { "type": "string", "description": "Case-insensitive substring of the target or source path." } }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: ManagedArgs = args(arguments)?;
            let mut entries = chezmoi.managed()?;
            if let Some(filter) = a.filter.map(|f| f.to_lowercase()) {
                entries.retain(|e| {
                    e.target.to_lowercase().contains(&filter)
                        || e.source_relative.to_lowercase().contains(&filter)
                });
            }
            Ok(ToolOutput::json(managed_json(&entries)))
        },
    },
    Tool {
        name: "chezmoi_diff",
        title: "Diff of pending changes",
        description: "What chezmoi_apply would change, as a Git-format diff: removed lines are on disk now, added lines are what apply writes. Encrypted files are left out unless include_encrypted is true.",
        properties: || {
            (
                json!({
                    "targets": targets_property("Limit to these targets. Default: all."),
                    "include_encrypted": { "type": "boolean", "default": false, "description": "Include decrypted contents of encrypted files." },
                }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: DiffArgs = args(arguments)?;
            let options = DiffOptions {
                exclude: if a.include_encrypted {
                    vec![]
                } else {
                    vec![EntryType::Encrypted]
                },
            };
            let diff = chezmoi.diff(&a.targets, &options)?;
            Ok(ToolOutput {
                text: if diff.is_empty() {
                    "No differences.".into()
                } else {
                    diff.clone()
                },
                data: json!({ "diff": diff }),
            })
        },
    },
    Tool {
        name: "chezmoi_apply",
        title: "Apply changes",
        description: "Update targets to match the source state; runs scripts. A dry run unless dry_run is false. A target edited outside chezmoi fails with kind \"conflict\" (listing paths) unless force is true.",
        properties: || {
            (
                json!({
                    "targets": targets_property("Targets to apply. Default: all."),
                    "dry_run": { "type": "boolean", "default": true, "description": "Report what would change without writing. Set false to apply." },
                    "force": { "type": "boolean", "default": false, "description": "Overwrite targets edited outside chezmoi." },
                }),
                &[],
            )
        },
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: ApplyArgs = args(arguments)?;
            let outcome = chezmoi.apply(
                &a.targets,
                &ApplyOptions {
                    dry_run: a.dry_run,
                    force: a.force,
                    ..Default::default()
                },
            )?;
            Ok(ToolOutput::json(json!(outcome)))
        },
    },
    Tool {
        name: "chezmoi_add",
        title: "Add files",
        description: "Start managing files: copies them into the source state.",
        properties: || {
            (
                json!({
                    "paths": targets_property("Files or directories in the destination."),
                    "template": { "type": "boolean", "default": false, "description": "Add as templates." },
                    "encrypt": { "type": "boolean", "default": false, "description": "Encrypt in the source state (encryption must be configured in chezmoi)." },
                }),
                &["paths"],
            )
        },
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: AddArgs = args(arguments)?;
            chezmoi.add(
                &a.paths,
                &AddOptions {
                    template: a.template,
                    encrypt: a.encrypt,
                },
            )?;
            Ok(ToolOutput::json(json!({ "added": a.paths })))
        },
    },
    Tool {
        name: "chezmoi_re_add",
        title: "Keep local edits",
        description: "Copy edits made to targets back into the source state. Templates cannot be re-added and are reported in `skipped`. Default: every target edited outside chezmoi.",
        properties: || {
            (
                json!({ "targets": targets_property("Targets to re-add. Default: all edited outside chezmoi.") }),
                &[],
            )
        },
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: TargetsArgs = args(arguments)?;
            let targets = if a.targets.is_empty() {
                chezmoi
                    .status(&[])?
                    .into_iter()
                    .filter(|e| e.has_local_changes() && !e.is_script())
                    .map(|e| e.path)
                    .collect()
            } else {
                a.targets
            };
            if targets.is_empty() {
                return Ok(ToolOutput::json(json!({ "updated": [], "skipped": [] })));
            }
            Ok(ToolOutput::json(json!(chezmoi.re_add(&targets)?)))
        },
    },
    Tool {
        name: "chezmoi_forget",
        title: "Stop managing",
        description: "Remove targets from the source state. The files in the destination are left in place.",
        properties: || {
            (
                json!({ "targets": targets_property("Targets to stop managing.") }),
                &["targets"],
            )
        },
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: RequiredTargets = args(arguments)?;
            chezmoi.forget(&a.targets)?;
            Ok(ToolOutput::json(json!({ "forgotten": a.targets })))
        },
    },
    Tool {
        name: "git_status",
        title: "Source repository status",
        description: "Branch, upstream, commits ahead/behind and uncommitted changes in the source directory's git repository (local; run git_fetch to refresh ahead/behind).",
        properties: no_properties,
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            Ok(ToolOutput::json(json!(chezmoi.git()?.status()?)))
        },
    },
    Tool {
        name: "git_log",
        title: "Source repository history",
        description: "Recent commits in the source repository, newest first.",
        properties: || {
            (
                json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 20 } }),
                &[],
            )
        },
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: LogArgs = args(arguments)?;
            let commits = chezmoi.git()?.log(a.limit.clamp(1, 500))?;
            Ok(ToolOutput::json(json!({ "commits": commits })))
        },
    },
    Tool {
        name: "git_diff",
        title: "Uncommitted source changes",
        description: "Uncommitted changes in the source repository (staged, unstaged and untracked) as a Git diff. These are source files, so encrypted files appear encrypted.",
        properties: no_properties,
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            let diff = chezmoi.git()?.diff()?;
            Ok(ToolOutput {
                text: if diff.is_empty() {
                    "No uncommitted changes.".into()
                } else {
                    diff.clone()
                },
                data: json!({ "diff": diff }),
            })
        },
    },
    Tool {
        name: "git_commit",
        title: "Commit source changes",
        description: "Stage every change in the source repository and commit it.",
        properties: || {
            (
                json!({ "message": { "type": "string", "minLength": 1 } }),
                &["message"],
            )
        },
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
        run: |chezmoi, arguments| {
            let a: CommitArgs = args(arguments)?;
            let hash = chezmoi.git()?.commit_all(&a.message)?;
            Ok(ToolOutput::json(json!({ "commit": hash })))
        },
    },
    Tool {
        name: "git_fetch",
        title: "Fetch from remote",
        description: "Update remote-tracking branches, then report the repository status.",
        properties: no_properties,
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            let git = chezmoi.git()?;
            git.fetch()?;
            Ok(ToolOutput::json(json!(git.status()?)))
        },
    },
    Tool {
        name: "git_pull",
        title: "Pull from remote",
        description: "Pull with --rebase --autostash into the source directory only; nothing is applied. A conflicting pull is aborted and leaves the repository unchanged. Returns the pending chezmoi status to review.",
        properties: no_properties,
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            chezmoi.git()?.pull()?;
            Ok(ToolOutput::json(json!({ "pending": chezmoi.status(&[])? })))
        },
    },
    Tool {
        name: "git_push",
        title: "Push to remote",
        description: "Push the current branch of the source repository (never forced).",
        properties: no_properties,
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
        run: |chezmoi, arguments| {
            args::<NoArgs>(arguments)?;
            chezmoi.git()?.push()?;
            Ok(ToolOutput::json(json!({ "pushed": true })))
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(
            Err(Error::ChezmoiNotFound {
                searched: "test".into(),
            }),
            false,
        )
    }

    fn request(server: &mut Server, value: Value) -> Value {
        server.handle_line(&value.to_string()).expect("a response")
    }

    #[test]
    fn legacy_handshake_negotiates_a_version() {
        let mut s = server();
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "chezmate");
        assert!(r["result"].get("resultType").is_none());

        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}),
        );
        assert_eq!(r["result"]["protocolVersion"], LEGACY_VERSIONS[0]);

        assert!(
            s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        let r = request(&mut s, json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
        assert_eq!(r["result"], json!({}));
    }

    #[test]
    fn modern_requests_are_stateless() {
        let mut s = server();
        let meta = json!({ PROTOCOL_VERSION_KEY: "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {} });
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":"a","method":"tools/list","params":{"_meta": meta}}),
        );
        assert_eq!(r["id"], "a");
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["cacheScope"], "public");
        assert_eq!(r["result"]["_meta"][SERVER_INFO_KEY]["name"], "chezmate");
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.first(), Some(&"chezmoi_info"));
        assert!(names.contains(&"chezmoi_apply"));

        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":2,"method":"server/discover","params":{"_meta": meta}}),
        );
        assert_eq!(r["result"]["supportedVersions"][0], "2026-07-28");
        assert!(
            r["result"]["supportedVersions"]
                .as_array()
                .unwrap()
                .contains(&json!("2025-11-25"))
        );
    }

    #[test]
    fn rejects_unknown_protocol_versions() {
        let mut s = server();
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{ PROTOCOL_VERSION_KEY: "1900-01-01" }}}),
        );
        assert_eq!(r["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(r["error"]["data"]["requested"], "1900-01-01");
    }

    #[test]
    fn protocol_errors() {
        let mut s = server();
        let r = s.handle_line("{not json").unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        let r = request(&mut s, json!({"jsonrpc":"2.0","id":1,"method":"nope"}));
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}),
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = s
            .handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#)
            .unwrap();
        assert_eq!(r.as_array().unwrap().len(), 1);
    }

    #[test]
    fn tool_failures_are_results_the_model_can_read() {
        let mut s = server();
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"chezmoi_status","arguments":{}}}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["structuredContent"]["kind"],
            "chezmoi_not_found"
        );
    }

    #[test]
    fn read_only_mode_hides_mutating_tools() {
        let mut s = Server::new(
            Err(Error::ChezmoiNotFound {
                searched: "t".into(),
            }),
            true,
        );
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        );
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(
            tools
                .iter()
                .all(|t| t["annotations"]["readOnlyHint"] == true)
        );
        let r = request(
            &mut s,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"chezmoi_apply","arguments":{"dry_run":false}}}),
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn every_schema_lists_only_accepted_arguments() {
        for tool in TOOLS {
            let described = tool.describe();
            assert_eq!(described["inputSchema"]["type"], "object", "{}", tool.name);
            assert_eq!(described["annotations"]["readOnlyHint"], tool.read_only);
            if tool.read_only {
                assert!(
                    !tool.destructive,
                    "{} cannot be read-only and destructive",
                    tool.name
                );
            }
        }
    }
}
