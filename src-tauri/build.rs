/// Every command the renderer may invoke. Listing them here makes Tauri generate an
/// `allow-<command>` permission for each, so only windows whose capability grants
/// them (the main window, see capabilities/default.json) can call them — the
/// equivalent of the Electron app's assertTrustedSender check.
const COMMANDS: &[&str] = &[
    "settings_load",
    "settings_save",
    "settings_replace_api_key",
    "settings_select_output_dir",
    "ai_detect",
    "ai_test",
    "whisper_list",
    "whisper_download",
    "whisper_cancel",
    "whisper_delete",
    "job_start",
    "job_cancel",
    "jobs_list",
    "jobs_dismiss",
    "history_list",
    "history_get_job",
    "thumbnails_generate",
    "shell_open_path",
    "shell_show_item_in_folder",
    "dialog_select_video",
    "clips_bulk_export",
    "system_is_packaged",
    "system_check_tools",
    "diagnostics_get_log_path",
    "diagnostics_open_log_folder",
    "update_check",
    "update_download",
    "update_install",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
