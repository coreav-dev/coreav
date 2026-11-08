pub fn inter_stats(stats: coreav_hevc::InterStats) -> serde_json::Value {
    serde_json::json!({"skip_units":stats.skip_units,"prediction_units":stats.prediction_units,
        "p_units":stats.p_units,"b_units":stats.b_units,"bi_units":stats.bi_units,
        "merge_units":stats.merge_units,"amvp_lists":stats.amvp_lists,
        "temporal_units":stats.temporal_units,"weighted_units":stats.weighted_units,
        "weighted_changed_samples":stats.weighted_changed_samples,
        "fractional_luma_units":stats.fractional_luma_units,
        "fractional_chroma_units":stats.fractional_chroma_units,
        "negative_motion_units":stats.negative_motion_units,"long_term_units":stats.long_term_units,
        "list_modification_units":stats.list_modification_units,"partitions":stats.partitions,
        "boundary_strengths":stats.boundary_strengths,"boundary_changed_samples":stats.boundary_changed_samples})
}

pub fn header_stats(headers: coreav_hevc::Summary) -> serde_json::Value {
    serde_json::json!({"nals":headers.nals,"pictures":headers.pictures,
        "warnings":{"profile_bit_depth":headers.warnings.profile_bit_depth,
            "invalid_sample_aspect_ratio":headers.warnings.invalid_sample_aspect_ratio,
            "temporal_nesting_nal_type":headers.warnings.temporal_nesting_nal_type},
        "output_flag_pictures":headers.output_flag_pictures,"output_pictures":headers.output_pictures,
        "independent_slices":headers.independent_slices,"dependent_slices":headers.dependent_slices,
        "picture_hashes":headers.picture_hashes,"reserved_hashes":headers.reserved_hashes})
}
