use std::path::Path;

use remote_server::transport::Error;

/// Installs a matching local NeoWarp binary through the existing SSH connection.
pub(super) async fn install_binary(socket_path: &Path) -> Result<(), Error> {
    let platform = super::detect_remote_platform(socket_path).await?;
    let artifact = remote_server::setup::local_artifact_path(&platform);
    let metadata = async_fs::metadata(&artifact).await.map_err(|error| {
        Error::Other(anyhow::anyhow!(
            "Matching remote server binary unavailable at '{}': {error}",
            artifact.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(Error::Other(anyhow::anyhow!(
            "Matching remote server binary is not a non-empty file: '{}'",
            artifact.display()
        )));
    }

    let install_dir = remote_server::setup::remote_server_dir();
    let relative_dir = install_dir
        .strip_prefix("~/")
        .expect("home-relative install directory");
    let mkdir_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &format!("mkdir -p \"$HOME\"/{}", shell_words::quote(relative_dir)),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await
    .map_err(Error::from)?;
    if !mkdir_output.status.success() {
        return Err(Error::ScriptFailed {
            exit_code: mkdir_output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&mkdir_output.stderr).into_owned(),
        });
    }

    let staging_name = format!(".neowarp-server-upload-{}", uuid::Uuid::new_v4());
    let staging_path = format!("{install_dir}/{staging_name}");
    let timeout = remote_server::setup::SCP_INSTALL_TIMEOUT;
    let result = async {
        log::info!(
            "Uploading remote server binary from '{}' to {staging_path}",
            artifact.display()
        );
        remote_server::ssh::scp_upload(socket_path, &artifact, &staging_path, timeout)
            .await
            .map_err(Error::Other)?;
        let script = remote_server::setup::install_script(&staging_path);
        let output = remote_server::ssh::run_ssh_script(socket_path, &script, timeout)
            .await
            .map_err(Error::from)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::ScriptFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }
    .await;

    // This invocation owns only its unique upload, including a partial SCP file.
    // Cleanup must never replace the original upload/install error.
    if result.is_err() {
        let cleanup = format!(
            "rm -f \"$HOME\"/{}",
            shell_words::quote(&format!("{relative_dir}/{staging_name}"))
        );
        match remote_server::ssh::run_ssh_command(
            socket_path,
            &cleanup,
            remote_server::setup::CHECK_TIMEOUT,
        )
        .await
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => log::warn!(
                "Failed to clean remote server upload: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => log::warn!("Failed to clean remote server upload: {error}"),
        }
    }
    result?;

    let binary_path = remote_server::setup::remote_server_binary();
    log::info!("Running post-install verification for {binary_path}");
    let output = remote_server::ssh::run_ssh_command(
        socket_path,
        &remote_server::setup::binary_check_command(),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await
    .map_err(|error| Error::Other(anyhow::anyhow!("Post-install verification failed: {error}")))?;
    if !output.status.success() {
        return Err(Error::Other(anyhow::anyhow!(
            "Post-install verification failed: binary not found or not executable at {binary_path} (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
