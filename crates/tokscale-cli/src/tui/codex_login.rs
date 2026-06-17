use anyhow::Result;

#[derive(Debug, Clone)]
pub(crate) enum CodexLoginOutcome {
    Imported(crate::commands::usage::codex::CodexAccountInfo),
    Failed(String),
}

#[derive(Debug)]
pub(crate) enum CodexLoginEvent {
    Output(String),
    Finished(CodexLoginOutcome),
}

pub(crate) fn run_codex_login_worker(tx: std::sync::mpsc::Sender<CodexLoginEvent>) {
    let result = run_codex_login_worker_inner(tx.clone());
    let outcome = match result {
        Ok(info) => CodexLoginOutcome::Imported(info),
        Err(e) => CodexLoginOutcome::Failed(e.to_string()),
    };
    let _ = tx.send(CodexLoginEvent::Finished(outcome));
}

fn run_codex_login_worker_inner(
    tx: std::sync::mpsc::Sender<CodexLoginEvent>,
) -> Result<crate::commands::usage::codex::CodexAccountInfo> {
    let codex_home =
        std::env::temp_dir().join(format!("tokscale-codex-login-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&codex_home)
        .map_err(|e| anyhow::anyhow!("failed to create temporary Codex home: {e}"))?;

    let result = run_codex_login_in_home(&codex_home, tx);
    let _ = std::fs::remove_dir_all(&codex_home);
    result
}

fn run_codex_login_in_home(
    codex_home: &std::path::Path,
    tx: std::sync::mpsc::Sender<CodexLoginEvent>,
) -> Result<crate::commands::usage::codex::CodexAccountInfo> {
    let _ = tx.send(CodexLoginEvent::Output(
        "Starting Codex browser login".to_string(),
    ));

    let mut child = std::process::Command::new("codex")
        .arg("login")
        .env("CODEX_HOME", codex_home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to start codex login: {e}"))?;

    let output_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_codex_login_output_reader(
            stdout,
            tx.clone(),
            std::sync::Arc::clone(&output_lines),
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_codex_login_output_reader(
            stderr,
            tx.clone(),
            std::sync::Arc::clone(&output_lines),
        ));
    }

    let status = child
        .wait()
        .map_err(|e| anyhow::anyhow!("failed to wait for codex login: {e}"))?;
    for reader in readers {
        let _ = reader.join();
    }

    if !status.success() {
        let output_lines = output_lines
            .lock()
            .map(|lines| lines.clone())
            .unwrap_or_default();
        anyhow::bail!("{}", codex_login_failure_message(&status, &output_lines));
    }

    let auth_path = codex_home.join("auth.json");
    let _ = crate::commands::usage::codex::save_current_account_as_active(None);
    crate::commands::usage::codex::import_auth_file_without_activating(&auth_path, None)
}

fn spawn_codex_login_output_reader<R>(
    reader: R,
    tx: std::sync::mpsc::Sender<CodexLoginEvent>,
    output_lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> std::thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(reader);
        for line in std::io::BufRead::lines(reader).map_while(std::result::Result::ok) {
            let line = sanitize_codex_login_line(&line);
            if !line.trim().is_empty() {
                if let Ok(mut output_lines) = output_lines.lock() {
                    output_lines.push(line.clone());
                }
                let _ = tx.send(CodexLoginEvent::Output(line));
            }
        }
    })
}

fn codex_login_failure_message(
    status: &std::process::ExitStatus,
    output_lines: &[String],
) -> String {
    codex_login_failure_message_from_output(&status.to_string(), output_lines)
}

fn codex_login_failure_message_from_output(status: &str, output_lines: &[String]) -> String {
    let output = output_lines.join("\n").to_lowercase();

    if output.contains("429") || output.contains("too many requests") {
        return "OpenAI login is rate-limited (429 Too Many Requests). Wait before trying Add Codex again.".to_string();
    }

    if output.contains("expired") {
        return "Codex device code expired. Start Add Codex again to get a new code.".to_string();
    }

    if output.contains("device auth failed") {
        return "Codex device login failed. Try Add Codex again later.".to_string();
    }

    format!("codex login exited with {status}")
}

fn sanitize_codex_login_line(line: &str) -> String {
    let mut sanitized = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' && chars.peek() == Some(&'\\') {
                            let _ = chars.next();
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
            continue;
        }

        if !ch.is_control() || ch == '\t' {
            sanitized.push(ch);
        }
    }

    sanitized
}
