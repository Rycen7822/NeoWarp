use dirs::home_dir;

use super::*;

#[test]
fn test_data_dir_path() {
    if let Some(root) = std::env::var_os("NEOWARP_DATA_DIR") {
        let root = PathBuf::from(root);
        assert_eq!(data_dir(), root.join("data"));
        assert_eq!(state_dir(), root.join("state"));
        assert_eq!(config_local_dir(), root.join("config"));
        assert_eq!(cache_dir(), root.join("cache"));
        assert_eq!(warp_home_config_dir(), Some(root.join("config")));
        return;
    }
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(data_dir(), home_dir.join(".neowarp-oss"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(data_dir(), home_dir.join(".local/share/neowarp"));
        } else if #[cfg(windows)] {
            assert_eq!(data_dir(), home_dir.join("AppData\\Roaming\\neowarp\\NeoWarp\\data"));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_config_local_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(config_local_dir(), home_dir.join(".neowarp-oss"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(config_local_dir(), home_dir.join(".config/neowarp"));
        } else if #[cfg(windows)] {
            assert_eq!(config_local_dir(), home_dir.join("AppData\\Local\\neowarp\\NeoWarp\\config"));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_config_dir_name_scopes_to_data_profile() {
    assert_eq!(
        macos_config_dir_name_for(Channel::Oss, None),
        ".neowarp-oss"
    );
    assert_eq!(
        macos_config_dir_name_for(Channel::Integration, None),
        ".neowarp-integration"
    );

    // Each development profile must get its own directory so shared config
    // (notably settings.toml) cannot leak between profiles.
    assert_eq!(
        macos_config_dir_name_for(Channel::Integration, Some("myprofile")),
        ".neowarp-integration-myprofile"
    );
    assert_eq!(
        macos_config_dir_name_for(Channel::Oss, Some("myprofile")),
        ".neowarp-oss-myprofile"
    );
}

#[test]
fn test_warp_home_config_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    let expected_dir_name = match ChannelState::data_profile() {
        Some(data_profile) => format!(".neowarp-oss-{data_profile}"),
        None => ".neowarp-oss".to_string(),
    };

    assert_eq!(
        warp_home_config_dir(),
        Some(home_dir.join(expected_dir_name))
    );
}

#[test]
fn test_cache_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(cache_dir(), home_dir.join("Library/Application Support/dev.neowarp.NeoWarp"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(cache_dir(), home_dir.join(".cache/neowarp"));
        } else if #[cfg(windows)] {
            assert_eq!(cache_dir(), home_dir.join("AppData\\Local\\neowarp\\NeoWarp\\cache"));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_state_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    cfg_if::cfg_if! {
        // ChannelState, by default, is configured for Channel::Oss.
        if #[cfg(target_os = "macos")] {
            assert_eq!(state_dir(), home_dir.join("Library/Application Support/dev.neowarp.NeoWarp"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(state_dir(), home_dir.join(".local/state/neowarp"));
        } else if #[cfg(windows)] {
            assert_eq!(state_dir(), home_dir.join("AppData\\Local\\neowarp\\NeoWarp\\data"));
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_neowarp_app_id() {
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "neowarp", "NeoWarp"), None)
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.neowarp.NeoWarp");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "neowarp");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "neowarp\\NeoWarp");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_neowarp_data_profile() {
    let project_dirs =
        project_dirs_for_app_id(AppId::new("dev", "neowarp", "NeoWarp"), Some("myprofile"))
            .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.neowarp.NeoWarp-myprofile");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "neowarp-myprofile");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "neowarp\\NeoWarp-myprofile");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_integration_app_id() {
    let project_dirs =
        project_dirs_for_app_id(AppId::new("dev", "neowarp", "NeoWarpIntegration"), None)
            .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.neowarp.NeoWarpIntegration");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "neowarpintegration");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "neowarp\\NeoWarpIntegration");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}
