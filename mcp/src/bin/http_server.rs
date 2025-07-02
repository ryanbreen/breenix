use anyhow::Result;
use breenix_mcp::http_server;

#[tokio::main]
async fn main() -> Result<()> {
    let port = std::env::var("BREENIX_MCP_PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("Invalid port number");
    
    http_server::run_server(port).await
}