//! Deterministic audio preview, rendered without an audio device or hub.

use kazoo_glass::patch::PATCHES;
use kazoo_glass::synth::GlassSynth;
use std::path::Path;

/// Six four-second vignettes, one per factory patch, as stereo float WAV.
pub fn render(path: &Path) -> color_eyre::Result<()> {
    let rate = 48_000;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for patch in PATCHES {
        let mut synth = GlassSynth::new(rate as f32);
        synth.set_patch(patch);
        for frame in 0..rate * 4 {
            match frame {
                0 => synth.note_on(60, 110),
                24_000 => synth.note_on(67, 90),
                48_000 => synth.note_on(72, 105),
                72_000 => {
                    synth.note_on(64, 85);
                    synth.note_on(79, 70);
                }
                96_000 => {
                    for note in [60, 67, 72, 64, 79] {
                        synth.note_off(note);
                    }
                }
                168_000 => synth.all_notes_off(),
                _ => {}
            }
            for sample in synth.process() {
                writer.write_sample(sample)?;
            }
        }
    }
    writer.finalize()?;
    println!(
        "Rendered six factory sounds to {} (24s, stereo, 48 kHz)",
        path.display()
    );
    Ok(())
}
