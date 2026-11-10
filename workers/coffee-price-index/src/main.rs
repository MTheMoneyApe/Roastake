//! ROASTAKE Coffee Price Index WASI Worker
//!
//! Entry point for OutLayer verifiable execution.
//! Reads JSON request from stdin, processes deterministic price calculation,
//! emits single JSON response on stdout, and writes diagnostics to stderr.

mod engine;
mod provider;

use std::io::{self, Read, Write};
use std::process;

fn main() {
    eprintln!("[roastake-worker] Starting Coffee Price Index WASI worker...");

    let mut input_buffer = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut input_buffer) {
        eprintln!("[roastake-worker] FATAL: failed to read from stdin: {}", e);
        process::exit(1);
    }

    eprintln!(
        "[roastake-worker] Received stdin payload ({} bytes). Processing...",
        input_buffer.len()
    );

    match engine::process_raw_json(&input_buffer) {
        Ok(json_output) => {
            // Write exact JSON payload to stdout
            print!("{}", json_output);
            if let Err(e) = io::stdout().flush() {
                eprintln!("[roastake-worker] FATAL: failed to flush stdout: {}", e);
                process::exit(1);
            }
            eprintln!("[roastake-worker] Output successfully written to stdout.");
            process::exit(0);
        }
        Err(err) => {
            eprintln!("[roastake-worker] ERROR: processing rejected: {}", err);
            process::exit(1);
        }
    }
}
