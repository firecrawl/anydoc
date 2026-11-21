//! Shared budgets for non-archive frontends that build the document model.

use crate::error::ConvertError;
use crate::package::limits;

/// Tracks allocations whose count can be amplified independently of
/// the input byte length, such as CSV cells and RTF text runs.
#[derive(Debug, Clone)]
pub(crate) struct MaterializationBudget {
    text_bytes: usize,
    cells: usize,
    text_runs: usize,
    max_input_bytes: usize,
    max_text_bytes: usize,
    max_cells: usize,
    max_text_runs: usize,
}

impl Default for MaterializationBudget {
    fn default() -> Self {
        Self {
            text_bytes: 0,
            cells: 0,
            text_runs: 0,
            max_input_bytes: limits::MAX_STANDALONE_INPUT_BYTES,
            max_text_bytes: limits::MAX_MATERIALIZED_TEXT_BYTES,
            max_cells: limits::MAX_MATERIALIZED_CELLS,
            max_text_runs: limits::MAX_MATERIALIZED_TEXT_RUNS,
        }
    }
}

impl MaterializationBudget {
    /// Reject a standalone input before a decoder or lexer duplicates it.
    pub(crate) fn check_input(&self, bytes: usize) -> Result<(), ConvertError> {
        if bytes > self.max_input_bytes {
            return Err(ConvertError::ResourceLimit {
                limit: "max_standalone_input_bytes",
                detail: format!(
                    "standalone input is {bytes} bytes (limit {})",
                    self.max_input_bytes
                ),
            });
        }
        Ok(())
    }

    /// Charge text bytes copied into retained strings in the model.
    pub(crate) fn charge_text(&mut self, bytes: usize) -> Result<(), ConvertError> {
        self.text_bytes =
            self.text_bytes.checked_add(bytes).ok_or_else(|| ConvertError::ResourceLimit {
                limit: "max_materialized_text_bytes",
                detail: "materialized text byte counter overflowed".into(),
            })?;
        if self.text_bytes > self.max_text_bytes {
            return Err(ConvertError::ResourceLimit {
                limit: "max_materialized_text_bytes",
                detail: format!(
                    "materialized text reached {} bytes (limit {})",
                    self.text_bytes, self.max_text_bytes
                ),
            });
        }
        Ok(())
    }

    /// Charge one content-bearing table cell.
    pub(crate) fn charge_cell(&mut self) -> Result<(), ConvertError> {
        self.cells = self.cells.saturating_add(1);
        if self.cells > self.max_cells {
            return Err(ConvertError::ResourceLimit {
                limit: "max_materialized_cells",
                detail: format!(
                    "materialized cell count reached {} (limit {})",
                    self.cells, self.max_cells
                ),
            });
        }
        Ok(())
    }

    /// Charge one separately allocated text run.
    pub(crate) fn charge_text_run(&mut self) -> Result<(), ConvertError> {
        self.text_runs = self.text_runs.saturating_add(1);
        if self.text_runs > self.max_text_runs {
            return Err(ConvertError::ResourceLimit {
                limit: "max_materialized_text_runs",
                detail: format!(
                    "materialized text run count reached {} (limit {})",
                    self.text_runs, self.max_text_runs
                ),
            });
        }
        Ok(())
    }

    /// Construct a small deterministic budget for unit tests.
    #[cfg(test)]
    pub(crate) fn with_limits(
        max_input_bytes: usize,
        max_text_bytes: usize,
        max_cells: usize,
        max_text_runs: usize,
    ) -> Self {
        Self { max_input_bytes, max_text_bytes, max_cells, max_text_runs, ..Self::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit_name(error: ConvertError) -> &'static str {
        match error {
            ConvertError::ResourceLimit { limit, .. } => limit,
            other => panic!("expected resource limit, got {other}"),
        }
    }

    #[test]
    fn every_counter_is_hard_bounded() {
        let budget = MaterializationBudget::with_limits(3, 3, 1, 1);
        assert_eq!(limit_name(budget.check_input(4).unwrap_err()), "max_standalone_input_bytes");

        let mut budget = MaterializationBudget::with_limits(10, 3, 1, 1);
        budget.check_input(3).unwrap();
        budget.charge_text(3).unwrap();
        assert_eq!(limit_name(budget.charge_text(1).unwrap_err()), "max_materialized_text_bytes");
        budget.charge_cell().unwrap();
        assert_eq!(limit_name(budget.charge_cell().unwrap_err()), "max_materialized_cells");
        budget.charge_text_run().unwrap();
        assert_eq!(limit_name(budget.charge_text_run().unwrap_err()), "max_materialized_text_runs");
    }
}
