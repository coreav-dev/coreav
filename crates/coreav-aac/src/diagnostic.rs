use crate::ics::{Bands, Channel, NOISE_HCB};
use crate::tools::bands_of;

#[derive(Clone, Debug)]
/// One coded noise band in one long or short window.
pub struct NoiseBand {
    /// First coefficient in the de-interleaved frame spectrum.
    pub start: usize,
    /// Coefficient immediately after the band.
    pub end: usize,
    /// Coded noise energy scalefactor.
    pub sf: i32,
}

#[derive(Clone, Debug)]
/// Coded window facts and normalized post-tool spectrum of one channel.
pub struct ChannelDiagnostic {
    /// The coded window sequence, from zero to three.
    pub sequence: u8,
    /// The coded current window shape: sine or KBD.
    pub shape: u8,
    /// Individual noise bands, not sums over short-window groups.
    pub noise: Vec<NoiseBand>,
    /// The decoder's complete pre-synthesis spectrum.
    pub spectrum: Vec<f32>,
}

pub(crate) fn channel(channel: &Channel, bands: &Bands) -> ChannelDiagnostic {
    let noise = bands_of(channel.info, bands)
        .filter_map(|(group, band, start, end)| {
            let codebook = channel.cb.get(group)?.get(band)?;
            (*codebook == NOISE_HCB).then(|| NoiseBand {
                start,
                end,
                sf: channel
                    .sf
                    .get(group)
                    .and_then(|row| row.get(band))
                    .copied()
                    .unwrap_or(0),
            })
        })
        .collect();
    ChannelDiagnostic {
        sequence: channel.info.window_sequence,
        shape: channel.info.window_shape,
        noise,
        spectrum: channel.spec.get(..bands.frame).unwrap_or_default().to_vec(),
    }
}
