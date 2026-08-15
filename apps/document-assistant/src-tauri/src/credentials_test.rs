use crate::{
    credentials::{MemorySecretStore, SecretStore},
    db::{ModelProfile, ModelRole},
};

#[test]
fn serialized_profile_never_contains_api_key() {
    let profile = ModelProfile {
        id: "vision-primary".into(),
        role: ModelRole::Vision,
        base_url: "https://api.example.com/v1".into(),
        model: "vision-model".into(),
        supports_vision: true,
        timeout_secs: 120,
        max_concurrency: 2,
    };

    let json = serde_json::to_string(&profile).expect("profile serializes");

    assert!(!json.to_lowercase().contains("api_key"));
    assert!(!json.contains("sk-test"));
}

#[test]
fn secret_store_round_trips_without_joining_the_profile() {
    let store = MemorySecretStore::default();
    store.set("vision-primary", "sk-test").expect("secret stores");
    assert_eq!(store.get("vision-primary").expect("secret reads"), Some("sk-test".into()));
    store.delete("vision-primary").expect("secret deletes");
    assert_eq!(store.get("vision-primary").expect("empty reads"), None);
}
