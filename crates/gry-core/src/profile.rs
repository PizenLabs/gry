//! AST structural profile: a compact, deterministic 25-dimensional vector used
//! for semantic relatedness and architecture heuristics.

use serde::{Deserialize, Serialize};

pub const DIMS: usize = 25;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AstProfile {
    pub if_count: u32,
    pub for_count: u32,
    pub while_count: u32,
    pub switch_count: u32,
    pub try_count: u32,
    pub return_count: u32,
    pub max_nesting_depth: u32,
    pub avg_nesting_depth_x10: u32,
    pub comparison_ops: u32,
    pub arithmetic_ops: u32,
    pub logical_ops: u32,
    pub assignment_count: u32,
    pub string_literals: u32,
    pub number_literals: u32,
    pub bool_literals: u32,
    pub param_count: u32,
    pub params_in_returns: u32,
    pub params_in_conditions: u32,
    pub variable_reassigns: u32,
    pub unique_operators: u32,
    pub unique_operands: u32,
    pub total_operators: u32,
    pub total_operands: u32,
    pub body_lines: u32,
    pub body_tokens: u32,
}

impl AstProfile {
    pub fn to_string_compact(&self) -> String {
        let v = self.to_vector();
        v.iter()
            .map(|x| format!("{x:.4}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn from_string_compact(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != DIMS {
            return None;
        }
        let nums: Vec<f32> = parts
            .iter()
            .map(|p| p.trim().parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        // Recovering exact integer counters from the normalized vector is not
        // possible; keep the vector as the canonical representation by mapping
        // it back to approximate counts.
        let mut out = AstProfile::default();
        out.if_count = (nums[0] * 100.0).round() as u32;
        out.for_count = (nums[1] * 100.0).round() as u32;
        out.while_count = (nums[2] * 100.0).round() as u32;
        out.switch_count = (nums[3] * 100.0).round() as u32;
        out.try_count = (nums[4] * 100.0).round() as u32;
        out.return_count = (nums[5] * 100.0).round() as u32;
        out.max_nesting_depth = (nums[6] * 20.0).round() as u32;
        out.avg_nesting_depth_x10 = (nums[7] * 100.0).round() as u32;
        Some(out)
    }

    /// Normalized vector for cosine similarity. Counts are log-scaled and
    /// clamped so a single huge function does not dominate.
    pub fn to_vector(&self) -> [f32; DIMS] {
        let raw = [
            self.if_count as f32,
            self.for_count as f32,
            self.while_count as f32,
            self.switch_count as f32,
            self.try_count as f32,
            self.return_count as f32,
            self.max_nesting_depth as f32,
            self.avg_nesting_depth_x10 as f32 / 10.0,
            self.comparison_ops as f32,
            self.arithmetic_ops as f32,
            self.logical_ops as f32,
            self.assignment_count as f32,
            self.string_literals as f32,
            self.number_literals as f32,
            self.bool_literals as f32,
            self.param_count as f32,
            self.params_in_returns as f32,
            self.params_in_conditions as f32,
            self.variable_reassigns as f32,
            self.unique_operators as f32,
            self.unique_operands as f32,
            self.total_operators as f32,
            self.total_operands as f32,
            self.body_lines as f32,
            self.body_tokens as f32,
        ];
        let mut v = [0.0f32; DIMS];
        for i in 0..DIMS {
            v[i] = (1.0 + raw[i]).ln();
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    pub fn cosine(&self, other: &AstProfile) -> f32 {
        let a = self.to_vector();
        let b = other.to_vector();
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_is_normalized() {
        let p = AstProfile {
            if_count: 3,
            body_lines: 20,
            body_tokens: 100,
            ..Default::default()
        };
        let norm: f32 = p.to_vector().iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn similar_profiles_score_higher() {
        let a = AstProfile {
            if_count: 2,
            for_count: 1,
            body_lines: 10,
            body_tokens: 50,
            ..Default::default()
        };
        let b = AstProfile {
            if_count: 2,
            for_count: 1,
            body_lines: 12,
            body_tokens: 55,
            ..Default::default()
        };
        let c = AstProfile {
            switch_count: 8,
            try_count: 4,
            body_lines: 100,
            body_tokens: 900,
            ..Default::default()
        };
        assert!(a.cosine(&b) > a.cosine(&c));
    }
}
