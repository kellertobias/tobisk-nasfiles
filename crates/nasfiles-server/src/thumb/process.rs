use std::path::Path;
use std::process::Output;
use std::time::Duration;

use tokio::process::Command;

use super::cache::ThumbError;

pub async fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
    tool: &'static str,
    source_path: &Path,
) -> Result<Option<Output>, ThumbError> {
    command.kill_on_drop(true);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(Some(output)),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Ok(Err(e)) => Err(ThumbError::Process(e.to_string())),
        Err(_) => {
            tracing::warn!("{tool} thumbnail timed out for {}", source_path.display());
            Ok(None)
        }
    }
}
