use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::env;
use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use uuid::Uuid;
use which::which;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, KeyPressEvent, KeyReleaseEvent, ModMask};
use x11rb::rust_connection::RustConnection;

pub const API_URL: &str = "https://open.bigmodel.cn/api/paas/v4/audio/transcriptions";
pub const MODEL: &str = "glm-asr-2512";
pub const HOTKEY_KEYCODE: u8 = 74;
pub const HOTKEY_NAME: &str = "F8";
pub const RECORDING_HISTORY_LIMIT: usize = 10;
pub const LOG_FILE_NAME: &str = "speak-it.log";
pub const FILLER_TOKENS: &[&str] = &["n", "r", "en", "er", "嗯", "呃", "额", "啊"];
pub const CLIPBOARD_PASTE_DELAY: Duration = Duration::from_millis(80);

static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

fn log_file_path() -> PathBuf {
    LOG_FILE_PATH
        .get_or_init(|| {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(LOG_FILE_NAME)
        })
        .clone()
}

fn append_log_line(level: &str, message: &str) {
    let path = log_file_path();
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            let _ = writeln!(file, "[{level}] {message}");
        }
        Err(error) => {
            eprintln!(
                "[WARN] failed to append log file {}: {error}",
                path.display()
            );
        }
    }
}

pub fn log_info(message: impl AsRef<str>) {
    let message = message.as_ref();
    println!("[INFO] {message}");
    append_log_line("INFO", message);
}

pub fn log_warn(message: impl AsRef<str>) {
    let message = message.as_ref();
    eprintln!("[WARN] {message}");
    append_log_line("WARN", message);
}

pub fn log_error(message: impl AsRef<str>) {
    let message = message.as_ref();
    eprintln!("[ERROR] {message}");
    append_log_line("ERROR", message);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderKind {
    Ffmpeg,
    Arecord,
}

impl Display for RecorderKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RecorderKind::Ffmpeg => write!(f, "ffmpeg"),
            RecorderKind::Arecord => write!(f, "arecord"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTool {
    Xclip,
    Xsel,
}

impl Display for ClipboardTool {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardTool::Xclip => write!(f, "xclip"),
            ClipboardTool::Xsel => write!(f, "xsel"),
        }
    }
}

pub struct RecorderProcess {
    child: Child,
    kind: RecorderKind,
    output_path: PathBuf,
    final_path: PathBuf,
    stdin: Option<ChildStdin>,
}

impl RecorderProcess {
    pub fn stop(mut self) -> Result<PathBuf> {
        log_info(format!(
            "stopping recorder process and finalizing {}",
            self.output_path.display()
        ));
        match self.kind {
            RecorderKind::Ffmpeg => {
                if let Some(mut stdin) = self.stdin.take() {
                    let _ = stdin.write_all(b"q\n");
                    let _ = stdin.flush();
                } else {
                    let _ = kill(Pid::from_raw(self.child.id() as i32), Signal::SIGINT);
                }
            }
            RecorderKind::Arecord => {
                let _ = kill(Pid::from_raw(self.child.id() as i32), Signal::SIGINT);
            }
        }
        let status = self
            .child
            .wait()
            .context("failed waiting recorder process")?;
        log_info(format!("recorder exited with status: {status}"));
        std::fs::rename(&self.output_path, &self.final_path).with_context(|| {
            format!(
                "failed to finalize recording {} -> {}",
                self.output_path.display(),
                self.final_path.display()
            )
        })?;
        Ok(self.final_path)
    }
}

#[derive(Debug, Clone)]
pub struct DependencyReport {
    pub api_key_present: bool,
    pub x11_display_present: bool,
    pub x11_session_detected: bool,
    pub xdotool_present: bool,
    pub clipboard_tool: Option<ClipboardTool>,
    pub recorder: Option<RecorderKind>,
}

impl DependencyReport {
    pub fn validate(&self, require_api_key: bool) -> Result<()> {
        if require_api_key && !self.api_key_present {
            bail!("missing ZHIPUAI_API_KEY");
        }
        if !self.x11_display_present {
            bail!("DISPLAY is not set");
        }
        if !self.x11_session_detected {
            bail!("X11 session not detected; Wayland is unsupported");
        }
        if !self.xdotool_present {
            bail!("missing xdotool");
        }
        if self.clipboard_tool.is_none() {
            bail!("missing clipboard tool; install xclip or xsel");
        }
        if self.recorder.is_none() {
            bail!("missing recorder; install ffmpeg or arecord");
        }
        Ok(())
    }
}

pub fn dependency_report() -> DependencyReport {
    let session_type = env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let recorder = if which("ffmpeg").is_ok() {
        Some(RecorderKind::Ffmpeg)
    } else if which("arecord").is_ok() {
        Some(RecorderKind::Arecord)
    } else {
        None
    };
    let clipboard_tool = if which("xclip").is_ok() {
        Some(ClipboardTool::Xclip)
    } else if which("xsel").is_ok() {
        Some(ClipboardTool::Xsel)
    } else {
        None
    };

    DependencyReport {
        api_key_present: env::var("ZHIPUAI_API_KEY")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        x11_display_present: env::var("DISPLAY")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        x11_session_detected: session_type.is_empty() || session_type.eq_ignore_ascii_case("x11"),
        xdotool_present: which("xdotool").is_ok(),
        clipboard_tool,
        recorder,
    }
}

pub fn doctor_output(report: &DependencyReport) -> (bool, Vec<String>) {
    let recorder = report
        .recorder
        .map(|kind| kind.to_string())
        .unwrap_or_else(|| "missing".to_string());
    let clipboard_tool = report
        .clipboard_tool
        .map(|tool| tool.to_string())
        .unwrap_or_else(|| "missing".to_string());
    let ok = report.api_key_present
        && report.x11_display_present
        && report.x11_session_detected
        && report.xdotool_present
        && report.clipboard_tool.is_some()
        && report.recorder.is_some();

    let mut lines = vec![
        format!(
            "ZHIPUAI_API_KEY: {}",
            status_text(report.api_key_present, "ok", "missing")
        ),
        format!(
            "DISPLAY: {}",
            status_text(report.x11_display_present, "ok", "missing")
        ),
        format!(
            "X11 session: {}",
            status_text(report.x11_session_detected, "ok", "not detected")
        ),
        format!(
            "xdotool: {}",
            status_text(report.xdotool_present, "ok", "missing")
        ),
        format!("clipboard tool: {clipboard_tool}"),
        format!("recorder: {recorder}"),
    ];

    if !report.api_key_present {
        lines.push("hint: export ZHIPUAI_API_KEY=...".to_string());
    }
    if !report.xdotool_present {
        lines.push("hint: install xdotool".to_string());
    }
    if report.clipboard_tool.is_none() {
        lines.push("hint: install xclip or xsel".to_string());
    }
    if report.recorder.is_none() {
        lines.push("hint: install ffmpeg or alsa-utils".to_string());
    }
    if !report.x11_display_present || !report.x11_session_detected {
        lines.push("hint: run under an X11 desktop session".to_string());
    }

    (ok, lines)
}

fn status_text<'a>(value: bool, ok: &'a str, fail: &'a str) -> &'a str {
    if value { ok } else { fail }
}

fn modifier_variants() -> [u16; 4] {
    [
        0,
        ModMask::LOCK.bits(),
        ModMask::M2.bits(),
        ModMask::LOCK.bits() | ModMask::M2.bits(),
    ]
}

pub fn spawn_recorder(kind: RecorderKind, output_path: &Path) -> Result<RecorderProcess> {
    let temp_output_path = staging_recording_path(output_path);
    log_info(format!(
        "starting recorder `{kind}` -> {}",
        temp_output_path.display()
    ));
    let mut command = match kind {
        RecorderKind::Ffmpeg => {
            let mut cmd = Command::new("ffmpeg");
            cmd.arg("-hide_banner")
                .arg("-loglevel")
                .arg("error")
                .arg("-y")
                .arg("-f")
                .arg("alsa")
                .arg("-i")
                .arg("default")
                .arg("-ac")
                .arg("1")
                .arg("-f")
                .arg("wav")
                .arg(&temp_output_path);
            cmd
        }
        RecorderKind::Arecord => {
            let mut cmd = Command::new("arecord");
            cmd.arg("-q")
                .arg("-c")
                .arg("1")
                .arg("-f")
                .arg("cd")
                .arg("-t")
                .arg("wav")
                .arg(&temp_output_path);
            cmd
        }
    };

    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start recorder {}", kind))?;
    let mut child = child;
    let stdin = child.stdin.take();

    Ok(RecorderProcess {
        child,
        kind,
        output_path: temp_output_path,
        final_path: output_path.to_path_buf(),
        stdin,
    })
}

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let clipboard_tool = if which("xclip").is_ok() {
        ClipboardTool::Xclip
    } else if which("xsel").is_ok() {
        ClipboardTool::Xsel
    } else {
        bail!("missing clipboard tool; install xclip or xsel");
    };

    log_info(format!(
        "injecting transcript through clipboard paste with {clipboard_tool}: {:?}",
        text
    ));
    set_clipboard_text(clipboard_tool, text)?;
    thread::sleep(CLIPBOARD_PASTE_DELAY);
    Command::new("xdotool")
        .arg("key")
        .arg("--clearmodifiers")
        .arg("ctrl+v")
        .status()
        .context("failed to execute xdotool")?
        .success()
        .then_some(())
        .ok_or_else(|| anyhow!("xdotool returned non-zero status"))
}

fn set_clipboard_text(tool: ClipboardTool, text: &str) -> Result<()> {
    let mut command = match tool {
        ClipboardTool::Xclip => {
            let mut command = Command::new("xclip");
            command.arg("-selection").arg("clipboard");
            command
        }
        ClipboardTool::Xsel => {
            let mut command = Command::new("xsel");
            command.arg("--clipboard").arg("--input");
            command
        }
    };

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start clipboard tool {tool}"))?;
    child
        .stdin
        .take()
        .context("failed to open clipboard tool stdin")?
        .write_all(text.as_bytes())
        .context("failed to write transcript to clipboard tool")?;
    child
        .wait()
        .with_context(|| format!("failed waiting clipboard tool {tool}"))?
        .success()
        .then_some(())
        .ok_or_else(|| anyhow!("clipboard tool {tool} returned non-zero status"))
}

pub async fn transcribe_file(audio_path: &Path) -> Result<String> {
    let api_key = env::var("ZHIPUAI_API_KEY").context("missing ZHIPUAI_API_KEY")?;
    let request_id = Uuid::new_v4().to_string();
    let audio_bytes = std::fs::read(audio_path)
        .with_context(|| format!("failed to read {}", audio_path.display()))?;
    log_info(format!(
        "uploading audio to BigModel: url={API_URL}, file={}, bytes={}, stream=false, model={MODEL}, request_id={request_id}",
        audio_path.display(),
        audio_bytes.len()
    ));
    let file_part = Part::bytes(audio_bytes)
        .mime_str("audio/wav")
        .context("failed to set audio mime type")?
        .file_name(
            audio_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("audio.wav")
                .to_string(),
        );

    let form = Form::new()
        .text("model", MODEL.to_string())
        .text("stream", "false")
        .text("request_id", request_id.clone())
        .part("file", file_part);

    let client = reqwest::Client::new();
    let response = client
        .post(API_URL)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .context("failed to call BigModel transcription API")?;
    let status = response.status();
    let headers = format!("{:?}", response.headers());
    log_info(format!(
        "BigModel response status={status}, headers={headers}"
    ));
    if !status.is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read error body>".to_string());
        log_error(format!("BigModel error body: {error_body}"));
        bail!("BigModel transcription API returned an error: HTTP status {status}");
    }
    log_info("BigModel accepted the request");

    let payload: NonStreamResponse = response.json().await.context("invalid JSON response")?;
    let raw_text = payload.text;
    let cleaned_text = normalize_transcript_text(&raw_text);
    log_info(format!("transcription completed raw: {:?}", raw_text));
    log_info(format!("transcription completed cleaned: {:?}", cleaned_text));
    Ok(cleaned_text)
}

#[derive(Debug, Deserialize)]
struct NonStreamResponse {
    text: String,
}

fn recording_output_path() -> Result<PathBuf> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    Ok(cwd.join(format!(
        "speak-it-{}.wav",
        Local::now().format("%Y%m%d-%H%M%S-%3f")
    )))
}

fn staging_recording_path(final_path: &Path) -> PathBuf {
    let parent = final_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("speak-it.wav");
    parent.join(format!(".{name}.part"))
}

fn transcript_output_path(audio_path: &Path) -> PathBuf {
    audio_path.with_extension("txt")
}

fn write_transcript_sidecar(audio_path: &Path, text: &str) -> Result<PathBuf> {
    let output_path = transcript_output_path(audio_path);
    std::fs::write(&output_path, text)
        .with_context(|| format!("failed to write transcript file {}", output_path.display()))?;
    Ok(output_path)
}

fn normalize_transcript_text(text: &str) -> String {
    text.split_whitespace()
        .filter(|token| !is_filler_token(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_filler_token(token: &str) -> bool {
    let trimmed = token.trim_matches(|c: char| {
        c.is_ascii_punctuation() || matches!(c, '，' | '。' | '！' | '？' | '；' | '：')
    });
    if trimmed.is_empty() {
        return false;
    }
    FILLER_TOKENS
        .iter()
        .any(|filler| trimmed.eq_ignore_ascii_case(filler) || trimmed == *filler)
}

fn prune_recordings(dir: &Path, keep: usize) -> Result<()> {
    let mut recordings = Vec::new();

    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read recording directory {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_file() || !file_name.starts_with("speak-it-") || !file_name.ends_with(".wav") {
            continue;
        }

        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", path.display()))?;
        let modified = metadata
            .modified()
            .with_context(|| format!("failed to read modified time for {}", path.display()))?;
        recordings.push((modified, path));
    }

    if recordings.len() <= keep {
        return Ok(());
    }

    recordings.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in recordings.into_iter().skip(keep) {
        log_info(format!("removing old recording {}", path.display()));
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove old recording {}", path.display()))?;
        let transcript_path = transcript_output_path(&path);
        if transcript_path.is_file() {
            log_info(format!(
                "removing old transcript sidecar {}",
                transcript_path.display()
            ));
            std::fs::remove_file(&transcript_path).with_context(|| {
                format!(
                    "failed to remove old transcript sidecar {}",
                    transcript_path.display()
                )
            })?;
        }
    }

    Ok(())
}

pub struct Daemon {
    conn: RustConnection,
    screen_num: usize,
    recorder_kind: RecorderKind,
}

impl Daemon {
    pub fn connect(recorder_kind: RecorderKind) -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None).context("failed to connect to X11")?;
        Ok(Self {
            conn,
            screen_num,
            recorder_kind,
        })
    }

    pub async fn run(&self) -> Result<()> {
        let setup = self.conn.setup();
        let root = setup.roots[self.screen_num].root;
        log_info(format!(
            "connected to X11, screen={}, root_window={root}, hotkey={} (keycode={HOTKEY_KEYCODE})",
            self.screen_num, HOTKEY_NAME,
        ));

        // Grab the chosen hotkey with common lock modifier combinations so Caps Lock / Num Lock do not break it.
        for modifiers in modifier_variants() {
            self.conn
                .grab_key(
                    false,
                    root,
                    modifiers.into(),
                    HOTKEY_KEYCODE,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .context("failed to grab hotkey")?;
            log_info(format!("grabbed hotkey for modifier mask {modifiers}"));
        }
        self.conn.flush().context("failed to flush X11 commands")?;
        println!("================ speak-it ================");
        println!("[READY] 全局监听已启动");
        println!("[READY] 按下 {HOTKEY_NAME} 立即开始录音，松开立即结束并转写");
        println!("[READY] 终端会持续打印状态日志");
        println!("==========================================");

        let mut recorder: Option<RecorderProcess> = None;
        let mut pending_event: Option<Event> = None;

        loop {
            let event = match pending_event.take() {
                Some(event) => Some(event),
                None => self
                    .conn
                    .poll_for_event()
                    .context("failed to poll X11 event")?,
            };
            match event {
                Some(event) => match event {
                    x11rb::protocol::Event::KeyPress(KeyPressEvent { detail, .. })
                        if detail == HOTKEY_KEYCODE =>
                    {
                        if recorder.is_none() {
                            log_info(format!(
                                "{HOTKEY_NAME} pressed; starting recording immediately"
                            ));
                            let output_path = recording_output_path()?;
                            log_info(format!(
                                "recording will be saved to {}",
                                output_path.display()
                            ));
                            let recorder_process = spawn_recorder(self.recorder_kind, &output_path)
                                .context("recording start failed")?;
                            println!("[STATE] recording...");
                            recorder = Some(recorder_process);
                        } else {
                            log_warn(format!(
                                "received duplicate {HOTKEY_NAME} press while recording is already active"
                            ));
                        }
                    }
                    x11rb::protocol::Event::KeyRelease(KeyReleaseEvent { detail, time, .. })
                        if detail == HOTKEY_KEYCODE =>
                    {
                        if self.consume_autorepeat_press(detail, time, &mut pending_event)? {
                            continue;
                        }
                        if let Some(active) = recorder.take() {
                            log_info(format!(
                                "{HOTKEY_NAME} released; stopping recording and starting transcription"
                            ));
                            let audio_path = active.stop()?;
                            self.discard_pending_hotkey_events()?;
                            if let Ok(metadata) = std::fs::metadata(&audio_path) {
                                log_info(format!(
                                    "recorded audio file ready: path={}, bytes={}",
                                    audio_path.display(),
                                    metadata.len()
                                ));
                            }
                            if let Some(dir) = audio_path.parent() {
                                prune_recordings(dir, RECORDING_HISTORY_LIMIT)?;
                            }
                            println!("[STATE] transcribing...");
                            let result = transcribe_file(&audio_path).await;
                            match result {
                                Ok(text) => {
                                    let transcript_path = write_transcript_sidecar(&audio_path, &text)?;
                                    log_info(format!(
                                        "transcript sidecar written to {}",
                                        transcript_path.display()
                                    ));
                                    inject_text(&text)?;
                                    log_info(format!("final transcript committed: {:?}", text));
                                }
                                Err(error) => {
                                    log_error(format!("transcription failed: {error:#}"));
                                }
                            }
                        } else {
                            log_warn(format!(
                                "received {HOTKEY_NAME} release while no recording session is active"
                            ));
                        }
                    }
                    _ => {}
                },
                None => thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    fn consume_autorepeat_press(
        &self,
        detail: u8,
        release_time: u32,
        pending_event: &mut Option<Event>,
    ) -> Result<bool> {
        let Some(next_event) = self
            .conn
            .poll_for_event()
            .context("failed to poll X11 autorepeat follow-up event")?
        else {
            return Ok(false);
        };

        match next_event {
            Event::KeyPress(KeyPressEvent {
                detail: next_detail,
                time: next_time,
                ..
            }) if next_detail == detail && next_time == release_time => {
                log_info(format!(
                    "ignoring X11 autorepeat {HOTKEY_NAME} release/press pair at time={release_time}"
                ));
                Ok(true)
            }
            other => {
                *pending_event = Some(other);
                Ok(false)
            }
        }
    }

    fn discard_pending_hotkey_events(&self) -> Result<()> {
        let mut discarded = 0usize;

        while let Some(event) = self
            .conn
            .poll_for_event()
            .context("failed to poll queued X11 event")?
        {
            match event {
                x11rb::protocol::Event::KeyPress(KeyPressEvent { detail, .. })
                | x11rb::protocol::Event::KeyRelease(KeyReleaseEvent { detail, .. })
                    if detail == HOTKEY_KEYCODE =>
                {
                    discarded += 1;
                }
                _ => {}
            }
        }

        if discarded > 0 {
            log_info(format!(
                "discarded {discarded} queued {HOTKEY_NAME} autorepeat event(s)"
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration as StdDuration, SystemTime};

    #[test]
    fn doctor_output_includes_expected_hints() {
        let report = DependencyReport {
            api_key_present: false,
            x11_display_present: false,
            x11_session_detected: false,
            xdotool_present: false,
            clipboard_tool: None,
            recorder: None,
        };
        let (ok, lines) = doctor_output(&report);
        assert!(!ok);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("export ZHIPUAI_API_KEY"))
        );
        assert!(lines.iter().any(|line| line.contains("xdotool")));
        assert!(lines.iter().any(|line| line.contains("xclip or xsel")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("ffmpeg or alsa-utils"))
        );
    }

    #[test]
    fn prune_recordings_keeps_latest_ten() {
        let test_dir = std::env::temp_dir().join(format!("speak-it-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&test_dir).expect("temp test dir should be created");

        for index in 0..12 {
            let path = test_dir.join(format!("speak-it-{index:02}.wav"));
            std::fs::write(&path, format!("sample-{index}"))
                .expect("recording file should be written");
            let modified = SystemTime::now() - StdDuration::from_secs((12 - index) as u64);
            filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(modified))
                .expect("mtime should be adjusted");
        }

        let ignored = test_dir.join("notes.txt");
        std::fs::write(&ignored, "ignore me").expect("non recording file should be written");

        prune_recordings(&test_dir, 10).expect("prune should succeed");

        let mut kept = std::fs::read_dir(&test_dir)
            .expect("dir should still exist")
            .map(|entry| entry.expect("entry should be readable").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wav"))
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("file name should be utf8")
                    .to_string()
            })
            .collect::<Vec<_>>();
        kept.sort();

        assert_eq!(kept.len(), 10);
        assert!(!kept.iter().any(|name| name == "speak-it-00.wav"));
        assert!(!kept.iter().any(|name| name == "speak-it-01.wav"));
        assert!(ignored.exists());

        std::fs::remove_dir_all(&test_dir).expect("temp test dir should be removed");
    }

    #[test]
    fn normalize_transcript_text_removes_filler_tokens() {
        assert_eq!(normalize_transcript_text("n 我 想 说"), "我 想 说");
        assert_eq!(normalize_transcript_text("嗯 这个 r 可以"), "这个 可以");
        assert_eq!(normalize_transcript_text("normal text"), "normal text");
        assert_eq!(normalize_transcript_text("r,"), "");
    }
}
