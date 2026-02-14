//! burl - Breenix URL transfer tool
//!
//! A curl-like CLI for HTTP and HTTPS requests.
//!
//! Usage: burl [OPTIONS] URL
//!
//! Options:
//!   -X METHOD        HTTP method (GET, POST, HEAD) [default: GET]
//!   -H "Name: Value" Add custom header (repeatable)
//!   -d DATA          Request body (implies POST)
//!   -v               Verbose (show request/response headers on stderr)
//!   -I               Headers only (HEAD request)
//!   -o FILE          Write body to file instead of stdout
//!   -s               Silent mode (no progress/errors)
//!   -k               Insecure (skip certificate validation)
//!   --help           Show help

use libbreenix::http::{
    http_request, HttpError, HttpMethod, HttpRequest, MAX_RESPONSE_SIZE,
};
use std::env;
use std::process;

fn print_usage() {
    eprintln!("Usage: burl [OPTIONS] URL");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -X METHOD        HTTP method (GET, POST, HEAD) [default: GET]");
    eprintln!("  -H \"Name: Value\" Add custom header (repeatable)");
    eprintln!("  -d DATA          Request body (implies POST)");
    eprintln!("  -v               Verbose (show headers on stderr)");
    eprintln!("  -I               Headers only (HEAD request)");
    eprintln!("  -o FILE          Write body to file");
    eprintln!("  -s               Silent mode");
    eprintln!("  -k               Insecure (skip certificate validation)");
    eprintln!("  --help           Show this help");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    let mut method = HttpMethod::Get;
    let mut headers: Vec<String> = Vec::new();
    let mut body: Option<String> = None;
    let mut verbose = false;
    let mut head_only = false;
    let mut output_file: Option<String> = None;
    let mut silent = false;
    let mut insecure = false;
    let mut url: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                print_usage();
                process::exit(0);
            }
            "-X" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("burl: -X requires a method argument");
                    process::exit(1);
                }
                method = match args[i].to_uppercase().as_str() {
                    "GET" => HttpMethod::Get,
                    "POST" => HttpMethod::Post,
                    "HEAD" => HttpMethod::Head,
                    "PUT" => HttpMethod::Put,
                    "DELETE" => HttpMethod::Delete,
                    _ => {
                        eprintln!("burl: unsupported method '{}'", args[i]);
                        process::exit(1);
                    }
                };
            }
            "-H" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("burl: -H requires a header argument");
                    process::exit(1);
                }
                headers.push(args[i].clone());
            }
            "-d" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("burl: -d requires a data argument");
                    process::exit(1);
                }
                body = Some(args[i].clone());
                // -d implies POST unless explicitly set
                if !args.iter().any(|a| a == "-X") {
                    method = HttpMethod::Post;
                }
            }
            "-v" => verbose = true,
            "-I" => {
                head_only = true;
                method = HttpMethod::Head;
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("burl: -o requires a filename argument");
                    process::exit(1);
                }
                output_file = Some(args[i].clone());
            }
            "-s" => silent = true,
            "-k" => insecure = true,
            arg if arg.starts_with('-') => {
                eprintln!("burl: unknown option '{}'", arg);
                process::exit(1);
            }
            _ => {
                url = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let url = match url {
        Some(u) => u,
        None => {
            if !silent {
                eprintln!("burl: no URL specified");
            }
            process::exit(1);
        }
    };

    // Build header refs
    let header_strs: Vec<&str> = headers.iter().map(|s| s.as_str()).collect();

    let request = HttpRequest {
        method,
        url: &url,
        headers: &header_strs,
        body: body.as_deref().map(|s| s.as_bytes()),
        insecure,
    };

    let mut response_buf = vec![0u8; MAX_RESPONSE_SIZE];

    match http_request(&request, &mut response_buf) {
        Ok((response, total_len)) => {
            if verbose {
                // Print response headers to stderr
                if let Some(headers_end) = response.body_offset.checked_sub(0) {
                    let header_bytes = &response_buf[..headers_end.min(total_len)];
                    if let Ok(header_str) = core::str::from_utf8(header_bytes) {
                        for line in header_str.lines() {
                            eprintln!("< {}", line);
                        }
                    }
                }
            }

            if head_only {
                // Print headers to stdout
                let header_bytes = &response_buf[..response.body_offset.min(total_len)];
                if let Ok(header_str) = core::str::from_utf8(header_bytes) {
                    print!("{}", header_str);
                }
            } else {
                // Print or save body
                let body_end = response.body_offset + response.body_len;
                let body = &response_buf[response.body_offset..body_end.min(total_len)];

                if let Some(ref _filename) = output_file {
                    // TODO: write to file when filesystem write is available
                    if !silent {
                        eprintln!("burl: -o not yet implemented, writing to stdout");
                    }
                    print_body(body);
                } else {
                    print_body(body);
                }
            }
        }
        Err(e) => {
            if !silent {
                eprintln!("burl: {}", error_message(&e));
            }
            process::exit(1);
        }
    }
}

fn print_body(body: &[u8]) {
    // Try to print as UTF-8, fall back to lossy
    if let Ok(s) = core::str::from_utf8(body) {
        print!("{}", s);
    } else {
        let s = String::from_utf8_lossy(body);
        print!("{}", s);
    }
}

fn error_message(e: &HttpError) -> &'static str {
    match e {
        HttpError::UrlTooLong => "URL too long",
        HttpError::InvalidUrl => "invalid URL",
        HttpError::DnsError(_) => "DNS resolution failed",
        HttpError::SocketError => "socket error",
        HttpError::ConnectError => "connection failed",
        HttpError::SendError => "send failed",
        HttpError::RecvError => "receive failed",
        HttpError::Timeout => "connection timed out",
        HttpError::ResponseTooLarge => "response too large",
        HttpError::ParseError => "failed to parse response",
        HttpError::TlsError => "TLS error",
    }
}
