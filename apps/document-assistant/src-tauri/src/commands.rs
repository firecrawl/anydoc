use serde::Serialize;
use tauri::Manager;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub app_data_dir: String,
}

pub(crate) fn app_info_for_test(app_data_dir: String) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        app_data_dir,
    }
}

#[tauri::command]
pub fn get_app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(app_info_for_test(app_data_dir.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::app_info_for_test;

    #[test]
    fn app_info_contains_a_version_and_camel_case_data_dir() {
        let info = app_info_for_test("C:\\Data".into());
        assert!(!info.version.is_empty());

        let json = serde_json::to_value(info).expect("app info serializes");
        assert_eq!(json["appDataDir"], "C:\\Data");
    }
}
