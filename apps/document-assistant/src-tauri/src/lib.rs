use tauri::Manager;

pub mod analysis;
pub mod chat;
mod commands;
pub mod credentials;
pub mod db;
pub mod events;
pub mod models;
pub mod rendering;
pub mod search;
pub mod storage;
pub mod tasks;

#[cfg(test)]
mod storage_test;

#[cfg(test)]
mod credentials_test;

#[cfg(test)]
mod rendering_test;

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
            commands::pick_and_register_document,
            commands::save_document_markdown,
            commands::list_documents,
            commands::delete_document_cache,
            commands::clear_all_document_caches,
            commands::save_model_profile,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::list_model_profiles,
            commands::test_model_profile,
            commands::analyze_document,
            commands::get_document_analysis,
            commands::ask_document,
            commands::get_conversation_messages,
            commands::start_document_task,
            commands::get_task,
            commands::list_recoverable_tasks,
            commands::pause_task,
            commands::cancel_task,
            commands::resume_task,
            commands::retry_failed_pages,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AnyDoc Assistant");
}
