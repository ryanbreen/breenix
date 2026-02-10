//! QMP (QEMU Machine Protocol) client over Unix domain socket.
//!
//! Provides commands needed for BTRT extraction:
//! - Connect and negotiate capabilities
//! - Stop VM execution
//! - Save physical memory to file (pmemsave)
//! - Quit QEMU

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// A QMP client connected to a QEMU instance.
pub struct QmpClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl QmpClient {
    /// Connect to a QMP socket and negotiate capabilities.
    pub fn connect<P: AsRef<Path>>(path: P) -> Result<Self> {
        let stream = UnixStream::connect(path.as_ref())
            .context("Failed to connect to QMP socket")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .context("Failed to set read timeout")?;

        let reader = BufReader::new(stream.try_clone()?);
        let mut client = QmpClient { stream, reader };

        // Read the QMP greeting
        let greeting = client.read_response()?;
        if !greeting.contains("\"QMP\"") {
            bail!("Unexpected QMP greeting: {}", greeting);
        }

        // Negotiate capabilities
        client.send_command(r#"{"execute": "qmp_capabilities"}"#)?;
        let resp = client.read_response()?;
        if !resp.contains("\"return\"") {
            bail!("QMP capabilities negotiation failed: {}", resp);
        }

        Ok(client)
    }

    /// Stop VM execution (pause).
    pub fn stop(&mut self) -> Result<()> {
        self.send_command(r#"{"execute": "stop"}"#)?;
        let resp = self.read_response()?;
        // May get an event before the return
        if resp.contains("\"return\"") {
            return Ok(());
        }
        // Read again for the return
        let resp2 = self.read_response()?;
        if !resp2.contains("\"return\"") {
            bail!("QMP stop failed: {} / {}", resp, resp2);
        }
        Ok(())
    }

    /// Save physical memory to a file.
    pub fn pmemsave(&mut self, addr: u64, size: u64, filename: &str) -> Result<()> {
        let cmd = format!(
            r#"{{"execute": "pmemsave", "arguments": {{"val": {}, "size": {}, "filename": "{}"}}}}"#,
            addr, size, filename
        );
        self.send_command(&cmd)?;
        let resp = self.read_response()?;
        if !resp.contains("\"return\"") {
            bail!("QMP pmemsave failed: {}", resp);
        }
        Ok(())
    }

    /// Quit QEMU.
    pub fn quit(&mut self) -> Result<()> {
        self.send_command(r#"{"execute": "quit"}"#)?;
        // Don't wait for response -- QEMU may exit immediately
        Ok(())
    }

    fn send_command(&mut self, cmd: &str) -> Result<()> {
        self.stream
            .write_all(cmd.as_bytes())
            .context("Failed to send QMP command")?;
        self.stream
            .write_all(b"\n")
            .context("Failed to send newline")?;
        self.stream.flush().context("Failed to flush QMP socket")?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<String> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .context("Failed to read QMP response")?;
        Ok(line)
    }
}
