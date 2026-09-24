//! A local stand-in for the Gemini API, for the release performance run (TEST_PLAN S7
//! `memory_1000`, S10 `stream_fps_1000`; BACKLOG B14). The app is built with its `perf`
//! feature and pointed here, so 1,000 respondents stream through the real engine, IPC and UI
//! without a key, a network or a bill.
//!
//!   mock-gemini [--port 8787] [--delay-ms 300]
//!
//! Answers `GET /v1beta/models` and `POST /v1beta/models/<model>:generateContent`. Replies
//! are chosen by the system prompt: survey answers (valid for every question type), theme
//! coding, or the synthesis. `--delay-ms` is added to every call, like Gemini's latency.
//! Plain HTTP/1.1 with keep-alive; binds to 127.0.0.1 only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use survey_core::engine::answer::reply_for_prompt;
use survey_evals::flag;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const MODELS: [&str; 2] = ["gemini-2.5-flash", "gemini-2.5-pro"];

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port: u16 = flag(&args, "--port").map_or(8787, |p| p.parse().expect("--port"));
    let delay = Duration::from_millis(
        flag(&args, "--delay-ms").map_or(300, |d| d.parse().expect("--delay-ms")),
    );
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("port is free");
    println!("mock-gemini listening on http://127.0.0.1:{port} (delay {delay:?})");
    let calls = Arc::new(AtomicU64::new(0));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let calls = calls.clone();
        tokio::spawn(async move {
            let _ = serve(stream, delay, calls).await;
        });
    }
}

async fn serve(stream: TcpStream, delay: Duration, calls: Arc<AtomicU64>) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await? == 0 {
            return Ok(());
        }
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).await?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).await?;

        let path = request_line.split_whitespace().nth(1).unwrap_or_default();
        let (status, reply) = if request_line.starts_with("GET ") && path.ends_with("/models") {
            let models: Vec<Value> = MODELS
                .iter()
                .map(|m| json!({ "name": format!("models/{m}") }))
                .collect();
            (200, json!({ "models": models }))
        } else if request_line.starts_with("POST ") && path.ends_with(":generateContent") {
            tokio::time::sleep(delay).await;
            let n = calls.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 1000 == 0 {
                println!("{n} calls answered");
            }
            (
                200,
                generate(&serde_json::from_slice(&body).unwrap_or(Value::Null)),
            )
        } else {
            (
                404,
                json!({ "error": { "code": 404, "message": "not mocked" } }),
            )
        };
        let text = reply.to_string();
        let head = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            if status == 200 { "OK" } else { "Not Found" },
            text.len()
        );
        write.write_all(head.as_bytes()).await?;
        write.write_all(text.as_bytes()).await?;
        write.flush().await?;
    }
}

/// The same replies the engine's own mock-server test uses (`engine::run` tests).
fn generate(body: &Value) -> Value {
    let system = body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let prompt = body["contents"][0]["parts"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let reply = if system.contains("survey as the person") {
        reply_for_prompt(prompt)
    } else if system.contains("code open-ended") {
        json!({ "themes": [{ "label": "Price", "description": "Mentions what it costs.", "answers": [1, 2] }] })
    } else {
        json!({ "summary": "Answers were mixed.", "friction_points": [], "segments": [] })
    };
    json!({
        "candidates": [{ "content": { "parts": [{ "text": reply.to_string() }] }, "finishReason": "STOP" }],
        "usageMetadata": { "promptTokenCount": 1200, "candidatesTokenCount": 400 }
    })
}
