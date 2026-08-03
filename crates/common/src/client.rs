use crate::ipc::{decode_response, encode_request, Request, Response};
use crate::xdg::daemon_socket;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn call_daemon(request: &Request) -> Result<Response> {
    let path = daemon_socket()?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    let encoded = encode_request(request)?;
    stream
        .write_all(encoded.as_bytes())
        .context("failed to write request to nealsd")?;
    stream.flush().context("failed to flush request to nealsd")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .context("failed to read response from nealsd")?;
    if n == 0 {
        bail!("nealsd closed the connection without a response");
    }
    decode_response(&line)
}
