use anyhow::{anyhow, Result};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::fs::File;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, System};
use tokio::sync::mpsc;
use tokio::time::sleep;
use warp::{Filter, Rejection, Reply};
use tracing::info;

const MAX_LOG_LINES: usize = 1000;

// MCP Protocol structures
#[derive(Debug, Serialize, Deserialize)]
struct McpRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct McpResponse {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<McpError>,
}

#[derive(Debug, Serialize)]
struct McpError {
    code: i32,
    message: String,
}

#[derive(Debug, Serialize)]
struct Tool {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct LogEntry {
    timestamp: f64,
    line: String,
}

struct BreenixSession {
    process: Option<Child>,
    logs: VecDeque<LogEntry>,
    last_command_time: Instant,
    last_prompt_time: Instant,
    log_sender: mpsc::UnboundedSender<LogEntry>,
    log_file: Arc<Mutex<Option<File>>>,
}

impl BreenixSession {
    fn new(log_sender: mpsc::UnboundedSender<LogEntry>) -> Self {
        Self {
            process: None,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
            last_command_time: Instant::now(),
            last_prompt_time: Instant::now(),
            log_sender,
            log_file: Arc::new(Mutex::new(None)),
        }
    }

    fn start(&mut self, display: bool, testing: bool) -> Result<()> {
        if self.process.is_some() {
            return Err(anyhow!("Breenix is already running"));
        }
        
        // Ensure the log directory exists and create/truncate log file
        std::fs::create_dir_all("/tmp/breenix-mcp").ok();
        let log_path = "/tmp/breenix-mcp/kernel.log";
        
        // Open the log file once and share it between stdout/stderr readers
        let log_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path)
            .map_err(|e| anyhow!("Failed to open log file: {}", e))?;
        
        *self.log_file.lock().unwrap() = Some(log_file);

        let mut cmd = Command::new("cargo");
        if testing {
            cmd.args(&["run", "--features", "testing", "--bin", "qemu-uefi", "--", "-serial", "stdio"]);
        } else {
            cmd.args(&["run", "--bin", "qemu-uefi", "--", "-serial", "stdio"]);
        }
        
        if !display {
            cmd.args(&["-display", "none"]);
        }

        cmd.current_dir("..")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        eprintln!("Starting QEMU with command: {:?}", cmd);
        let mut child = cmd.spawn()
            .map_err(|e| {
                eprintln!("Failed to spawn QEMU: {}", e);
                anyhow!("Failed to spawn QEMU: {}", e)
            })?;
        eprintln!("QEMU process started with PID: {:?}", child.id());

        // Set up stdout reader
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let sender = self.log_sender.clone();
            let log_file = self.log_file.clone();
            
            std::thread::spawn(move || {
                for line in reader.lines() {
                    if let Ok(line) = line {
                        // Write to log file
                        if let Ok(mut guard) = log_file.lock() {
                            if let Some(ref mut writer) = *guard {
                                if let Err(e) = writeln!(writer, "{}", line) {
                                    eprintln!("❌ Failed to write to log file: {}", e);
                                } else {
                                    let _ = writer.flush();
                                }
                            }
                        }
                        
                        let entry = LogEntry {
                            timestamp: Utc::now().timestamp_millis() as f64 / 1000.0,
                            line,
                        };
                        let _ = sender.send(entry);
                    }
                }
            });
        }

        // Set up stderr reader
        if let Some(stderr) = child.stderr.take() {
            let reader = BufReader::new(stderr);
            let sender = self.log_sender.clone();
            let log_file = self.log_file.clone();
            
            std::thread::spawn(move || {
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let stderr_line = format!("[STDERR] {}", line);
                        
                        // Write to log file
                        if let Ok(mut guard) = log_file.lock() {
                            if let Some(ref mut writer) = *guard {
                                if let Err(e) = writeln!(writer, "{}", stderr_line) {
                                    eprintln!("❌ Failed to write to log file: {}", e);
                                } else {
                                    let _ = writer.flush();
                                }
                            }
                        }
                        
                        let entry = LogEntry {
                            timestamp: Utc::now().timestamp_millis() as f64 / 1000.0,
                            line: stderr_line,
                        };
                        let _ = sender.send(entry);
                    }
                }
            });
        }

        self.process = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        // Close the log file
        *self.log_file.lock().unwrap() = None;
        
        // First try to stop a managed process
        if let Some(mut process) = self.process.take() {
            process.kill()?;
            process.wait()?;
            Ok(())
        } else {
            // If no managed process, try to kill any QEMU processes
            let killed = kill_all_qemu()?;
            if killed > 0 {
                Ok(())
            } else {
                Err(anyhow!("No Breenix session to stop"))
            }
        }
    }

    fn is_running(&mut self) -> bool {
        // First check if we have a managed process
        if let Some(ref mut process) = self.process {
            match process.try_wait() {
                Ok(None) => return true,  // Still running
                _ => {
                    // Process has exited, clean up the reference
                    self.process = None;
                }
            }
        }
        
        // Also check if any QEMU process is running
        count_qemu_processes() > 0
    }

    fn send_command(&mut self, command: &str) -> Result<()> {
        if let Some(ref mut process) = self.process {
            if let Some(ref mut stdin) = process.stdin {
                writeln!(stdin, "{}", command)?;
                stdin.flush()?;
                self.last_command_time = Instant::now();
                Ok(())
            } else {
                Err(anyhow!("No stdin available"))
            }
        } else {
            Err(anyhow!("No Breenix session running"))
        }
    }

    fn get_logs(&self, n: Option<usize>, since_timestamp: Option<f64>) -> Vec<LogEntry> {
        let mut logs: Vec<_> = self.logs.iter().cloned().collect();
        
        if let Some(ts) = since_timestamp {
            logs.retain(|log| log.timestamp >= ts);
        }
        
        if let Some(n) = n {
            let start = logs.len().saturating_sub(n);
            logs = logs[start..].to_vec();
        }
        
        logs
    }

    fn update_logs(&mut self, entry: LogEntry) {
        // Check for prompt - Breenix outputs just "> " as prompt
        let trimmed = entry.line.trim();
        if trimmed == ">" || trimmed == "> " || trimmed.ends_with("> ") {
            self.last_prompt_time = Instant::now();
            eprintln!("🎯 Detected prompt: '{}'", trimmed);
        }
        
        self.logs.push_back(entry);
        if self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }
}

fn kill_all_qemu() -> Result<usize> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::everything());
    
    let mut killed = 0;
    for (pid, process) in sys.processes() {
        if process.name().contains("qemu-system") {
            if process.kill() {
                killed += 1;
                info!("Killed QEMU process {}", pid);
            }
        }
    }
    
    Ok(killed)
}

fn count_qemu_processes() -> usize {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::everything());
    
    sys.processes()
        .values()
        .filter(|process| process.name().contains("qemu-system"))
        .count()
}

// HTTP Request/Response types
#[derive(Debug, Deserialize)]
struct StartRequest {
    display: Option<bool>,
    testing: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SendCommandRequest {
    command: String,
}

#[derive(Debug, Deserialize)]
struct LogsRequest {
    lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WaitPromptRequest {
    timeout: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RunCommandRequest {
    command: String,
    wait_pattern: Option<String>,
    timeout: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl ApiResponse {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: None,
        }
    }

    fn ok_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: Some(data),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
}

// HTTP handlers
async fn handle_start(
    req: StartRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let display = req.display.unwrap_or(false);
    let testing = req.testing.unwrap_or(false);
    
    let start_result = {
        let mut session = session.lock().unwrap();
        session.start(display, testing)
    };
    
    match start_result {
        Ok(_) => {
            // Wait for boot
            sleep(Duration::from_secs(2)).await;
            Ok(warp::reply::json(&ApiResponse::ok("Breenix started successfully")))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::error(format!("Failed to start Breenix: {}", e))))
    }
}

async fn handle_stop(
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let mut session = session.lock().unwrap();
    match session.stop() {
        Ok(_) => Ok(warp::reply::json(&ApiResponse::ok("Breenix stopped"))),
        Err(e) => Ok(warp::reply::json(&ApiResponse::error(format!("Failed to stop: {}", e))))
    }
}

async fn handle_status(
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let mut session = session.lock().unwrap();
    let has_managed = session.process.is_some();
    let is_running = session.is_running();
    let qemu_count = count_qemu_processes();
    
    Ok(warp::reply::json(&ApiResponse::ok_with_data(
        "Status retrieved",
        serde_json::json!({
            "running": is_running,
            "qemu_processes": qemu_count,
            "mcp_managed": has_managed,
        })
    )))
}

async fn handle_kill_all() -> Result<impl Reply, Rejection> {
    match kill_all_qemu() {
        Ok(count) => Ok(warp::reply::json(&ApiResponse::ok(format!("Killed {} QEMU processes", count)))),
        Err(e) => Ok(warp::reply::json(&ApiResponse::error(format!("Failed to kill QEMU processes: {}", e))))
    }
}

async fn handle_send_command(
    req: SendCommandRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let mut session = session.lock().unwrap();
    
    match session.send_command(&req.command) {
        Ok(_) => Ok(warp::reply::json(&ApiResponse::ok(format!("Sent: {}", req.command)))),
        Err(e) => Ok(warp::reply::json(&ApiResponse::error(format!("Failed to send command: {}", e))))
    }
}

async fn handle_logs(
    req: LogsRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let lines = req.lines.unwrap_or(50);
    let session = session.lock().unwrap();
    let logs = session.get_logs(Some(lines), None);
    
    Ok(warp::reply::json(&ApiResponse::ok_with_data(
        "Logs retrieved",
        serde_json::json!({
            "logs": logs.iter().map(|entry| &entry.line).collect::<Vec<_>>()
        })
    )))
}

async fn handle_wait_prompt(
    req: WaitPromptRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let timeout = req.timeout.unwrap_or(5.0);
    let start = Instant::now();
    
    loop {
        let prompt_ready = {
            let session = session.lock().unwrap();
            session.last_prompt_time > session.last_command_time
        };
        
        if prompt_ready {
            return Ok(warp::reply::json(&ApiResponse::ok("Prompt ready")));
        }
        
        if start.elapsed().as_secs_f64() > timeout {
            return Ok(warp::reply::json(&ApiResponse::error("Timeout waiting for prompt")));
        }
        
        sleep(Duration::from_millis(100)).await;
    }
}

async fn handle_run_command(
    req: RunCommandRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    let before_time = Utc::now().timestamp_millis() as f64 / 1000.0;
    
    // Send command
    {
        let mut session = session.lock().unwrap();
        if let Err(e) = session.send_command(&req.command) {
            return Ok(warp::reply::json(&ApiResponse::error(format!("Failed to send command: {}", e))));
        }
    }
    
    // Wait for completion
    if let Some(pattern) = req.wait_pattern {
        let regex = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => {
                return Ok(warp::reply::json(&ApiResponse::error(format!("Invalid regex pattern: {}", e))));
            }
        };
        
        let timeout = req.timeout.unwrap_or(5.0);
        let start = Instant::now();
        
        loop {
            let (_found, output) = {
                let session = session.lock().unwrap();
                let logs = session.get_logs(None, Some(before_time));
                
                let mut found = false;
                for log in &logs {
                    if regex.is_match(&log.line) {
                        found = true;
                        break;
                    }
                }
                
                let output = if found {
                    Some(logs
                        .iter()
                        .map(|entry| &entry.line)
                        .cloned()
                        .collect::<Vec<_>>())
                } else {
                    None
                };
                
                (found, output)
            };
            
            if let Some(output) = output {
                return Ok(warp::reply::json(&ApiResponse::ok_with_data(
                    "Command completed",
                    serde_json::json!({ "output": output })
                )));
            }
            
            if start.elapsed().as_secs_f64() > timeout {
                return Ok(warp::reply::json(&ApiResponse::error(format!("Timeout waiting for pattern: {}", pattern))));
            }
            
            sleep(Duration::from_millis(100)).await;
        }
    } else {
        // Just wait a bit and return recent logs
        sleep(Duration::from_millis(500)).await;
        
        let output = {
            let session = session.lock().unwrap();
            let logs = session.get_logs(Some(20), None);
            logs.iter()
                .map(|entry| &entry.line)
                .cloned()
                .collect::<Vec<_>>()
        };
        
        Ok(warp::reply::json(&ApiResponse::ok_with_data(
            "Command sent",
            serde_json::json!({ "output": output })
        )))
    }
}

async fn handle_mcp_request(
    request: McpRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> McpResponse {
    match request.method.as_str() {
        "initialize" => {
            McpResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: Some(serde_json::json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "breenix-mcp",
                        "version": "1.0.0"
                    }
                })),
                error: None,
            }
        }
        
        "tools/list" => {
            let tools = vec![
                Tool {
                    name: "mcp__breenix__start".to_string(),
                    description: "Start Breenix in QEMU".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "display": {
                                "type": "boolean",
                                "description": "Show QEMU display window",
                                "default": false
                            },
                            "testing": {
                                "type": "boolean",
                                "description": "Enable kernel testing features",
                                "default": false
                            }
                        }
                    }),
                },
                Tool {
                    name: "mcp__breenix__stop".to_string(),
                    description: "Stop the running Breenix session".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
                Tool {
                    name: "mcp__breenix__running".to_string(),
                    description: "Check if Breenix is running".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
                Tool {
                    name: "mcp__breenix__kill".to_string(),
                    description: "Kill all QEMU processes".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
                Tool {
                    name: "mcp__breenix__send".to_string(),
                    description: "Send a command to Breenix via serial".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Command to send"
                            }
                        },
                        "required": ["command"]
                    }),
                },
                Tool {
                    name: "mcp__breenix__logs".to_string(),
                    description: "Get recent Breenix logs".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "lines": {
                                "type": "integer",
                                "description": "Number of recent lines to return",
                                "default": 50
                            }
                        }
                    }),
                },
                Tool {
                    name: "mcp__breenix__wait_prompt".to_string(),
                    description: "Wait for Breenix serial prompt to be ready".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "timeout": {
                                "type": "number",
                                "description": "Timeout in seconds",
                                "default": 5.0
                            }
                        }
                    }),
                },
                Tool {
                    name: "mcp__breenix__run_command".to_string(),
                    description: "Run a command and wait for completion".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Command to run"
                            },
                            "wait_pattern": {
                                "type": "string",
                                "description": "Regex pattern that indicates command completion"
                            },
                            "timeout": {
                                "type": "number",
                                "description": "Timeout in seconds",
                                "default": 5.0
                            }
                        },
                        "required": ["command"]
                    }),
                },
            ];

            McpResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: Some(serde_json::json!({ "tools": tools })),
                error: None,
            }
        }
        
        "tools/call" => {
            let params = match request.params {
                Some(p) => p,
                None => {
                    return McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        error: Some(McpError {
                            code: -32602,
                            message: "Invalid params".to_string(),
                        }),
                        result: None,
                    };
                }
            };

            let tool_name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];

            match tool_name {
                "mcp__breenix__start" => {
                    let display = args["display"].as_bool().unwrap_or(false);
                    let testing = args["testing"].as_bool().unwrap_or(false);
                    let mut session = session.lock().unwrap();
                    
                    match session.start(display, testing) {
                        Ok(_) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": "Breenix started successfully"
                                }]
                            })),
                            error: None,
                        },
                        Err(e) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Failed to start Breenix: {}", e)
                                }]
                            })),
                            error: None,
                        },
                    }
                }
                
                "mcp__breenix__stop" => {
                    let mut session = session.lock().unwrap();
                    match session.stop() {
                        Ok(_) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": "Breenix stopped"
                                }]
                            })),
                            error: None,
                        },
                        Err(e) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Failed to stop: {}", e)
                                }]
                            })),
                            error: None,
                        },
                    }
                }
                
                "mcp__breenix__running" => {
                    let mut session = session.lock().unwrap();
                    let has_managed = session.process.is_some();
                    let is_running = session.is_running();
                    let qemu_count = count_qemu_processes();
                    
                    McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        result: Some(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "Breenix: {}\nQEMU processes: {}\nMCP-managed: {}",
                                    if is_running { "running" } else { "not running" },
                                    qemu_count,
                                    if has_managed { "yes" } else { "no" }
                                )
                            }]
                        })),
                        error: None,
                    }
                }
                
                "mcp__breenix__kill" => {
                    match kill_all_qemu() {
                        Ok(count) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Killed {} QEMU processes", count)
                                }]
                            })),
                            error: None,
                        },
                        Err(e) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Failed to kill QEMU processes: {}", e)
                                }]
                            })),
                            error: None,
                        },
                    }
                }
                
                "mcp__breenix__send" => {
                    let command = args["command"].as_str().unwrap_or("");
                    let mut session = session.lock().unwrap();
                    
                    match session.send_command(command) {
                        Ok(_) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Sent: {}", command)
                                }]
                            })),
                            error: None,
                        },
                        Err(e) => McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Failed to send command: {}", e)
                                }]
                            })),
                            error: None,
                        },
                    }
                }
                
                "mcp__breenix__logs" => {
                    let lines = args["lines"].as_u64().unwrap_or(50) as usize;
                    let session = session.lock().unwrap();
                    let logs = session.get_logs(Some(lines), None);
                    let log_text = logs
                        .iter()
                        .map(|entry| &entry.line)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n");
                    
                    McpResponse {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        result: Some(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": log_text
                            }]
                        })),
                        error: None,
                    }
                }
                
                "mcp__breenix__wait_prompt" => {
                    let timeout = args["timeout"].as_f64().unwrap_or(5.0);
                    let start = Instant::now();
                    
                    loop {
                        let prompt_ready = {
                            let session = session.lock().unwrap();
                            session.last_prompt_time > session.last_command_time
                        };
                        
                        if prompt_ready {
                            return McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: request.id,
                                result: Some(serde_json::json!({
                                    "content": [{
                                        "type": "text",
                                        "text": "Prompt ready"
                                    }]
                                })),
                                error: None,
                            };
                        }
                        
                        if start.elapsed().as_secs_f64() > timeout {
                            return McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: request.id,
                                result: Some(serde_json::json!({
                                    "content": [{
                                        "type": "text",
                                        "text": "Timeout waiting for prompt"
                                    }]
                                })),
                                error: None,
                            };
                        }
                        
                        // Can't use async sleep here, need to refactor
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                
                "mcp__breenix__run_command" => {
                    let command = args["command"].as_str().unwrap_or("");
                    let wait_pattern = args["wait_pattern"].as_str();
                    let timeout = args["timeout"].as_f64().unwrap_or(5.0);
                    
                    let before_time = Utc::now().timestamp_millis() as f64 / 1000.0;
                    
                    // Send command
                    {
                        let mut session = session.lock().unwrap();
                        if let Err(e) = session.send_command(command) {
                            return McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: request.id,
                                result: Some(serde_json::json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!("Failed to send command: {}", e)
                                    }]
                                })),
                                error: None,
                            };
                        }
                    }
                    
                    // Wait for completion
                    if let Some(pattern) = wait_pattern {
                        let regex = match Regex::new(pattern) {
                            Ok(r) => r,
                            Err(e) => {
                                return McpResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id: request.id,
                                    result: Some(serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("Invalid regex pattern: {}", e)
                                        }]
                                    })),
                                    error: None,
                                };
                            }
                        };
                        
                        let start = Instant::now();
                        
                        loop {
                            let (found, output) = {
                                let session = session.lock().unwrap();
                                let logs = session.get_logs(None, Some(before_time));
                                
                                let mut found = false;
                                for log in &logs {
                                    if regex.is_match(&log.line) {
                                        found = true;
                                        break;
                                    }
                                }
                                
                                let output = logs
                                    .iter()
                                    .map(|entry| &entry.line)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                
                                (found, output)
                            };
                            
                            if found {
                                return McpResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id: request.id,
                                    result: Some(serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": output
                                        }]
                                    })),
                                    error: None,
                                };
                            }
                            
                            if start.elapsed().as_secs_f64() > timeout {
                                return McpResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id: request.id,
                                    result: Some(serde_json::json!({
                                        "content": [{
                                            "type": "text",
                                            "text": format!("Timeout waiting for pattern: {}", pattern)
                                        }]
                                    })),
                                    error: None,
                                };
                            }
                            
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    } else {
                        // Just wait a bit and return recent logs
                        std::thread::sleep(Duration::from_millis(500));
                        
                        let output = {
                            let session = session.lock().unwrap();
                            let logs = session.get_logs(Some(20), None);
                            logs.iter()
                                .map(|entry| &entry.line)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
                        
                        McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: Some(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": output
                                }]
                            })),
                            error: None,
                        }
                    }
                }
                
                _ => McpResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    error: Some(McpError {
                        code: -32601,
                        message: format!("Unknown tool: {}", tool_name),
                    }),
                    result: None,
                }
            }
        }
        
        _ => McpResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            error: Some(McpError {
                code: -32601,
                message: format!("Method not found: {}", request.method),
            }),
            result: None,
        }
    }
}

async fn handle_mcp(
    req: McpRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> Result<impl Reply, Rejection> {
    eprintln!("📨 MCP Request received: {} (id: {:?})", req.method, req.id);
    let response = handle_mcp_request(req, session).await;
    eprintln!("📤 Sending MCP response");
    Ok(warp::reply::json(&response))
}

async fn handle_health() -> Result<impl Reply, Rejection> {
    Ok(warp::reply::json(&ApiResponse::ok("Breenix MCP HTTP Server is running")))
}

pub async fn run_server(port: u16) -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    eprintln!("🚀 Starting Breenix MCP HTTP Server on port {}", port);
    
    // Initialize session
    let (log_sender, mut log_receiver) = mpsc::unbounded_channel();
    let session = Arc::new(Mutex::new(BreenixSession::new(log_sender)));
    eprintln!("✅ Session initialized");
    
    // Log receiver task
    let session_clone = session.clone();
    tokio::spawn(async move {
        while let Some(entry) = log_receiver.recv().await {
            let mut session = session_clone.lock().unwrap();
            session.update_logs(entry);
        }
    });
    
    // Define routes
    let session_filter = warp::any().map(move || session.clone());
    
    // MCP JSON-RPC route (root path)
    let mcp = warp::path::end()
        .and(warp::post())
        .and(warp::body::json())
        .and(session_filter.clone())
        .and_then(handle_mcp);
    
    let health = warp::path("health")
        .and(warp::get())
        .and_then(handle_health);
    
    let start = warp::path("start")
        .and(warp::post())
        .and(warp::body::json())
        .and(session_filter.clone())
        .and_then(handle_start);
    
    let stop = warp::path("stop")
        .and(warp::post())
        .and(session_filter.clone())
        .and_then(handle_stop);
    
    let status = warp::path("status")
        .and(warp::get())
        .and(session_filter.clone())
        .and_then(handle_status);
    
    let kill_all = warp::path("kill-all")
        .and(warp::post())
        .and_then(handle_kill_all);
    
    let send = warp::path("send")
        .and(warp::post())
        .and(warp::body::json())
        .and(session_filter.clone())
        .and_then(handle_send_command);
    
    let logs = warp::path("logs")
        .and(warp::get())
        .and(warp::query())
        .and(session_filter.clone())
        .and_then(handle_logs);
    
    let wait_prompt = warp::path("wait-prompt")
        .and(warp::post())
        .and(warp::body::json())
        .and(session_filter.clone())
        .and_then(handle_wait_prompt);
    
    let run_command = warp::path("run-command")
        .and(warp::post())
        .and(warp::body::json())
        .and(session_filter.clone())
        .and_then(handle_run_command);
    
    let routes = mcp
        .or(health)
        .or(start)
        .or(stop)
        .or(status)
        .or(kill_all)
        .or(send)
        .or(logs)
        .or(wait_prompt)
        .or(run_command);
    
    info!("Starting Breenix MCP HTTP Server on port {}", port);
    
    warp::serve(routes)
        .run(([127, 0, 0, 1], port))
        .await;
    
    Ok(())
}