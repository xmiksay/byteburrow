//! Host-side face comparison — the single "is this a known person?" decision.
//!
//! Both the classification job (`src/job/face.rs`) and the CLI `face_match`
//! tool route through [`match_embedding`], so the threshold and ambiguity rules
//! live in exactly one place. Previously the two entry points ran disconnected
//! matchers with disagreeing hardcoded thresholds (0.8 vs 0.5) and no guard
//! against ambiguous matches.
//!
//! The matcher is pure single-nearest-neighbour by contact with two guards:
//! a configurable **similarity threshold** and a **margin** between the best
//! contact and the best *different* contact, which rejects ambiguous matches
//! where two people are almost equally close.

use std::collections::HashMap;

/// A confirmed exemplar embedding belonging to a known contact.
///
/// Borrows its embedding and model identity so callers can decode the stored
/// references once and reuse the slice across many query faces.
pub struct Exemplar<'a> {
    pub contact_id: i32,
    pub model_id: &'a str,
    pub model_version: &'a str,
    pub embedding: &'a [f32],
}

/// Tunables for the match decision. Sourced from `Config` so every entry point
/// agrees on what "known person" means.
#[derive(Clone, Copy, Debug)]
pub struct MatchParams {
    /// Minimum cosine similarity of the best contact for a positive match.
    pub threshold: f32,
    /// Minimum gap between the best contact's similarity and the best
    /// *different* contact's similarity. Rejects ambiguous matches where two
    /// people are almost equally close. Set to `0.0` to disable the guard.
    pub margin: f32,
}

/// A positive, unambiguous match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMatch {
    pub contact_id: i32,
    /// Best cosine similarity between the query and the matched contact.
    pub similarity: f32,
    /// Best similarity to any *other* contact, or `None` when only one contact
    /// had a comparable exemplar. This is what the margin guard tested against.
    pub runner_up: Option<f32>,
}

/// Full outcome of a match attempt, including diagnostics for the caller.
#[derive(Clone, Copy, Debug)]
pub struct MatchOutcome {
    /// The winning contact, or `None` when nothing cleared the threshold or the
    /// match was rejected as ambiguous by the margin guard.
    pub best: Option<FaceMatch>,
    /// Confirmed exemplars skipped because their embedding model differed from
    /// the query's — comparing across models is meaningless, so they are
    /// refused rather than scored 0.
    pub skipped_cross_model: usize,
}

/// Decide which known contact (if any) a query embedding belongs to.
///
/// Scores each contact by its single best (nearest) exemplar, then applies the
/// threshold and margin guards from `params`. Only exemplars sharing the query's
/// `(model_id, model_version)` are considered; the rest are counted in
/// [`MatchOutcome::skipped_cross_model`].
pub fn match_embedding(
    query: &[f32],
    query_model_id: &str,
    query_model_version: &str,
    exemplars: &[Exemplar],
    params: MatchParams,
) -> MatchOutcome {
    // Best (max) similarity per contact — single nearest neighbour, grouped so
    // the margin guard can compare the winner against a *different* person.
    let mut per_contact: HashMap<i32, f32> = HashMap::new();
    let mut skipped_cross_model = 0usize;

    for ex in exemplars {
        if ex.model_id != query_model_id || ex.model_version != query_model_version {
            skipped_cross_model += 1;
            continue;
        }
        let Some(sim) = cosine_similarity(query, ex.embedding) else {
            // Same model yet incomparable dimensions ⇒ corrupt data; skip it
            // rather than let it poison the score.
            continue;
        };
        let slot = per_contact
            .entry(ex.contact_id)
            .or_insert(f32::NEG_INFINITY);
        if sim > *slot {
            *slot = sim;
        }
    }

    let mut ranked: Vec<(i32, f32)> = per_contact.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let best = ranked
        .first()
        .copied()
        .and_then(|(contact_id, similarity)| {
            if similarity < params.threshold {
                return None;
            }
            let runner_up = ranked.get(1).map(|(_, s)| *s);
            if let Some(second) = runner_up {
                if similarity - second < params.margin {
                    // Ambiguous: two contacts are within the margin of each other.
                    return None;
                }
            }
            Some(FaceMatch {
                contact_id,
                similarity,
                runner_up,
            })
        });

    MatchOutcome {
        best,
        skipped_cross_model,
    }
}

/// Cosine similarity of two embeddings, or `None` when they are incomparable
/// (differing dimensions or empty). Returning `None` instead of silently
/// scoring 0 keeps a dimension mismatch from masquerading as "not similar".
///
/// Callers must only pass embeddings already confirmed to share a model
/// identity; [`match_embedding`] enforces this.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        Some(dot / (norm_a * norm_b))
    } else {
        Some(0.0)
    }
}

/// Encode an embedding for storage in the `face_reference.embedding` blob.
pub fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode a stored `face_reference.embedding` blob back into an embedding.
pub fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "test-model";
    const VER: &str = "1";

    fn ex<'a>(contact_id: i32, embedding: &'a [f32]) -> Exemplar<'a> {
        Exemplar {
            contact_id,
            model_id: MODEL,
            model_version: VER,
            embedding,
        }
    }

    fn params(threshold: f32, margin: f32) -> MatchParams {
        MatchParams { threshold, margin }
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = [0.5f32, 0.5, 0.5, 0.5];
        let sim = cosine_similarity(&v, &v).unwrap();
        assert!((sim - 1.0).abs() < 1e-6, "got {sim}");
    }

    #[test]
    fn cosine_of_orthogonal_vectors_is_zero() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        assert!(cosine_similarity(&a, &b).unwrap().abs() < 1e-6);
    }

    #[test]
    fn cosine_dimension_mismatch_is_none_not_zero() {
        // Incomparable vectors must report "unknown" (None), never a real 0.0.
        assert_eq!(cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0]), None);
    }

    #[test]
    fn cosine_empty_is_none() {
        let empty: [f32; 0] = [];
        assert_eq!(cosine_similarity(&empty, &empty), None);
    }

    #[test]
    fn floats_bytes_roundtrip() {
        let floats = vec![0.0f32, -1.5, 3.5, 42.0];
        let bytes = floats_to_bytes(&floats);
        assert_eq!(bytes.len(), floats.len() * 4);
        assert_eq!(bytes_to_floats(&bytes), floats);
    }

    #[test]
    fn matches_nearest_contact_above_threshold() {
        let alice = [1.0f32, 0.0, 0.0];
        let bob = [0.0f32, 1.0, 0.0];
        let exemplars = [ex(1, &alice), ex(2, &bob)];
        let query = [0.98f32, 0.02, 0.0];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.05));
        let m = out.best.expect("should match");
        assert_eq!(m.contact_id, 1);
        assert!(m.similarity > 0.8);
        assert_eq!(out.skipped_cross_model, 0);
    }

    #[test]
    fn rejects_below_threshold() {
        let alice = [1.0f32, 0.0];
        let exemplars = [ex(1, &alice)];
        // ~45° away ⇒ ~0.707 similarity, under a 0.8 threshold.
        let query = [1.0f32, 1.0];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.0));
        assert!(out.best.is_none());
    }

    #[test]
    fn margin_test_rejects_ambiguous_match() {
        // Two different contacts almost equally close to the query — the whole
        // point of the margin guard. Nearest alone would (wrongly) pick one.
        let alice = [1.0f32, 0.0];
        let bob = [0.9998f32, 0.02];
        let exemplars = [ex(1, &alice), ex(2, &bob)];
        let query = [0.999f32, 0.01];

        // Without a margin the nearest wins.
        let lenient = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.0));
        assert!(lenient.best.is_some());

        // With a real margin the ambiguity is rejected.
        let strict = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.05));
        assert!(strict.best.is_none(), "ambiguous match should be rejected");
    }

    #[test]
    fn margin_allows_clear_winner() {
        let alice = [1.0f32, 0.0];
        let bob = [0.0f32, 1.0];
        let exemplars = [ex(1, &alice), ex(2, &bob)];
        let query = [0.99f32, 0.01];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.1));
        let m = out.best.expect("clear winner should pass the margin");
        assert_eq!(m.contact_id, 1);
        assert!(m.runner_up.unwrap() < m.similarity);
    }

    #[test]
    fn single_contact_needs_no_margin() {
        let alice = [1.0f32, 0.0];
        let exemplars = [ex(1, &alice)];
        let query = [0.99f32, 0.01];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.5));
        assert_eq!(out.best.unwrap().contact_id, 1);
        assert!(out.best.unwrap().runner_up.is_none());
    }

    #[test]
    fn best_exemplar_wins_within_a_contact() {
        // A contact with several exemplars is scored by its nearest, so a second
        // exemplar of the SAME person never counts as an ambiguous runner-up.
        let far = [0.0f32, 1.0];
        let near = [1.0f32, 0.0];
        let exemplars = [ex(1, &far), ex(1, &near)];
        let query = [0.99f32, 0.01];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.5));
        let m = out
            .best
            .expect("same-person second exemplar must not block");
        assert_eq!(m.contact_id, 1);
        assert!(m.runner_up.is_none());
    }

    #[test]
    fn cross_model_exemplars_are_skipped_and_counted() {
        let alice = [1.0f32, 0.0];
        let other = [1.0f32, 0.0];
        let exemplars = [
            Exemplar {
                contact_id: 1,
                model_id: "other-model",
                model_version: VER,
                embedding: &other,
            },
            ex(2, &alice),
        ];
        let query = [0.99f32, 0.01];

        let out = match_embedding(&query, MODEL, VER, &exemplars, params(0.8, 0.0));
        assert_eq!(out.skipped_cross_model, 1);
        assert_eq!(out.best.unwrap().contact_id, 2);
    }
}
