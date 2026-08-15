mod conversations;
mod documents;
mod model_profiles;
mod models;

pub use conversations::{ConversationMessage, ConversationRepository};
pub use documents::DocumentRepository;
pub use model_profiles::{ModelProfile, ModelProfileRepository, ModelRole};
pub use models::{AnalysisConsentRecord, DocumentPageRecord, DocumentRecord, PageAnalysisRecord};

pub(crate) const ANALYSIS_MIGRATION: &str = include_str!("../../migrations/0002_analysis.sql");

#[cfg(test)]
mod documents_test;
