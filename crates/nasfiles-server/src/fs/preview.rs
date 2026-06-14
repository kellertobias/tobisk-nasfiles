use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
};

const STATUS_TTL_MS: i64 = 30 * 60 * 1000;
const STDERR_TAIL_LIMIT: usize = 4096;
const HLS_PLAYLIST_NAME: &str = "preview.m3u8";
const HLS_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const HLS_POLL_INTERVAL: Duration = Duration::from_millis(150);
const HLS_SEGMENT_SECONDS: &str = "4";

#[derive(Clone)]
pub struct MediaPreviewService {
    limiter: Arc<Semaphore>,
    status: PreviewStatusStore,
    hls: HlsSessionStore,
}

impl MediaPreviewService {
    pub fn new(max_concurrent_transcodes: usize) -> Self {
        Self {
            limiter: Arc::new(Semaphore::new(max_concurrent_transcodes.max(1))),
            status: PreviewStatusStore::new(),
            hls: HlsSessionStore::new(),
        }
    }

    pub fn status(&self, session: &str, path: &Path) -> Option<PreviewStatus> {
        let session = sanitize_session(Some(session))?;
        self.status.get(&session, &source_key(path))
    }

    /// Stream a browser-friendly media preview produced by ffmpeg.
    ///
    /// Video previews are capped to 480p without upscaling and audio previews
    /// are transcoded to 128k AAC. The output is fragmented MP4 so playback can
    /// begin while ffmpeg is still producing bytes.
    pub async fn serve_media_preview(
        &self,
        path: &Path,
        session: Option<&str>,
        segment: Option<&str>,
        segment_url_prefix: Option<&str>,
        server_side_execution_enabled: bool,
    ) -> Result<Response, PreviewError> {
        if !server_side_execution_enabled {
            return Err(PreviewError::ServerSideExecutionDisabled);
        }

        if !path.is_file() {
            return Err(PreviewError::NotFound);
        }

        let kind = preview_kind(path).ok_or(PreviewError::Unsupported)?;
        let session = sanitize_session(session).unwrap_or_else(fallback_session);
        let source_key = source_key(path);

        if kind == PreviewKind::Video {
            if let Some(segment) = segment {
                return self.serve_hls_segment(&session, &source_key, segment).await;
            }

            return self
                .serve_hls_playlist(
                    path,
                    &session,
                    &source_key,
                    segment_url_prefix.ok_or_else(|| {
                        PreviewError::Process("missing HLS segment URL prefix".to_string())
                    })?,
                )
                .await;
        }

        let profile = kind.profile().to_string();
        let content_type = kind.content_type();
        let started_at = Instant::now();

        self.status.start(&session, &source_key, &profile);

        let permit =
            self.limiter.clone().acquire_owned().await.map_err(|_| {
                PreviewError::Process("media preview limiter was closed".to_string())
            })?;
        self.status.mark_starting(&session);

        let mut command = build_ffmpeg_command(path, kind);
        let mut child = command.spawn().map_err(|e| {
            let error = if e.kind() == std::io::ErrorKind::NotFound {
                PreviewError::FfmpegUnavailable
            } else {
                PreviewError::Io(e)
            };
            self.status
                .mark_failed(&session, started_at, None, None, Some(error.to_string()));
            error
        })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| PreviewError::Process("failed to capture ffmpeg stdout".to_string()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| PreviewError::Process("failed to capture ffmpeg stderr".to_string()))?;
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let status = self.status.clone();
        let session_for_task = session.clone();
        let path_display = path.display().to_string();
        let stderr_redacted_path = path_display.clone();

        status.mark_streaming(&session_for_task);
        tokio::spawn(async move {
            let stderr_task = tokio::spawn(async move {
                let mut output = String::new();
                let _ = stderr.read_to_string(&mut output).await;
                trim_stderr_tail(&output, Some(&stderr_redacted_path))
            });

            let mut bytes_sent = 0_u64;
            let mut buf = vec![0_u8; 32 * 1024];
            let mut copy_error: Option<String> = None;

            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = writer.write_all(&buf[..n]).await {
                            copy_error = Some(e.to_string());
                            let _ = child.kill().await;
                            break;
                        }
                        bytes_sent += n as u64;
                        status.update_bytes(&session_for_task, bytes_sent, started_at);
                    }
                    Err(e) => {
                        copy_error = Some(e.to_string());
                        let _ = child.kill().await;
                        break;
                    }
                }
            }
            let _ = writer.shutdown().await;
            drop(writer);

            let wait_result = child.wait().await;
            let stderr_tail = stderr_task.await.unwrap_or_default();
            drop(permit);

            match wait_result {
                Ok(exit_status) if exit_status.success() && copy_error.is_none() => {
                    status.mark_completed(
                        &session_for_task,
                        started_at,
                        bytes_sent,
                        Some(exit_status_to_string(exit_status)),
                        stderr_tail,
                    );
                }
                Ok(exit_status) => {
                    let exit = exit_status_to_string(exit_status);
                    let error = copy_error.unwrap_or_else(|| format!("ffmpeg exited with {exit}"));
                    tracing::warn!(
                        "ffmpeg preview failed for {}: {}{}",
                        path_display,
                        error,
                        if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {stderr_tail}")
                        }
                    );
                    status.mark_failed(
                        &session_for_task,
                        started_at,
                        Some(bytes_sent),
                        Some(exit),
                        Some(error),
                    );
                    if !stderr_tail.is_empty() {
                        status.update_stderr(&session_for_task, stderr_tail);
                    }
                }
                Err(e) => {
                    tracing::debug!("failed to wait for ffmpeg preview process: {e}");
                    status.mark_failed(
                        &session_for_task,
                        started_at,
                        Some(bytes_sent),
                        None,
                        Some(e.to_string()),
                    );
                    if !stderr_tail.is_empty() {
                        status.update_stderr(&session_for_task, stderr_tail);
                    }
                }
            }
        });

        let stream = tokio_util::io::ReaderStream::new(reader);
        let body = Body::from_stream(stream);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::CONTENT_ENCODING, "identity")
            .header(header::CONTENT_DISPOSITION, "inline")
            .header("X-NasFiles-Preview-Session", session)
            .header("X-Content-Type-Options", "nosniff")
            .body(body)
            .map_err(|e| PreviewError::Response(e.to_string()))
    }

    async fn serve_hls_playlist(
        &self,
        path: &Path,
        session: &str,
        source_key: &str,
        segment_url_prefix: &str,
    ) -> Result<Response, PreviewError> {
        self.hls.prune_expired();
        let hls_session = match self.hls.get(session, source_key) {
            Some(existing) => existing,
            None => self.start_hls_session(path, session, source_key)?,
        };

        let playlist = wait_for_hls_playlist(&hls_session.playlist_path).await?;
        self.status.mark_streaming(session);
        let playlist = rewrite_hls_playlist(&playlist, segment_url_prefix);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/x-mpegurl")
            .header(header::CACHE_CONTROL, "no-store")
            .header("X-NasFiles-Preview-Session", session)
            .header("X-Content-Type-Options", "nosniff")
            .body(Body::from(playlist))
            .map_err(|e| PreviewError::Response(e.to_string()))
    }

    async fn serve_hls_segment(
        &self,
        session: &str,
        source_key: &str,
        segment: &str,
    ) -> Result<Response, PreviewError> {
        let hls_session = self
            .hls
            .get(session, source_key)
            .ok_or(PreviewError::NotFound)?;
        let segment = sanitize_hls_segment(segment).ok_or(PreviewError::NotFound)?;
        let segment_path = hls_session.dir.path().join(segment);
        let bytes = tokio::fs::read(segment_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => PreviewError::NotFound,
                _ => PreviewError::Io(e),
            })?;

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "video/mp2t")
            .header(header::CACHE_CONTROL, "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .body(Body::from(bytes))
            .map_err(|e| PreviewError::Response(e.to_string()))
    }

    fn start_hls_session(
        &self,
        path: &Path,
        session: &str,
        source_key: &str,
    ) -> Result<HlsPreviewSession, PreviewError> {
        let temp_dir = tempfile::Builder::new()
            .prefix("nasfiles-media-preview-")
            .tempdir()
            .map_err(PreviewError::Io)?;
        let dir = Arc::new(temp_dir);
        let playlist_path = dir.path().join(HLS_PLAYLIST_NAME);
        let segment_pattern = dir.path().join("segment_%05d.ts");
        let hls_session = HlsPreviewSession {
            source_key: source_key.to_string(),
            dir: dir.clone(),
            playlist_path: playlist_path.clone(),
            created_at: now_ms(),
        };

        self.hls.insert(session, hls_session.clone());
        self.status
            .start(session, source_key, PreviewKind::Video.profile());

        let limiter = self.limiter.clone();
        let status = self.status.clone();
        let session_for_task = session.to_string();
        let source = path.to_path_buf();
        let path_display = path.display().to_string();
        let started_at = Instant::now();

        tokio::spawn(async move {
            let permit = match limiter.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    status.mark_failed(
                        &session_for_task,
                        started_at,
                        Some(0),
                        None,
                        Some("media preview limiter was closed".to_string()),
                    );
                    return;
                }
            };

            status.mark_starting(&session_for_task);
            let mut command = build_hls_ffmpeg_command(&source, &playlist_path, &segment_pattern);
            let output = command.output().await;
            drop(permit);

            match output {
                Ok(output) if output.status.success() => {
                    status.mark_completed(
                        &session_for_task,
                        started_at,
                        hls_output_bytes(dir.path()),
                        Some(exit_status_to_string(output.status)),
                        trim_stderr_tail(
                            &String::from_utf8_lossy(&output.stderr),
                            Some(&path_display),
                        ),
                    );
                }
                Ok(output) => {
                    let stderr_tail = trim_stderr_tail(
                        &String::from_utf8_lossy(&output.stderr),
                        Some(&path_display),
                    );
                    let exit = exit_status_to_string(output.status);
                    tracing::warn!(
                        "ffmpeg HLS preview failed for {}: {}{}",
                        path_display,
                        exit,
                        if stderr_tail.is_empty() {
                            String::new()
                        } else {
                            format!(": {stderr_tail}")
                        }
                    );
                    status.mark_failed(
                        &session_for_task,
                        started_at,
                        Some(hls_output_bytes(dir.path())),
                        Some(exit),
                        Some("ffmpeg HLS transcode failed".to_string()),
                    );
                    if !stderr_tail.is_empty() {
                        status.update_stderr(&session_for_task, stderr_tail);
                    }
                }
                Err(e) => {
                    let error = if e.kind() == std::io::ErrorKind::NotFound {
                        PreviewError::FfmpegUnavailable.to_string()
                    } else {
                        e.to_string()
                    };
                    status.mark_failed(
                        &session_for_task,
                        started_at,
                        Some(hls_output_bytes(dir.path())),
                        None,
                        Some(error),
                    );
                }
            }
        });

        Ok(hls_session)
    }
}

#[derive(Clone)]
struct HlsSessionStore {
    entries: Arc<DashMap<String, HlsPreviewSession>>,
}

impl HlsSessionStore {
    fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    fn get(&self, session: &str, source_key: &str) -> Option<HlsPreviewSession> {
        self.prune_expired();
        self.entries
            .get(session)
            .filter(|entry| entry.source_key == source_key)
            .map(|entry| entry.value().clone())
    }

    fn insert(&self, session: &str, hls_session: HlsPreviewSession) {
        self.prune_expired();
        self.entries.insert(session.to_string(), hls_session);
    }

    fn prune_expired(&self) {
        let cutoff = now_ms() - STATUS_TTL_MS;
        self.entries.retain(|_, entry| entry.created_at >= cutoff);
    }
}

#[derive(Clone)]
struct HlsPreviewSession {
    source_key: String,
    dir: Arc<tempfile::TempDir>,
    playlist_path: PathBuf,
    created_at: i64,
}

#[derive(Clone)]
pub struct PreviewStatusStore {
    entries: Arc<DashMap<String, PreviewStatus>>,
}

impl PreviewStatusStore {
    fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    fn get(&self, session: &str, source_key: &str) -> Option<PreviewStatus> {
        self.prune_expired();
        self.entries
            .get(session)
            .filter(|entry| entry.source_key == source_key)
            .map(|entry| entry.value().clone())
    }

    fn start(&self, session: &str, source_key: &str, profile: &str) {
        self.prune_expired();
        let now = now_ms();
        self.entries.insert(
            session.to_string(),
            PreviewStatus {
                session: session.to_string(),
                source_key: source_key.to_string(),
                state: "queued".to_string(),
                profile: profile.to_string(),
                mode: "transcoded".to_string(),
                bytes_sent: 0,
                elapsed_ms: 0,
                exit_status: None,
                stderr_tail: None,
                error: None,
                created_at: now,
                updated_at: now,
            },
        );
    }

    fn mark_starting(&self, session: &str) {
        self.update(session, |entry| {
            entry.state = "starting".to_string();
            entry.error = None;
        });
    }

    fn mark_streaming(&self, session: &str) {
        self.update(session, |entry| {
            entry.state = "streaming".to_string();
            entry.error = None;
        });
    }

    fn update_bytes(&self, session: &str, bytes_sent: u64, started_at: Instant) {
        self.update(session, |entry| {
            entry.bytes_sent = bytes_sent;
            entry.elapsed_ms = started_at.elapsed().as_millis() as u64;
        });
    }

    fn update_stderr(&self, session: &str, stderr_tail: String) {
        self.update(session, |entry| {
            entry.stderr_tail = Some(stderr_tail);
        });
    }

    fn mark_completed(
        &self,
        session: &str,
        started_at: Instant,
        bytes_sent: u64,
        exit_status: Option<String>,
        stderr_tail: String,
    ) {
        self.update(session, |entry| {
            entry.state = "completed".to_string();
            entry.bytes_sent = bytes_sent;
            entry.elapsed_ms = started_at.elapsed().as_millis() as u64;
            entry.exit_status = exit_status;
            entry.stderr_tail = (!stderr_tail.is_empty()).then_some(stderr_tail);
            entry.error = None;
        });
    }

    fn mark_failed(
        &self,
        session: &str,
        started_at: Instant,
        bytes_sent: Option<u64>,
        exit_status: Option<String>,
        error: Option<String>,
    ) {
        self.update(session, |entry| {
            entry.state = "failed".to_string();
            if let Some(bytes_sent) = bytes_sent {
                entry.bytes_sent = bytes_sent;
            }
            entry.elapsed_ms = started_at.elapsed().as_millis() as u64;
            entry.exit_status = exit_status;
            entry.error = error.map(|error| sanitize_status_text(error, None));
        });
    }

    fn update(&self, session: &str, f: impl FnOnce(&mut PreviewStatus)) {
        if let Some(mut entry) = self.entries.get_mut(session) {
            f(&mut entry);
            entry.updated_at = now_ms();
        }
    }

    fn prune_expired(&self) {
        let cutoff = now_ms() - STATUS_TTL_MS;
        self.entries
            .retain(|_, entry| entry.updated_at >= cutoff || entry.state == "streaming");
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewStatus {
    pub session: String,
    #[serde(skip_serializing)]
    pub source_key: String,
    pub state: String,
    pub profile: String,
    pub mode: String,
    pub bytes_sent: u64,
    pub elapsed_ms: u64,
    pub exit_status: Option<String>,
    pub stderr_tail: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewKind {
    Video,
    Audio,
}

impl PreviewKind {
    fn content_type(self) -> &'static str {
        match self {
            PreviewKind::Video => "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\"",
            PreviewKind::Audio => "audio/mp4; codecs=\"mp4a.40.2\"",
        }
    }

    fn profile(self) -> &'static str {
        match self {
            PreviewKind::Video => "video_480p_h264_aac_128k_hls",
            PreviewKind::Audio => "audio_aac_128k_frag_mp4",
        }
    }
}

fn build_ffmpeg_command(path: &Path, kind: PreviewKind) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-fflags")
        .arg("+genpts")
        .arg("-i")
        .arg(path)
        .arg("-map_metadata")
        .arg("-1");

    for arg in transcode_args(kind) {
        command.arg(arg);
    }

    command
        .arg("-movflags")
        .arg("frag_keyframe+empty_moov+default_base_moof")
        .arg("-avoid_negative_ts")
        .arg("make_zero")
        .arg("-f")
        .arg("mp4")
        .arg("pipe:1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    command
}

fn build_hls_ffmpeg_command(path: &Path, playlist_path: &Path, segment_pattern: &Path) -> Command {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-fflags")
        .arg("+genpts")
        .arg("-i")
        .arg(path)
        .arg("-map_metadata")
        .arg("-1");

    for arg in transcode_args(PreviewKind::Video) {
        command.arg(arg);
    }

    command
        .arg("-sc_threshold")
        .arg("0")
        .arg("-force_key_frames")
        .arg(format!("expr:gte(t,n_forced*{HLS_SEGMENT_SECONDS})"))
        .arg("-hls_time")
        .arg(HLS_SEGMENT_SECONDS)
        .arg("-hls_list_size")
        .arg("0")
        .arg("-hls_playlist_type")
        .arg("event")
        .arg("-hls_flags")
        .arg("independent_segments+temp_file")
        .arg("-hls_segment_filename")
        .arg(segment_pattern)
        .arg("-f")
        .arg("hls")
        .arg(playlist_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    command
}

fn transcode_args(kind: PreviewKind) -> Vec<&'static str> {
    match kind {
        PreviewKind::Video => vec![
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-sn",
            "-dn",
            "-vf",
            "scale=w='if(gt(a,854/480),min(854,iw),-2)':h='if(gt(a,854/480),-2,min(480,ih))'",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-profile:v",
            "baseline",
            "-level:v",
            "3.0",
            "-tune",
            "zerolatency",
            "-crf",
            "32",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ac",
            "2",
        ],
        PreviewKind::Audio => vec![
            "-map", "0:a:0", "-sn", "-dn", "-vn", "-c:a", "aac", "-b:a", "128k", "-ac", "2",
        ],
    }
}

fn preview_kind(path: &Path) -> Option<PreviewKind> {
    let mime = mime_guess::from_path(path).first()?;
    let essence = mime.essence_str();

    if essence.starts_with("video/") {
        Some(PreviewKind::Video)
    } else if essence.starts_with("audio/") {
        Some(PreviewKind::Audio)
    } else {
        None
    }
}

fn sanitize_session(session: Option<&str>) -> Option<String> {
    let session = session?.trim();
    if session.is_empty() || session.len() > 96 {
        return None;
    }

    session
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        .then(|| session.to_string())
}

fn sanitize_hls_segment(segment: &str) -> Option<&str> {
    if segment.len() > 64 {
        return None;
    }

    let stem = segment.strip_prefix("segment_")?.strip_suffix(".ts")?;
    if stem.len() != 5 || !stem.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(segment)
}

async fn wait_for_hls_playlist(path: &Path) -> Result<String, PreviewError> {
    let started = Instant::now();
    loop {
        match tokio::fs::read_to_string(path).await {
            Ok(playlist) if playlist.contains("#EXTINF") => return Ok(playlist),
            Ok(_) | Err(_) if started.elapsed() < HLS_STARTUP_TIMEOUT => {
                tokio::time::sleep(HLS_POLL_INTERVAL).await;
            }
            Ok(_) => {
                return Err(PreviewError::Process(
                    "HLS playlist did not contain any media segments".to_string(),
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PreviewError::Process(
                    "timed out waiting for HLS playlist".to_string(),
                ));
            }
            Err(e) => return Err(PreviewError::Io(e)),
        }
    }
}

fn rewrite_hls_playlist(playlist: &str, segment_url_prefix: &str) -> String {
    playlist
        .lines()
        .map(|line| {
            if sanitize_hls_segment(line).is_some() {
                format!("{segment_url_prefix}{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn hls_output_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn fallback_session() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn trim_stderr_tail(output: &str, redacted_path: Option<&str>) -> String {
    let sanitized = sanitize_status_text(output.trim(), redacted_path);
    if sanitized.len() <= STDERR_TAIL_LIMIT {
        return sanitized;
    }

    let mut tail = VecDeque::new();
    let mut total_len = 0;
    for ch in sanitized.chars().rev() {
        let len = ch.len_utf8();
        if total_len + len > STDERR_TAIL_LIMIT {
            break;
        }
        tail.push_front(ch);
        total_len += len;
    }
    tail.into_iter().collect()
}

fn sanitize_status_text(text: impl Into<String>, redacted_path: Option<&str>) -> String {
    let mut text = text.into();
    if let Some(path) = redacted_path.filter(|path| !path.is_empty()) {
        text = text.replace(path, "[source]");
    }

    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn exit_status_to_string(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn source_key(path: &Path) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("file not found")]
    NotFound,
    #[error("unsupported media type")]
    Unsupported,
    #[error("ffmpeg is not available")]
    FfmpegUnavailable,
    #[error("server-side execution is disabled")]
    ServerSideExecutionDisabled,
    #[error("io error: {0}")]
    Io(std::io::Error),
    #[error("process error: {0}")]
    Process(String),
    #[error("response error: {0}")]
    Response(String),
}

impl IntoResponse for PreviewError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            PreviewError::NotFound => (StatusCode::NOT_FOUND, "file not found"),
            PreviewError::Unsupported => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported media type")
            }
            PreviewError::FfmpegUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "ffmpeg is not available")
            }
            PreviewError::ServerSideExecutionDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server-side execution is disabled",
            ),
            PreviewError::Io(_) | PreviewError::Process(_) | PreviewError::Response(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };
        (status, Json(serde_json::json!({"error": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_video_preview_kind() {
        assert_eq!(
            preview_kind(Path::new("clip.mp4")),
            Some(PreviewKind::Video)
        );
    }

    #[test]
    fn detects_audio_preview_kind() {
        assert_eq!(
            preview_kind(Path::new("track.mp3")),
            Some(PreviewKind::Audio)
        );
    }

    #[test]
    fn rejects_non_media_preview_kind() {
        assert_eq!(preview_kind(Path::new("notes.txt")), None);
    }

    #[test]
    fn disabled_error_maps_to_service_unavailable() {
        let response = PreviewError::ServerSideExecutionDisabled.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn disabled_guard_runs_before_file_or_ffmpeg_work() {
        let service = MediaPreviewService::new(1);
        let result = service
            .serve_media_preview(
                Path::new("/definitely/not/a/file.mp4"),
                Some("test"),
                None,
                None,
                false,
            )
            .await;

        assert!(matches!(
            result,
            Err(PreviewError::ServerSideExecutionDisabled)
        ));
    }

    #[test]
    fn transcode_profiles_use_expected_caps() {
        let video_args = transcode_args(PreviewKind::Video);
        assert!(video_args.contains(&"libx264"));
        assert!(video_args.contains(&"yuv420p"));
        assert!(video_args.contains(&"128k"));
        assert!(video_args.contains(&"baseline"));
        assert!(video_args.contains(&"3.0"));
        assert!(video_args.iter().any(|arg| arg.contains("min(480,ih)")));

        let audio_args = transcode_args(PreviewKind::Audio);
        assert!(audio_args.contains(&"aac"));
        assert!(audio_args.contains(&"128k"));
        assert!(audio_args.contains(&"-vn"));
    }

    #[test]
    fn preview_content_types_include_browser_codecs() {
        assert_eq!(
            PreviewKind::Video.content_type(),
            "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""
        );
        assert_eq!(
            PreviewKind::Audio.content_type(),
            "audio/mp4; codecs=\"mp4a.40.2\""
        );
    }

    #[test]
    fn hls_command_uses_seekable_event_playlist() {
        let command = build_hls_ffmpeg_command(
            Path::new("clip.avi"),
            Path::new("preview.m3u8"),
            Path::new("segment_%05d.ts"),
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|pair| pair == ["-hls_list_size", "0"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-hls_playlist_type", "event"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-hls_flags", "independent_segments+temp_file"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-force_key_frames", "expr:gte(t,n_forced*4)"])
        );
    }

    #[test]
    fn hls_segment_names_are_strictly_sanitized() {
        assert_eq!(
            sanitize_hls_segment("segment_00001.ts"),
            Some("segment_00001.ts")
        );
        assert_eq!(sanitize_hls_segment("../segment_00001.ts"), None);
        assert_eq!(sanitize_hls_segment("segment_abcde.ts"), None);
        assert_eq!(sanitize_hls_segment("segment_00001.m3u8"), None);
    }

    #[test]
    fn hls_playlist_rewrites_segment_urls() {
        let playlist = "#EXTM3U\n#EXTINF:4.0,\nsegment_00000.ts\n#EXT-X-ENDLIST\n";
        let rewritten = rewrite_hls_playlist(
            playlist,
            "/api/files/root/preview?path=x&session=y&segment=",
        );

        assert!(
            rewritten.contains("/api/files/root/preview?path=x&session=y&segment=segment_00000.ts")
        );
        assert!(rewritten.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn status_store_records_failure_and_prunes_text() {
        let store = PreviewStatusStore::new();
        let started_at = Instant::now();

        let source_key = source_key(Path::new("/tmp/media.mp4"));
        store.start("abc", &source_key, "video_480p_h264_aac_128k_hls");
        store.mark_streaming("abc");
        store.update_bytes("abc", 1024, started_at);
        store.mark_failed(
            "abc",
            started_at,
            Some(1024),
            Some("1".to_string()),
            Some(sanitize_status_text("bad\u{0}thing", None)),
        );
        store.update_stderr(
            "abc",
            trim_stderr_tail(&"x".repeat(STDERR_TAIL_LIMIT + 20), None),
        );

        let status = store.get("abc", &source_key).expect("status should exist");
        assert_eq!(status.state, "failed");
        assert_eq!(status.bytes_sent, 1024);
        assert_eq!(status.exit_status.as_deref(), Some("1"));
        assert_eq!(status.error.as_deref(), Some("bad thing"));
        assert!(status.stderr_tail.unwrap().len() <= STDERR_TAIL_LIMIT);
    }

    #[test]
    fn status_store_binds_session_to_source() {
        let store = PreviewStatusStore::new();
        store.start("abc", "source-a", "audio_aac_128k_frag_mp4");

        assert!(store.get("abc", "source-a").is_some());
        assert!(store.get("abc", "source-b").is_none());
    }

    #[test]
    fn stderr_status_redacts_source_path() {
        let output = trim_stderr_tail(
            "failed to open /srv/private/video.mov",
            Some("/srv/private/video.mov"),
        );

        assert_eq!(output, "failed to open [source]");
    }
}
