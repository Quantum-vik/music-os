//! Arranger traits and song-section engines.
//!
//! Sections are the structural layer above patterns (`docs/05` §1 project
//! layer): a [`SectionPlan`] names consecutive spans of bars (intro, verse,
//! chorus…), and [`SectionPlan::offsets`] converts them to tick positions —
//! where markers go and where clips get placed. Pure math, 4/4 in v1.

use musicos_core_types::{Tick, PPQ};

/// Ticks per 4/4 bar.
pub const BAR: i64 = PPQ * 4;

/// One named span of bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Section name ("intro", "verse", "chorus", …).
    pub name: String,
    /// Length in bars. Always ≥ 1.
    pub bars: usize,
}

/// A song structure: consecutive sections from a start bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionPlan {
    /// Sections in timeline order.
    pub sections: Vec<Section>,
    /// Bar the first section starts at.
    pub start_bar: usize,
}

/// A section resolved to its timeline position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedSection {
    /// The section.
    pub section: Section,
    /// First bar (0-based).
    pub start_bar: usize,
    /// Tick position of the section start.
    pub at: Tick,
}

impl SectionPlan {
    /// Builds a plan, rejecting empty plans and zero-length sections.
    ///
    /// # Errors
    /// Returns [`ArrangementError`] on invalid input.
    pub fn new(sections: Vec<Section>, start_bar: usize) -> Result<SectionPlan, ArrangementError> {
        if sections.is_empty() {
            return Err(ArrangementError::Empty);
        }
        if let Some(bad) = sections
            .iter()
            .find(|s| s.bars == 0 || s.name.trim().is_empty())
        {
            return Err(ArrangementError::InvalidSection(bad.name.clone()));
        }
        Ok(SectionPlan {
            sections,
            start_bar,
        })
    }

    /// Resolves every section to its start bar and tick.
    pub fn offsets(&self) -> Vec<PlacedSection> {
        let mut bar = self.start_bar;
        self.sections
            .iter()
            .map(|s| {
                let placed = PlacedSection {
                    section: s.clone(),
                    start_bar: bar,
                    at: Tick(i64::try_from(bar).expect("bar count fits i64") * BAR),
                };
                bar += s.bars;
                placed
            })
            .collect()
    }

    /// Total length in bars.
    pub fn total_bars(&self) -> usize {
        self.sections.iter().map(|s| s.bars).sum()
    }
}

/// Errors from section planning.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArrangementError {
    /// The plan had no sections.
    #[error("a section plan needs at least one section")]
    Empty,
    /// A section had no bars or no name.
    #[error("invalid section '{0}': needs a name and at least one bar")]
    InvalidSection(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> SectionPlan {
        SectionPlan::new(
            vec![
                Section {
                    name: "intro".into(),
                    bars: 4,
                },
                Section {
                    name: "verse".into(),
                    bars: 8,
                },
                Section {
                    name: "chorus".into(),
                    bars: 8,
                },
            ],
            0,
        )
        .unwrap()
    }

    #[test]
    fn offsets_are_cumulative() {
        let placed = plan().offsets();
        assert_eq!(placed[0].at, Tick(0));
        assert_eq!(placed[1].at, Tick(4 * BAR));
        assert_eq!(placed[2].at, Tick(12 * BAR));
        assert_eq!(plan().total_bars(), 20);
    }

    #[test]
    fn start_bar_shifts_everything() {
        let mut p = plan();
        p.start_bar = 2;
        assert_eq!(p.offsets()[0].at, Tick(2 * BAR));
    }

    #[test]
    fn invalid_plans_are_rejected() {
        assert_eq!(
            SectionPlan::new(vec![], 0).unwrap_err(),
            ArrangementError::Empty
        );
        assert!(SectionPlan::new(
            vec![Section {
                name: "x".into(),
                bars: 0
            }],
            0
        )
        .is_err());
        assert!(SectionPlan::new(
            vec![Section {
                name: "  ".into(),
                bars: 4
            }],
            0
        )
        .is_err());
    }
}
