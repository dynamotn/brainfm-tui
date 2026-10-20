//! Serde types for the Brain.fm API, shaped after what the web app reads.

use std::time::Duration;

use serde::{Deserialize, Serialize};

const AUDIO_BASE: &str = "https://audio.brain.fm/";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentalState {
    pub id: String,
    pub display_value: String,
    #[serde(default)]
    pub default_activity: Option<Activity>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: String,
    pub display_value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EffectLevel {
    Low,
    Medium,
    High,
}

impl EffectLevel {
    pub const ALL: [EffectLevel; 3] = [EffectLevel::Low, EffectLevel::Medium, EffectLevel::High];

    /// Maps a track variation's 0–1 level the way the web app displays it.
    pub fn from_value(v: f64) -> Self {
        if v <= 0.33 {
            Self::Low
        } else if v <= 0.66 {
            Self::Medium
        } else {
            Self::High
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tag {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub tags: Vec<Tag>,
}

impl Track {
    pub fn genre(&self) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.kind == "genre" && t.value != "Nature")
            .map(|t| t.value.as_str())
    }

    pub fn moods(&self) -> Vec<&str> {
        self.tags
            .iter()
            .filter(|t| t.kind == "mood")
            .map(|t| t.value.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackVariation {
    pub id: String,
    #[serde(default)]
    pub tokened_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub length_in_seconds: f64,
    #[serde(default)]
    pub neural_effect_level: Option<f64>,
}

impl TrackVariation {
    pub fn stream_url(&self) -> Option<String> {
        self.tokened_url
            .clone()
            .filter(|u| !u.is_empty())
            .or_else(|| self.url.as_ref().map(|u| format!("{AUDIO_BASE}{u}")))
    }

    pub fn length(&self) -> Duration {
        Duration::try_from_secs_f64(self.length_in_seconds).unwrap_or_default()
    }

    pub fn effect(&self) -> Option<EffectLevel> {
        self.neural_effect_level.map(EffectLevel::from_value)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Serving {
    pub track: Track,
    pub track_variation: TrackVariation,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    pub dynamic_activity_id: String,
    pub version: u8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub neural_effect_levels: Vec<EffectLevel>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genre_names: Vec<String>,
}

impl SessionRequest {
    pub fn new(activity_id: &str, effects: Vec<EffectLevel>, genres: Vec<String>) -> Self {
        Self {
            dynamic_activity_id: activity_id.to_owned(),
            version: 3,
            neural_effect_levels: effects,
            genre_names: genres,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn effect_from_value_thresholds() {
        assert_eq!(EffectLevel::from_value(0.33), EffectLevel::Low);
        assert_eq!(EffectLevel::from_value(0.34), EffectLevel::Medium);
        assert_eq!(EffectLevel::from_value(0.66), EffectLevel::Medium);
        assert_eq!(EffectLevel::from_value(0.67), EffectLevel::High);
    }

    #[test]
    fn session_request_omits_empty_filters() {
        let req = SessionRequest::new("act1", vec![EffectLevel::High], vec![]);
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("genreNames").is_none());
        assert_eq!(json["neuralEffectLevels"], serde_json::json!(["High"]));
        assert_eq!(json["dynamicActivityId"], "act1");
        assert_eq!(json["version"], 3);
    }

    #[test]
    fn tolerates_extra_and_missing_fields() {
        let s: Serving = serde_json::from_value(serde_json::json!({
            "foo": 1,
            "track": {"id": "t1", "name": "Ascent", "tags": [], "bar": true},
            "trackVariation": {"id": "v1", "tokenedUrl": null, "url": "x/y.mp3",
                               "lengthInSeconds": 478.5}
        }))
        .unwrap();
        assert_eq!(s.track.image_url, None);
        assert_eq!(
            s.track_variation.stream_url().as_deref(),
            Some("https://audio.brain.fm/x/y.mp3")
        );
        assert_eq!(s.track_variation.length(), Duration::from_secs_f64(478.5));
        assert_eq!(s.track_variation.effect(), None);
    }

    #[test]
    fn track_genre_skips_nature() {
        let t: Track = serde_json::from_value(serde_json::json!({
            "id": "t1", "name": "n",
            "tags": [
                {"type": "genre", "value": "Nature"},
                {"type": "genre", "value": "Piano"},
                {"type": "mood", "value": "Calm"}
            ]
        }))
        .unwrap();
        assert_eq!(t.genre(), Some("Piano"));
        assert_eq!(t.moods(), vec!["Calm"]);
    }
}
