use tauri::Manager;

mod commands;
pub mod credentials;
pub mod db;
pub mod events;
pub mod storage;

#[cfg(test)]
mod storage_test;

#[cfg(test)]
mod credentials_test;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            app.manage(commands::AppState::open(app_data_dir)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::register_document,
            commands::list_documents,
            commands::delete_document_cache,
            commands::clear_all_document_caches,
            commands::save_model_profile,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::list_model_profiles,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AnyDoc Assistant");
}
