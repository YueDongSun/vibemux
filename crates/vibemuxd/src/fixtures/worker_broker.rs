//! Synthetic controller transport to the actual file-only MCP broker.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
use vibemux_harness::dispatch::writable_profile::WritableGrant;
use vibemux_worker_tools::contract::MAX_FRAME_BYTES;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(super) struct BrokerClient {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u64,
    candidate: PathBuf,
}

impl BrokerClient {
    pub(super) fn launch(arguments: &[String]) -> Result<Option<Self>> {
        let flags = [
            "--fixture_broker",
            "--broker_bootstrap",
            "--broker_scratch",
            "--broker_candidate",
        ];
        if !arguments.iter().any(|arg| flags.contains(&arg.as_str())) {
            return Ok(None);
        }
        let values = flags.map(|flag| {
            let indices: Vec<_> = arguments
                .iter()
                .enumerate()
                .filter_map(|(index, arg)| (arg == flag).then_some(index))
                .collect();
            if indices.len() != 1 {
                return Err("fixture_broker_argument_invalid");
            }
            let path = arguments
                .get(indices[0] + 1)
                .map(PathBuf::from)
                .ok_or("fixture_broker_argument_missing")?;
            if !path.is_absolute() {
                return Err("fixture_broker_absolute_path_required");
            }
            Ok(path)
        });
        let [executable, bootstrap, scratch, candidate] = values;
        let candidate = candidate?;
        let mut command = Command::new(executable?);
        command
            .args(["--bootstrap"])
            .arg(bootstrap?)
            .arg("--scratch-parent")
            .arg(scratch?)
            .arg("--candidate-out")
            .arg(&candidate)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in ["SystemRoot", "WINDIR", "SystemDrive", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take().ok_or("fixture_broker_stdin_missing")?;
        let output = BufReader::new(child.stdout.take().ok_or("fixture_broker_stdout_missing")?);
        let mut client = Self {
            child,
            input: Some(input),
            output,
            next_id: 1,
            candidate,
        };
        let result = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "synthetic_workflow_controller", "version": "1"}
            }),
        )?;
        if result["protocolVersion"] != "2025-06-18" {
            return Err("fixture_broker_initialize_invalid".into());
        }
        client.send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))?;
        Ok(Some(client))
    }

    fn send(&mut self, message: Value) -> Result<()> {
        let bytes = serde_json::to_vec(&message)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err("fixture_broker_request_limit".into());
        }
        let input = self.input.as_mut().ok_or("fixture_broker_closed")?;
        input.write_all(&bytes)?;
        input.write_all(b"\n")?;
        input.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        let mut bytes = Vec::new();
        self.output
            .by_ref()
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES || bytes.last() != Some(&b'\n') {
            return Err("fixture_broker_response_invalid".into());
        }
        let response: Value = serde_json::from_slice(&bytes)?;
        if response["jsonrpc"] != "2.0" || response["id"] != id || response.get("error").is_some() {
            return Err("fixture_broker_response_invalid".into());
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "fixture_broker_result_missing".into())
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<std::result::Result<Value, String>> {
        let reply = self.request("tools/call", json!({"name":name, "arguments":arguments}))?;
        let text = reply["content"][0]["text"]
            .as_str()
            .ok_or("fixture_broker_content_invalid")?;
        match reply["isError"].as_bool() {
            Some(true) => Ok(Err(text.to_owned())),
            Some(false) => Ok(Ok(serde_json::from_str(text)?)),
            None => Err("fixture_broker_content_invalid".into()),
        }
    }

    pub(super) fn read(&mut self, path: &str) -> Result<()> {
        self.call("read_file", json!({"path":path}))?
            .map_err(|_| "fixture_broker_read_refused")?;
        Ok(())
    }

    pub(super) fn search(&mut self, prefix: &str) -> Result<()> {
        self.call("search_paths", json!({"prefix":prefix}))?
            .map_err(|_| "fixture_broker_search_refused")?;
        Ok(())
    }

    pub(super) fn write(&mut self, path: &str, content: &str, grant: WritableGrant) -> Result<()> {
        // A definitive missing response permits create; no unknown outcome is retried.
        if grant.edit_files {
            match self.call(
                "edit_file",
                json!({"path":path, "expected_revision":0, "content":content}),
            )? {
                Ok(_) => return Ok(()),
                Err(code) if code == "worker_tools_missing" && grant.write_files => (),
                Err(_) => return Err("fixture_broker_edit_refused".into()),
            }
        } else if !grant.write_files {
            return Err("fixture_broker_write_refused".into());
        }
        self.call("create_file", json!({"path":path, "content":content}))?
            .map_err(|_| "fixture_broker_create_refused")?;
        Ok(())
    }

    pub(super) fn finish(&mut self, fault: Option<&str>) -> Result<()> {
        self.input.take();
        if !self.child.wait()?.success() {
            return Err("fixture_broker_failed".into());
        }
        // Explicit synthetic publication faults exercise owner-side admission.
        match fault {
            None => (),
            Some("missing") => std::fs::remove_file(&self.candidate)?,
            Some("partial") => std::fs::write(&self.candidate, b"{")?,
            Some("stale_attempt" | "stale_snapshot") => {
                let mut value: Value = serde_json::from_slice(&std::fs::read(&self.candidate)?)?;
                if fault == Some("stale_attempt") {
                    value["candidate"]["attempt_id"] = json!(uuid::Uuid::nil());
                } else {
                    value["candidate"]["base_snapshot"] = json!("0".repeat(64));
                }
                std::fs::write(&self.candidate, serde_json::to_vec(&value)?)?;
            }
            Some(_) => return Err("fixture_broker_fault_invalid".into()),
        }
        Ok(())
    }
}
