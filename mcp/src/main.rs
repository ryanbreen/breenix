use anyhow::{anyhow, Result};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, System};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::sleep;

const MAX_LOG_LINES: usize = 1000;

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
}

impl BreenixSession {
    fn new(log_sender: mpsc::UnboundedSender<LogEntry>) -> Self {
        Self {
            process: None,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
            last_command_time: Instant::now(),
            last_prompt_time: Instant::now(),
            log_sender,
        }
    }

    fn start(&mut self, display: bool, testing: bool) -> Result<()> {
        if self.process.is_some() {
            return Err(anyhow!("Breenix is already running"));
        }
        
        // Kill any existing QEMU processes before starting
        let _ = kill_all_qemu();

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
            .map_err(|e| anyhow!("Failed to spawn QEMU: {}", e))?;

        // Set up stdout reader
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let sender = self.log_sender.clone();
            
            std::thread::spawn(move || {
                for line in reader.lines() {
                    if let Ok(line) = line {
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
            
            std::thread::spawn(move || {
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let entry = LogEntry {
                            timestamp: Utc::now().timestamp_millis() as f64 / 1000.0,
                            line: format!("[STDERR] {}", line),
                        };
                        let _ = sender.send(entry);
                    }
                }
            });
        }

        self.process = Some(child);
        eprintln!("QEMU process started successfully");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
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
        // Check for serial command task started - this indicates the kernel is ready
        if entry.line.contains("kernel::serial::command: Serial command task started") {
            self.last_prompt_time = Instant::now();
            eprintln!("🎯 Detected kernel ready: Serial command task started");
        }
        
        // Also check for traditional prompt - Breenix outputs just "> " as prompt
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
                eprintln!("Killed QEMU process {}", pid);
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

async fn handle_request(
    request: McpRequest,
    session: Arc<Mutex<BreenixSession>>,
) -> McpResponse {
    eprintln!("🎯 Handling request method: {}", request.method);
    
    match request.method.as_str() {
        "initialize" => {
            eprintln!("🤝 Processing initialize request");
            // Handle initialization request
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
                    description: "Stop the Breenix QEMU session".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
                Tool {
                    name: "mcp__breenix__running".to_string(),
                    description: "Check if Breenix is running and count QEMU processes".to_string(),
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
            eprintln!("🔨 Processing tools/call request");
            let params = match request.params {
                Some(p) => p,
                None => {
                    eprintln!("❌ No params provided for tools/call");
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
            eprintln!("🛠️  Tool: {} with args: {:?}", tool_name, args);

            match tool_name {
                "mcp__breenix__start" => {
                    let display = args["display"].as_bool().unwrap_or(false);
                    let testing = args["testing"].as_bool().unwrap_or(false);
                    
                    // First kill any existing QEMU processes
                    let _ = kill_all_qemu();
                    
                    let mut session = session.lock().unwrap();
                    
                    match session.start(display, testing) {
                        Ok(_) => {
                            drop(session);
                            // Wait for boot
                            sleep(Duration::from_secs(2)).await;
                            
                            McpResponse {
                                jsonrpc: "2.0".to_string(),
                                id: request.id,
                                result: Some(serde_json::json!({
                                    "content": [{
                                        "type": "text",
                                        "text": "Breenix started successfully"
                                    }]
                                })),
                                error: None,
                            }
                        }
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
                
                
                "mcp__breenix__wait_prompt" => {
                    let timeout = args["timeout"].as_f64().unwrap_or(5.0);
                    let start = Instant::now();
                    
                    loop {
                        // Check the log file directly for the serial command task started message
                        let output = match Command::new("grep")
                            .args(&["-q", "kernel::serial::command: Serial command task started", "/tmp/breenix-mcp/kernel.log"])
                            .status()
                        {
                            Ok(status) => status.success(),
                            Err(_) => false,
                        };
                        
                        if output {
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
                        
                        sleep(Duration::from_millis(100)).await;
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
                            let session = session.lock().unwrap();
                            let logs = session.get_logs(None, Some(before_time));
                            
                            for log in &logs {
                                if regex.is_match(&log.line) {
                                    let output = logs
                                        .iter()
                                        .map(|entry| &entry.line)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    
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
                            }
                            drop(session);
                            
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
                            
                            sleep(Duration::from_millis(100)).await;
                        }
                    } else {
                        // Just wait a bit and return recent logs
                        sleep(Duration::from_millis(500)).await;
                        
                        let session = session.lock().unwrap();
                        let logs = session.get_logs(Some(20), None);
                        let output = logs
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
                },
            }
        }
        
        _ => McpResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            error: Some(McpError {
                code: -32601,
                message: "Method not found".to_string(),
            }),
            result: None,
        },
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("🚀 Breenix MCP Server starting...");
    
    // Initialize
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
    
    // Main loop
    eprintln!("🎧 Listening for MCP requests on stdin...");
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut line = String::new();
    
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                eprintln!("📭 EOF received, shutting down...");
                break; // EOF
            }
            Ok(n) => {
                eprintln!("📨 Received {} bytes: {}", n, line.trim());
                match serde_json::from_str::<McpRequest>(&line) {
                    Ok(request) => {
                        eprintln!("🔧 Processing request: {} (id: {:?})", request.method, request.id);
                        let response = handle_request(request, session.clone()).await;
                        let response_str = serde_json::to_string(&response)?;
                        eprintln!("📤 Sending response: {}", response_str);
                        println!("{}", response_str);
                        tokio::io::stdout().flush().await?;
                    }
                    Err(e) => {
                        eprintln!("❌ Parse error: {}", e);
                        let error_response = McpResponse {
                            jsonrpc: "2.0".to_string(),
                            id: None,
                            error: Some(McpError {
                                code: -32700,
                                message: format!("Parse error: {}", e),
                            }),
                            result: None,
                        };
                        let error_str = serde_json::to_string(&error_response)?;
                        eprintln!("📤 Sending error response: {}", error_str);
                        println!("{}", error_str);
                        tokio::io::stdout().flush().await?;
                    }
                }
            }
            Err(e) => {
                eprintln!("💥 Error reading stdin: {}", e);
                break;
            }
        }
    }
    
    // Cleanup
    let mut session = session.lock().unwrap();
    let _ = session.stop();
    
    Ok(())
}