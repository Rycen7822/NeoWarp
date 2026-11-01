use settings::SupportedPlatforms;
use settings::macros::define_settings_group;

define_settings_group!(SshSettings,
    settings: [
        reuse_existing_control_master: ReuseExistingSshControlMaster {
            type: bool,
            default: false,
            supported_platforms: SupportedPlatforms::ALL,
            private: false,
            storage_key: "ReuseExistingSshControlMaster",
            toml_path: "warpify.ssh.reuse_existing_control_master",
            description: "Whether the legacy SSH wrapper attaches to an existing SSH ControlMaster for the destination host instead of always creating its own.",
        },
    ]
);

/// Returns the stable local partition ID for remote daemon sockets and data.
/// This is not an authentication credential.
pub(crate) fn remote_server_instance_id(ctx: &warpui::AppContext) -> anyhow::Result<String> {
    use warp_core::user_preferences::GetUserPreferences;

    const REMOTE_SERVER_INSTANCE_ID_KEY: &str = "NeoWarpRemoteServerInstanceId";
    let preferences = ctx.private_user_preferences();
    if let Some(value) = preferences.read_value(REMOTE_SERVER_INSTANCE_ID_KEY)? {
        return Ok(uuid::Uuid::parse_str(&value)?.to_string());
    }

    let instance_id = uuid::Uuid::new_v4().to_string();
    preferences.write_value(REMOTE_SERVER_INSTANCE_ID_KEY, instance_id.clone())?;
    Ok(instance_id)
}

#[cfg(test)]
#[test]
fn remote_server_instance_id_is_stable_across_reads() {
    warpui::App::test((), |mut app| async move {
        app.update(crate::settings::init_and_register_user_preferences);
        app.update(|ctx| {
            let instance_id = remote_server_instance_id(ctx).unwrap();
            assert_eq!(remote_server_instance_id(ctx).unwrap(), instance_id);
        });
    });
}
