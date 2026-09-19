//! Fuzz target: the representation-selection policy.
//!
//! Properties: `resolve_audio` and `AudioPreference::from_value` never panic
//! on arbitrary byte-derived inputs; resolution always terminates; the result
//! index is always in range; the inputs are never mutated.
#![no_main]

use libfuzzer_sys::fuzz_target;
use musicpack_core::json::Value;
use musicpack_core::policy::{
    AudioPreference, Candidate, Playability, RepresentationRef, SourceRef, TrackAudio,
    item_id, resolve_audio,
};

struct Spec {
    codecs: Option<Vec<String>>,
    reject_mimes: Vec<String>,
    reject_ids: Vec<f64>,
}

impl Playability for Spec {
    fn can_play(&self, candidate: &Candidate<'_>) -> bool {
        if let SourceRef::Representation { id } = candidate.source {
            if self.reject_ids.contains(&id) {
                return false;
            }
        }
        if self.reject_mimes.contains(&candidate.mime_type.unwrap_or("").to_lowercase()) {
            return false;
        }
        match &self.codecs {
            Some(codecs) => codecs.contains(&candidate.codec.unwrap_or("").to_lowercase()),
            None => true,
        }
    }
}

fn codec(byte: u8) -> Option<String> {
    match byte % 4 {
        0 => Some("flac".to_string()),
        1 => Some("wav".to_string()),
        2 => Some("musepack-sv8".to_string()),
        _ => None,
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // Deterministic track derived from the bytes.
    let reps: Vec<RepresentationRef> = data
        .chunks(3)
        .take(32)
        .enumerate()
        .map(|(i, chunk)| RepresentationRef {
            id: (i as f64) - (chunk.first().copied().unwrap_or(0) as f64 % 5.0),
            codec: codec(chunk.first().copied().unwrap_or(0)),
            mime_type: codec(chunk.get(1).copied().unwrap_or(0)).map(|c| format!("audio/{c}")),
        })
        .collect();
    let track = TrackAudio {
        codec: codec(data[0]),
        mime_type: codec(data.get(1).copied().unwrap_or(0)).map(|c| format!("audio/{c}")),
        representations: reps,
    };
    let spec = Spec {
        codecs: if data.len() % 2 == 0 {
            Some(
                data.iter()
                    .take(8)
                    .filter_map(|b| codec(*b))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        },
        reject_mimes: data
            .iter()
            .take(4)
            .filter_map(|b| codec(*b))
            .map(|c| format!("audio/{c}"))
            .collect(),
        reject_ids: data
            .iter()
            .take(8)
            .enumerate()
            .map(|(i, b)| (i as f64) - (*b as f64 % 5.0))
            .collect(),
    };

    // Preference parsing on a byte-derived JSON object (and arbitrary junk).
    let pref_value = Value::Object(vec![
        (
            "mode".into(),
            Value::String(
                match data[0] % 5 {
                    0 => "default",
                    1 => "representation",
                    2 => "codec",
                    3 => "lossless",
                    _ => "shiny",
                }
                .into(),
            ),
        ),
        ("id".into(), Value::Number(data[0] as f64 / 7.0)),
        ("codec".into(), Value::String(codec(data[0]).unwrap_or_default())),
    ]);
    let parsed = AudioPreference::from_value(&pref_value);

    let before = track.clone();
    for pref in [
        None,
        Some(AudioPreference::Default),
        Some(AudioPreference::Lossless),
        Some(AudioPreference::Codec {
            codec: "flac".into(),
        }),
        Some(AudioPreference::Representation { id: 0.0 }),
        parsed,
    ] {
        let selected = resolve_audio(&track, pref.as_ref(), &spec);
        if let Some(index) = selected.representation {
            assert!(index < track.representations.len());
            let _ = item_id(index as i64, selected.representation_id(&track));
        } else {
            let _ = item_id(-1, None);
        }
    }
    assert_eq!(track, before, "policy must not mutate its input");
});
