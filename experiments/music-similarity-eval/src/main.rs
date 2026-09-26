use std::time::Instant;

use music_similarity_eval::audio;
use music_similarity_eval::corpus;
use music_similarity_eval::eval::{TrackEmbedding, aggregate_albums, pool_mean_norm};
use music_similarity_eval::mel::{
    FRAME_HOP, FRAME_SIZE, MEL_BANDS, MelFrontend, PATCH_SIZE, SAMPLE_RATE,
};
use music_similarity_eval::model::ModelRunner;
use music_similarity_eval::report::{ReportMetadata, write_reports};
use music_similarity_eval::{Config, Result};

fn main() {
    if let Err(error) = run() {
        eprintln!("music-similarity-eval: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let Some(config) = Config::from_args()? else {
        return Ok(());
    };
    let started = Instant::now();

    let actual_hash = audio::sha256_file(&config.model_path)?;
    if actual_hash != config.model_sha256 {
        return Err(format!(
            "model SHA-256 mismatch: expected {}, got {}",
            config.model_sha256, actual_hash
        )
        .into());
    }
    println!("model digest verified: {actual_hash}");

    let selected = corpus::select_library(&config.library, config.albums, config.tracks_per_album)?;
    let selected_tracks = selected.albums.iter().map(Vec::len).sum::<usize>();
    println!(
        "corpus selected: {} albums / {} tracks ({} supported files discovered)",
        selected.albums.len(),
        selected_tracks,
        selected.discovered_files
    );
    if selected.albums.is_empty() {
        return Err("no supported audio files found in the selected corpus".into());
    }

    let frontend = MelFrontend::new(config.patch_hop)?;
    let model = ModelRunner::load(&config.model_path)?;
    let model_input_name = model.input_name();
    let model_input_shape = model.input_shape();
    let model_output_name = model.output_name();
    let model_output_shape = model.output_shape();
    println!(
        "model loaded: input={model_input_shape} output_node={model_output_name} output={model_output_shape}"
    );

    let mut records = Vec::new();
    let mut skipped = 0usize;
    for (album_index, album) in selected.albums.iter().enumerate() {
        for (track_index, spec) in album.iter().enumerate() {
            let label = format!("album {} track {}", album_index + 1, track_index + 1);
            match analyze_track(&frontend, &model, spec) {
                Ok(Some(record)) => {
                    println!("embedded {label}");
                    records.push(record);
                }
                Ok(None) => {
                    println!("skipped {label}: no complete model patch");
                    skipped += 1;
                }
                Err(error) => {
                    println!("skipped {label}: {error}");
                    skipped += 1;
                }
            }
        }
    }

    if records.is_empty() {
        return Err("no track produced an embedding".into());
    }
    let albums = aggregate_albums(&records);
    let corpus_identity = corpus_identity(&records);
    let metadata = ReportMetadata {
        model_name: format!("Discogs-EffNet {}", config.model_variant),
        model_version: "EffnetDiscogs version 1; official release 2022-06-15".to_string(),
        model_source: format!(
            "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_{}_embeddings-effnet-bs64-1.onnx",
            config.model_variant
        ),
        model_file: config
            .model_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model.onnx")
            .to_string(),
        model_size_bytes: std::fs::metadata(&config.model_path)?.len(),
        model_sha256: actual_hash,
        model_input_name,
        model_input_shape,
        model_output_name,
        model_output_shape,
        model_license: "CC BY-NC-SA 4.0 (weights); Essentia AGPL/commercial terms".to_string(),
        corpus_root_sha256: audio::sha256_text(&config.library.to_string_lossy()),
        corpus_identity,
        corpus_selection: "sorted album groups; evenly spaced album selection; evenly spaced track limit within each album".to_string(),
        discovered_files: selected.discovered_files,
        unsupported_audio_files: selected.unsupported_audio_files,
        runtime: "rten 0.26.0".to_string(),
        runtime_license: "MIT OR Apache-2.0".to_string(),
        rust_version: "package MSRV 1.94; host observed rustc 1.97.1".to_string(),
        sample_rate: SAMPLE_RATE,
        decoder: "musicpack-core::audio::open/read_f32; WAV, FLAC, and Musepack SV8".to_string(),
        downmix: "arithmetic channel mean in f64, cast to f32".to_string(),
        resampler: "rubato 0.16.2 FftFixedIn, 1024-frame chunks, oversampling factor 2; trim startup delay and round output length".to_string(),
        window: "symmetric Hann, normalized=false, zeroPhase=true half rotation".to_string(),
        mel_filterbank: "96 bands, Slaney mel, linear weighting, unit_tri area normalization, power spectrum".to_string(),
        log_scaling: "log10(max(1e-30, mel * 10000 + 1))".to_string(),
        frame_policy: "centered 512-sample frames at hop 256; zero-pad boundaries; require complete 128-frame patches".to_string(),
        batch_policy: "fixed model batch 64; zero-pad only the final batch and discard padded outputs".to_string(),
        embedding_dimensions: model.embedding_dim(),
        frame_size: FRAME_SIZE,
        frame_hop: FRAME_HOP,
        mel_bands: MEL_BANDS,
        patch_size: PATCH_SIZE,
        patch_hop: config.patch_hop,
        pooling: "per-window L2 mean, then track L2".to_string(),
        album_aggregation: "equal mean of unit track embeddings, then album L2".to_string(),
        normalization: "L2".to_string(),
        similarity: "cosine".to_string(),
        elapsed_seconds: started.elapsed().as_secs_f64(),
    };
    write_reports(&config.output, &metadata, &records, config.top_k)?;
    println!(
        "wrote {} track embeddings and {} album aggregates to {} ({} skipped, {:.3}s)",
        records.len(),
        albums.len(),
        config.output.display(),
        skipped,
        metadata.elapsed_seconds
    );
    Ok(())
}

fn corpus_identity(records: &[TrackEmbedding]) -> String {
    let mut entries = records
        .iter()
        .map(|record| format!("{}\0{}", record.spec.path.display(), record.source_sha256))
        .collect::<Vec<_>>();
    entries.sort();
    audio::sha256_text(&entries.join("\n"))
}

fn analyze_track(
    frontend: &MelFrontend,
    model: &ModelRunner,
    spec: &corpus::TrackSpec,
) -> Result<Option<TrackEmbedding>> {
    let decoded = audio::decode_and_prepare(&spec.path)?;
    let frontend_output = frontend.process(&decoded.samples);
    if frontend_output.patches.is_empty() {
        return Ok(None);
    }
    let windows = model.run_patches(&frontend_output.patches)?;
    let vector = pool_mean_norm(&windows, model.embedding_dim())?;
    let embedding_sha256 = audio::sha256_f32(&vector);
    Ok(Some(TrackEmbedding {
        spec: spec.clone(),
        duration_seconds: decoded.duration_seconds,
        source_sha256: decoded.source_sha256,
        frame_count: frontend_output.frame_count,
        patch_count: frontend_output.patches.len(),
        embedding_sha256,
        vector,
    }))
}
