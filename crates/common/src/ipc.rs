use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Request {
    Ping,
    Up { project: String },
    Down { project: String },
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Response {
    Pong,
    Ok,
    Error { message: String },
    Status { projects: Vec<ProjectRuntime> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRuntime {
    pub name: String,
    pub pid: u32,
    pub uptime_secs: u64,
}

fn encode_line<T: Serialize>(value: &T) -> Result<String> {
    let mut line = serde_json::to_string(value).context("failed to serialize IPC message")?;
    line.push('\n');
    Ok(line)
}

fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        bail!("empty IPC message");
    }
    if trimmed.contains('\n') {
        bail!("IPC message must be a single line");
    }
    serde_json::from_str(trimmed).context("failed to parse IPC message")
}

pub fn encode_request(req: &Request) -> Result<String> {
    encode_line(req)
}

pub fn decode_request(line: &str) -> Result<Request> {
    decode_line(line)
}

pub fn encode_response(res: &Response) -> Result<String> {
    encode_line(res)
}

pub fn decode_response(line: &str) -> Result<Response> {
    decode_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_json_contract() {
        let req = Request::Up {
            project: "demo".into(),
        };
        let encoded = encode_request(&req).unwrap();
        assert_eq!(encoded, "{\"Up\":{\"project\":\"demo\"}}\n");
        assert_eq!(decode_request(&encoded).unwrap(), req);
    }

    #[test]
    fn request_round_trips() {
        let cases = [
            Request::Ping,
            Request::Up {
                project: "demo".into(),
            },
            Request::Down {
                project: "demo".into(),
            },
            Request::Status,
        ];
        for case in cases {
            let line = encode_request(&case).unwrap();
            assert!(line.ends_with('\n'));
            assert_eq!(decode_request(&line).unwrap(), case);
        }
    }

    #[test]
    fn response_round_trips() {
        let cases = [
            Response::Pong,
            Response::Ok,
            Response::Error {
                message: "not running".into(),
            },
            Response::Status { projects: vec![] },
            Response::Status {
                projects: vec![ProjectRuntime {
                    name: "demo".into(),
                    pid: 42,
                    uptime_secs: 120,
                }],
            },
        ];
        for case in cases {
            let line = encode_response(&case).unwrap();
            assert!(line.ends_with('\n'));
            assert_eq!(decode_response(&line).unwrap(), case);
        }
    }

    #[test]
    fn decode_rejects_empty_and_multiline() {
        assert!(decode_request("").is_err());
        assert!(decode_request("\n").is_err());
        assert!(decode_request("{\"Ping\":null}\n{\"Status\":null}\n").is_err());
    }
}
