use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use tokscale_core::telemetry::{AccountDailyUsageInput, AccountUsageSummaryInput, TelemetryStore};

use super::{
    derive_account_id, load_credentials_store, matching_account_id_for_tokens,
    read_current_credentials,
};

const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);
const INITIALIZE_REQUEST_ID: i64 = 1;
const ACTIVITY_REQUEST_ID: i64 = 2;
const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;
const MAX_PENDING_FRAMES: usize = 16;
const APP_SERVER_SOURCE: &str = "codex-app-server";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppServerActivityResult {
    #[serde(default)]
    summary: Option<AppServerActivitySummary>,
    #[serde(default)]
    daily_usage_buckets: Option<Vec<AppServerDailyUsageBucket>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppServerActivitySummary {
    lifetime_tokens: Option<u64>,
    peak_daily_tokens: Option<u64>,
    longest_running_turn_sec: Option<u64>,
    current_streak_days: Option<u64>,
    longest_streak_days: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppServerDailyUsageBucket {
    start_date: String,
    tokens: u64,
}

#[derive(Debug)]
enum ActivityFetchError {
    UnsupportedCli,
    UnsupportedAuth,
    Unavailable(&'static str),
}

impl fmt::Display for ActivityFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCli => write!(
                formatter,
                "the installed Codex CLI does not support account activity"
            ),
            Self::UnsupportedAuth => write!(
                formatter,
                "Codex account activity requires supported Codex authentication"
            ),
            Self::Unavailable(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ActivityFetchError {}

enum AppServerFrame {
    Json(String),
    Oversized,
    InvalidUtf8,
}

struct AppServerTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Option<mpsc::Receiver<AppServerFrame>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
}

impl AppServerTransport {
    fn spawn() -> std::result::Result<Self, ActivityFetchError> {
        let mut command = Command::new("codex");
        command
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_app_server_process(&mut command);
        let mut child = command.spawn().map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => ActivityFetchError::UnsupportedCli,
            _ => ActivityFetchError::Unavailable("could not start Codex app-server"),
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
            terminate_app_server_process_tree(&mut child);
            return Err(ActivityFetchError::Unavailable(
                "Codex app-server did not expose its required streams",
            ));
        };
        let (sender, frames) = mpsc::sync_channel(MAX_PENDING_FRAMES);

        Ok(Self {
            child,
            stdin: Some(stdin),
            frames: Some(frames),
            stdout_reader: Some(spawn_stdout_reader(stdout, sender)),
            stderr_reader: Some(spawn_stderr_drain(stderr)),
        })
    }

    fn write_message(&mut self, message: &Value) -> std::result::Result<(), ActivityFetchError> {
        let stdin = self.stdin.as_mut().ok_or(ActivityFetchError::Unavailable(
            "Codex app-server closed its input",
        ))?;
        serde_json::to_writer(&mut *stdin, message)
            .map_err(|_| ActivityFetchError::Unavailable("could not encode app-server input"))?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|_| ActivityFetchError::Unavailable("Codex app-server closed its input"))
    }

    fn wait_for_response(
        &mut self,
        expected_id: i64,
        deadline: Instant,
    ) -> std::result::Result<Value, ActivityFetchError> {
        loop {
            let remaining = deadline.checked_duration_since(Instant::now()).ok_or(
                ActivityFetchError::Unavailable("timed out waiting for Codex account activity"),
            )?;
            let frame = self
                .frames
                .as_ref()
                .ok_or(ActivityFetchError::Unavailable(
                    "Codex app-server closed before returning account activity",
                ))?
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => ActivityFetchError::Unavailable(
                        "timed out waiting for Codex account activity",
                    ),
                    mpsc::RecvTimeoutError::Disconnected => ActivityFetchError::Unavailable(
                        "Codex app-server closed before returning account activity",
                    ),
                })?;
            let line = match frame {
                AppServerFrame::Json(line) => line,
                AppServerFrame::Oversized => {
                    return Err(ActivityFetchError::Unavailable(
                        "Codex app-server returned an oversized message",
                    ));
                }
                AppServerFrame::InvalidUtf8 => {
                    return Err(ActivityFetchError::Unavailable(
                        "Codex app-server returned a non-text message",
                    ));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let message = serde_json::from_str::<Value>(&line).map_err(|_| {
                ActivityFetchError::Unavailable("Codex app-server returned an invalid message")
            })?;
            if is_server_request(&message) {
                self.write_message(&unsupported_server_request_response(&message))?;
                continue;
            }
            if message.get("id").and_then(Value::as_i64) == Some(expected_id) {
                return Ok(message);
            }
        }
    }
}

impl Drop for AppServerTransport {
    fn drop(&mut self) {
        self.frames.take();
        self.stdin.take();
        terminate_app_server_process_tree(&mut self.child);

        // Reader threads can outlive the launcher when its native child inherited
        // a pipe. Detach them instead of allowing transport cleanup to block the
        // usage worker indefinitely.
        drop(self.stdout_reader.take());
        drop(self.stderr_reader.take());
    }
}

fn configure_app_server_process(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
}

fn terminate_app_server_process_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(child.id()) {
        // The npm launcher cannot forward SIGKILL to the native Codex process.
        // Both processes inherit this dedicated group, so terminate the group.
        unsafe {
            libc::killpg(process_group, libc::SIGKILL);
        }
    }

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_stdout_reader(
    stdout: ChildStdout,
    sender: mpsc::SyncSender<AppServerFrame>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(Some(frame)) = read_jsonl_frame(&mut reader) {
            if sender.send(frame).is_err() {
                return;
            }
        }
    })
}

fn spawn_stderr_drain(mut stderr: ChildStderr) -> JoinHandle<()> {
    thread::spawn(move || {
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
    })
}

fn read_jsonl_frame<R: BufRead>(reader: &mut R) -> std::io::Result<Option<AppServerFrame>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame_from_bytes(bytes)))
            };
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let content_len = newline.unwrap_or(chunk.len());
        if bytes.len().saturating_add(content_len) > MAX_JSONL_LINE_BYTES {
            let consumed = newline.map_or(chunk.len(), |index| index + 1);
            reader.consume(consumed);
            if newline.is_none() {
                discard_until_newline(reader)?;
            }
            return Ok(Some(AppServerFrame::Oversized));
        }
        bytes.extend_from_slice(&chunk[..content_len]);
        let consumed = newline.map_or(chunk.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(frame_from_bytes(bytes)));
        }
    }
}

fn discard_until_newline<R: BufRead>(reader: &mut R) -> std::io::Result<()> {
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(index) = chunk.iter().position(|byte| *byte == b'\n') {
            reader.consume(index + 1);
            return Ok(());
        }
        let length = chunk.len();
        reader.consume(length);
    }
}

fn frame_from_bytes(bytes: Vec<u8>) -> AppServerFrame {
    String::from_utf8(bytes)
        .map(AppServerFrame::Json)
        .unwrap_or(AppServerFrame::InvalidUtf8)
}

fn is_server_request(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str).is_some()
        && message.get("id").is_some()
        && message.get("result").is_none()
        && message.get("error").is_none()
}

fn unsupported_server_request_response(message: &Value) -> Value {
    serde_json::json!({
        "id": message.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32601,
            "message": "Method not supported by tokscale"
        }
    })
}

fn initialize_request() -> Value {
    serde_json::json!({
        "id": INITIALIZE_REQUEST_ID,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "tokscale",
                "title": "Tokscale",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

fn initialized_notification() -> Value {
    serde_json::json!({"method": "initialized", "params": {}})
}

fn activity_request() -> Value {
    serde_json::json!({
        "id": ACTIVITY_REQUEST_ID,
        "method": "account/usage/read"
    })
}

fn classify_rpc_error(response: &Value) -> ActivityFetchError {
    let error = response.get("error");
    if error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
        == Some(-32601)
    {
        return ActivityFetchError::UnsupportedCli;
    }
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if message.contains("auth")
        || message.contains("sign in")
        || message.contains("login")
        || message.contains("not authenticated")
    {
        return ActivityFetchError::UnsupportedAuth;
    }
    ActivityFetchError::Unavailable("Codex app-server could not read account activity")
}

fn fetch_account_usage_from_app_server(
) -> std::result::Result<AppServerActivityResult, ActivityFetchError> {
    let mut transport = AppServerTransport::spawn()?;
    let deadline = Instant::now() + APP_SERVER_TIMEOUT;
    transport.write_message(&initialize_request())?;
    let initialize_response = transport.wait_for_response(INITIALIZE_REQUEST_ID, deadline)?;
    if initialize_response.get("error").is_some() {
        return Err(classify_rpc_error(&initialize_response));
    }
    if initialize_response.get("result").is_none() {
        return Err(ActivityFetchError::UnsupportedCli);
    }
    transport.write_message(&initialized_notification())?;
    transport.write_message(&activity_request())?;
    let activity_response = transport.wait_for_response(ACTIVITY_REQUEST_ID, deadline)?;
    if activity_response.get("error").is_some() {
        return Err(classify_rpc_error(&activity_response));
    }
    let result = activity_response
        .get("result")
        .ok_or(ActivityFetchError::Unavailable(
            "Codex app-server returned an invalid account activity response",
        ))?;
    parse_account_usage_result(result)
}

fn parse_account_usage_result(
    result: &Value,
) -> std::result::Result<AppServerActivityResult, ActivityFetchError> {
    let activity: AppServerActivityResult =
        serde_json::from_value(result.clone()).map_err(|_| {
            ActivityFetchError::Unavailable(
                "Codex app-server returned an invalid account activity response",
            )
        })?;
    for bucket in activity.daily_usage_buckets.iter().flatten() {
        NaiveDate::parse_from_str(&bucket.start_date, "%Y-%m-%d").map_err(|_| {
            ActivityFetchError::Unavailable(
                "Codex app-server returned an invalid daily activity date",
            )
        })?;
    }
    Ok(activity)
}

pub(super) fn refresh_current_account_activity() -> Result<usize> {
    let (auth, _) = read_current_credentials()?;
    let tokens = auth
        .tokens
        .as_ref()
        .context("Codex credentials contain no tokens")?;
    let account_id = load_credentials_store()
        .as_ref()
        .and_then(|store| matching_account_id_for_tokens(store, tokens))
        .unwrap_or_else(|| derive_account_id(tokens));
    let activity = fetch_account_usage_from_app_server()?;
    let fetched_at_ms = Utc::now().timestamp_millis();
    let store = TelemetryStore::open_default()?;
    let mut changed = 0usize;
    if let Some(summary) = activity.summary {
        changed = changed.saturating_add(store.upsert_account_usage_summary(
            &AccountUsageSummaryInput {
                provider: "Codex".to_string(),
                account_id: account_id.clone(),
                lifetime_tokens: summary.lifetime_tokens,
                peak_daily_tokens: summary.peak_daily_tokens,
                longest_running_turn_seconds: summary.longest_running_turn_sec,
                current_streak_days: summary.current_streak_days,
                longest_streak_days: summary.longest_streak_days,
                fetched_at_ms,
                source: APP_SERVER_SOURCE.to_string(),
            },
        )? as usize);
    }
    let inputs = activity
        .daily_usage_buckets
        .unwrap_or_default()
        .into_iter()
        .map(|bucket| AccountDailyUsageInput {
            provider: "Codex".to_string(),
            account_id: account_id.clone(),
            start_date: bucket.start_date,
            tokens: bucket.tokens,
            fetched_at_ms,
            source: APP_SERVER_SOURCE.to_string(),
        })
        .collect::<Vec<_>>();
    changed = changed.saturating_add(store.upsert_account_daily_usage(&inputs)?);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_summary_and_daily_usage_buckets() {
        let activity = parse_account_usage_result(&serde_json::json!({
            "summary": {
                "lifetimeTokens": 999,
                "peakDailyTokens": 400,
                "longestRunningTurnSec": 120,
                "currentStreakDays": 3,
                "longestStreakDays": 5
            },
            "dailyUsageBuckets": [
                {"startDate": "2026-07-22", "tokens": 20},
                {"startDate": "2026-07-23", "tokens": 130}
            ]
        }))
        .unwrap();

        let summary = activity.summary.unwrap();
        let buckets = activity.daily_usage_buckets.unwrap();
        assert_eq!(summary.lifetime_tokens, Some(999));
        assert_eq!(summary.longest_running_turn_sec, Some(120));
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[1].tokens, 130);
    }

    #[test]
    fn rejects_invalid_daily_usage_date() {
        assert!(parse_account_usage_result(&serde_json::json!({
            "dailyUsageBuckets": [{"startDate": "today", "tokens": 1}]
        }))
        .is_err());
    }

    #[test]
    fn protocol_messages_use_app_server_handshake() {
        assert_eq!(initialize_request()["method"], "initialize");
        assert_eq!(initialized_notification()["method"], "initialized");
        assert_eq!(activity_request()["method"], "account/usage/read");
    }

    #[test]
    fn rejects_oversized_jsonl_frames() {
        let mut input = vec![b'x'; MAX_JSONL_LINE_BYTES + 1];
        input.push(b'\n');
        let mut reader = BufReader::new(input.as_slice());
        assert!(matches!(
            read_jsonl_frame(&mut reader).unwrap(),
            Some(AppServerFrame::Oversized)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_closes_stderr_inherited_by_launcher_child() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 >/dev/null & echo $!"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let stderr_reader = spawn_stderr_drain(child.stderr.take().unwrap());
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut descendant_pid = String::new();
        stdout.read_line(&mut descendant_pid).unwrap();
        let descendant_pid = descendant_pid.trim().parse::<i32>().unwrap();
        child.wait().unwrap();

        terminate_app_server_process_tree(&mut child);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !stderr_reader.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let reader_finished = stderr_reader.is_finished();
        if !reader_finished {
            unsafe {
                libc::kill(descendant_pid, libc::SIGKILL);
            }
        }

        assert!(
            reader_finished,
            "process-tree cleanup left an inherited stderr pipe open"
        );
        stderr_reader.join().unwrap();
    }
}
