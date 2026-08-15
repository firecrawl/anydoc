use anydoc_assistant_lib::db::{ModelProfile, ModelProfileRepository, ModelRole};

#[test]
fn persisted_model_profile_schema_never_contains_credentials() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("security.db");
    let profiles = ModelProfileRepository::open(&database).expect("profiles open");
    profiles
        .upsert(&ModelProfile {
            id: "text-primary".into(),
            role: ModelRole::Text,
            base_url: "https://api.example.com/v1".into(),
            model: "text-model".into(),
            supports_vision: false,
            timeout_secs: 30,
            max_concurrency: 1,
        })
        .expect("profile saves");
    let persisted = std::fs::read(&database).expect("database reads");
    let searchable = String::from_utf8_lossy(&persisted).to_lowercase();
    assert!(!searchable.contains("api_key"));
    assert!(!searchable.contains("authorization: bearer"));
}
