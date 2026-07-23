use byteburrow_plugin_api::ClassificationResult;

/// Aggregated results from all plugins for a single file.
#[derive(Debug, Default, Clone)]
pub struct MergedClassification {
    pub keywords: Vec<String>,
    pub custom: serde_json::Map<String, serde_json::Value>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub date_unix: Option<i64>,
}

impl MergedClassification {
    pub(crate) fn absorb(&mut self, r: ClassificationResult) {
        self.keywords.extend(r.keywords);
        for (k, v) in r.custom {
            self.custom.insert(k, v);
        }
        if r.latitude.is_some() {
            self.latitude = r.latitude;
        }
        if r.longitude.is_some() {
            self.longitude = r.longitude;
        }
        if r.date_unix.is_some() {
            self.date_unix = r.date_unix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with(
        keywords: &[&str],
        custom: &[(&str, serde_json::Value)],
    ) -> ClassificationResult {
        ClassificationResult {
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            custom: custom
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn absorb_accumulates_keywords_across_calls() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(&["cat", "sunset"], &[]));
        merged.absorb(result_with(&["beach"], &[]));

        assert_eq!(merged.keywords, vec!["cat", "sunset", "beach"]);
    }

    #[test]
    fn absorb_merges_custom_metadata_by_key() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 100}))],
        ));
        merged.absorb(result_with(&[], &[("faces", serde_json::json!(2))]));

        assert_eq!(merged.custom.len(), 2);
        assert_eq!(merged.custom["exif"], serde_json::json!({"iso": 100}));
        assert_eq!(merged.custom["faces"], serde_json::json!(2));
    }

    #[test]
    fn absorb_later_custom_value_overwrites_earlier_for_same_key() {
        let mut merged = MergedClassification::default();
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 100}))],
        ));
        merged.absorb(result_with(
            &[],
            &[("exif", serde_json::json!({"iso": 200}))],
        ));

        assert_eq!(merged.custom["exif"], serde_json::json!({"iso": 200}));
    }

    #[test]
    fn absorb_sets_geo_and_date_fields() {
        let mut merged = MergedClassification::default();
        merged.absorb(ClassificationResult {
            latitude: Some(50.1),
            longitude: Some(14.4),
            date_unix: Some(1_700_000_000),
            ..Default::default()
        });

        assert_eq!(merged.latitude, Some(50.1));
        assert_eq!(merged.longitude, Some(14.4));
        assert_eq!(merged.date_unix, Some(1_700_000_000));
    }

    #[test]
    fn absorb_keeps_previous_geo_when_later_plugin_has_none() {
        let mut merged = MergedClassification::default();
        merged.absorb(ClassificationResult {
            latitude: Some(50.1),
            longitude: Some(14.4),
            ..Default::default()
        });
        merged.absorb(ClassificationResult::default());

        assert_eq!(merged.latitude, Some(50.1));
        assert_eq!(merged.longitude, Some(14.4));
    }
}
